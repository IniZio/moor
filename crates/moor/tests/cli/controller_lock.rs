//! Real controllers on one Postgres database: only the holder of the advisory lock serves, and a
//! pod with its own state dir stands in for each replica. Skipped without a database.

use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use moor_testkit::ChildGuard;
use postgres::{Client, NoTls};

use super::{fresh_dir, Dir};

/// The variable naming the database, as the store tests read it.
const POSTGRES_URL: &str = "MOOR_TEST_POSTGRES_URL";

/// How long a frozen holder's liveness file takes to fail the probe, past the chart's 20 s.
const PROBE_FAILS: Duration = Duration::from_secs(30);

/// How long a controller may take to serve once nothing stops it.
const START: Duration = Duration::from_secs(15);

/// Time between looks at a controller.
const POLL: Duration = Duration::from_millis(20);

/// A schema of the test database, standing for one moor deployment's database.
struct Deployment {
    /// The test database's URL.
    url: String,
    /// The schema's name.
    schema: String,
    /// Every pod's config, so all share the one vault key beside them, as the chart's secret does.
    config: Dir,
}

impl Deployment {
    /// A new empty schema, or none without a test database.
    fn new(tag: &str) -> Option<Self> {
        let url = std::env::var(POSTGRES_URL).ok()?;
        let dir = fresh_dir("lock-name");
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        let schema = format!("moor_lock_{tag}_{}", name.replace('-', "_").to_lowercase());
        let config = fresh_dir("lock-config");
        let this = Self {
            url,
            schema,
            config,
        };
        this.admin()
            .batch_execute(&format!("CREATE SCHEMA {}", this.schema))
            .unwrap();
        Some(this)
    }

    /// This schema's URL, its connections marked with `app`.
    fn url_for(&self, app: &str) -> String {
        let sep = if self.url.contains('?') { '&' } else { '?' };
        format!(
            "{}{sep}application_name={app}&options=-c%20search_path%3D{}",
            self.url, self.schema
        )
    }

    /// A connection of the test itself.
    fn admin(&self) -> Client {
        Client::connect(&self.url, NoTls).unwrap()
    }

    /// Whether `table` exists in the schema.
    fn has_table(&self, table: &str) -> bool {
        self.admin()
            .query_one(
                "SELECT to_regclass($1 || '.' || $2)::text IS NOT NULL",
                &[&self.schema, &table],
            )
            .unwrap()
            .get(0)
    }

    /// The name of the session holding the controller lock.
    fn holder(&self) -> Option<String> {
        let mut client = Client::connect(&self.url_for("lock_reader"), NoTls).unwrap();
        client
            .query_opt(
                "SELECT a.application_name FROM pg_locks l JOIN pg_stat_activity a ON a.pid = l.pid \
                 WHERE l.locktype = 'advisory' AND l.granted AND l.objid = 2 AND l.objsubid = 2 \
                 AND l.classid = hashtext(current_schema())::oid",
                &[],
            )
            .unwrap()
            .map(|row| row.get(0))
    }

    /// How many open connections carry `app`.
    fn connections(&self, app: &str) -> i64 {
        self.admin()
            .query_one(
                "SELECT count(*) FROM pg_stat_activity WHERE application_name = $1",
                &[&app],
            )
            .unwrap()
            .get(0)
    }

    /// A session holding the lock under `holder`, as an older build still serving would.
    fn hold_as(&self, holder: &str) -> Client {
        let mut client = Client::connect(&self.url_for(holder), NoTls).unwrap();
        client
            .batch_execute("SELECT pg_advisory_lock(hashtext(current_schema()), 2)")
            .unwrap();
        client
    }

    /// Ends the sessions named `app`, as a failover or `pg_terminate_backend` does.
    fn terminate(&self, app: &str) {
        self.admin()
            .query(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE application_name = $1",
                &[&app],
            )
            .unwrap();
    }
}

impl Drop for Deployment {
    fn drop(&mut self) {
        drop(
            self.admin()
                .batch_execute(&format!("DROP SCHEMA {} CASCADE", self.schema)),
        );
    }
}

/// One `moor controller serve`, in a state dir of its own as a pod has.
struct Pod {
    /// Its pod's name, which also marks its connections.
    name: String,
    /// Keeps the state dir until the pod is gone.
    root: Dir,
    /// The process.
    child: ChildGuard,
}

impl Pod {
    fn start(deployment: &Deployment, name: &str) -> Self {
        let root = fresh_dir("lock");
        let config = deployment.config.join(format!("{name}.yaml"));
        std::fs::write(
            &config,
            format!(
                "socket: {}\nstate_dir: {}\n",
                root.join("c.sock").display(),
                root.join("state").display()
            ),
        )
        .unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_moor"))
            .current_dir(&*root)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", root.join("home"))
            .env("MOOR_STATE_DIR", root.join("state"))
            .env("MOOR_DATABASE_URL", deployment.url_for(name))
            .env("HOSTNAME", name)
            .args(["controller", "serve", "--config"])
            .arg(&config)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Self {
            name: name.to_owned(),
            root,
            child: ChildGuard::new(child),
        }
    }

    fn socket(&self) -> PathBuf {
        self.root.join("c.sock")
    }

    fn serves(&self) -> bool {
        UnixStream::connect(self.socket()).is_ok()
    }

    /// Waits for it to serve within `limit`.
    fn serving_within(&self, limit: Duration) {
        for _ in 0..polls(limit) {
            if self.serves() {
                return;
            }
            std::thread::sleep(POLL);
        }
        panic!("{} did not serve within {limit:?}", self.name);
    }

    fn signal(&self, signal: rustix::process::Signal) {
        let pid = rustix::process::Pid::from_raw(i32::try_from(self.child.id()).unwrap()).unwrap();
        rustix::process::kill_process(pid, signal).unwrap();
    }

    /// Its exit code within `limit`.
    fn exit_within(&mut self, limit: Duration) -> Option<i32> {
        for _ in 0..polls(limit) {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.code();
            }
            std::thread::sleep(POLL);
        }
        panic!("{} did not exit within {limit:?}", self.name);
    }

    fn alive_file(&self) -> PathBuf {
        self.root.join("state").join("controller.alive")
    }
}

/// How many waits of [`POLL`] make up `limit`.
const fn polls(limit: Duration) -> u128 {
    limit.as_millis() / POLL.as_millis()
}

/// The time `path` last changed.
fn modified(path: &Path) -> std::time::SystemTime {
    std::fs::metadata(path).unwrap().modified().unwrap()
}

/// The chart's liveness probe command, run against `alive`.
fn probe_passes(alive: &Path) -> bool {
    let chart = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../deploy/helm/moor/templates/deployment.yaml"
    );
    let chart = std::fs::read_to_string(chart).unwrap();
    let line = chart
        .lines()
        .find(|l| l.contains("controller.alive"))
        .unwrap();
    let script = line
        .trim()
        .trim_start_matches("- '")
        .trim_end_matches('\'')
        .replace(
            "/var/lib/moor/pv/state/controller.alive",
            &alive.display().to_string(),
        );
    let status = ChildGuard::new(Command::new("sh").args(["-c", &script]).spawn().unwrap()).wait();
    status.unwrap().success()
}

/// A waiter stays quiet beside the holder, then serves as soon as the holder drains.
#[test]
fn a_waiter_has_one_connection_and_no_socket_and_takes_over_at_once_on_a_clean_exit() {
    let Some(deployment) = Deployment::new("clean") else {
        return;
    };
    let mut first = Pod::start(&deployment, "lock_first");
    first.serving_within(START);
    let second = Pod::start(&deployment, "lock_second");
    std::thread::sleep(Duration::from_secs(3));

    assert!(!second.socket().exists(), "a waiter binds no socket");
    assert_eq!(deployment.connections(&second.name), 1, "only its lock");
    assert_eq!(deployment.holder().as_deref(), Some("lock_first"));
    let seen = modified(&second.alive_file());
    std::thread::sleep(Duration::from_secs(6));
    assert!(
        modified(&second.alive_file()) > seen,
        "a waiter feeds liveness"
    );

    first.signal(rustix::process::Signal::TERM);
    assert_eq!(first.exit_within(START), Some(0));
    second.serving_within(Duration::from_secs(5));
    assert_eq!(deployment.holder().as_deref(), Some("lock_second"));
}

/// A holder that dies is replaced as soon as its socket closes, with no limit to wait out.
#[test]
fn a_waiter_takes_over_at_once_when_the_holder_crashes() {
    let Some(deployment) = Deployment::new("crash") else {
        return;
    };
    let mut first = Pod::start(&deployment, "lock_crashed");
    first.serving_within(START);
    let second = Pod::start(&deployment, "lock_after_crash");
    std::thread::sleep(Duration::from_secs(2));

    first.signal(rustix::process::Signal::KILL);
    assert_eq!(first.exit_within(START), None, "killed by the signal");
    second.serving_within(Duration::from_secs(5));
    assert_eq!(deployment.holder().as_deref(), Some("lock_after_crash"));
}

/// A frozen holder keeps the lock, carries on if it resumes, fails the probe if it does not, and
/// passes the lock on when the probe's kill comes.
#[test]
fn a_frozen_holder_is_not_replaced_until_the_liveness_kill() {
    let Some(deployment) = Deployment::new("frozen") else {
        return;
    };
    let mut first = Pod::start(&deployment, "lock_frozen");
    first.serving_within(START);
    let second = Pod::start(&deployment, "lock_after_freeze");
    std::thread::sleep(Duration::from_secs(1));
    assert!(probe_passes(&first.alive_file()), "a running holder passes");

    first.signal(rustix::process::Signal::STOP);
    std::thread::sleep(Duration::from_secs(2));
    first.signal(rustix::process::Signal::CONT);
    std::thread::sleep(Duration::from_secs(1));
    assert!(first.child.try_wait().unwrap().is_none(), "it carried on");
    assert_eq!(deployment.holder().as_deref(), Some("lock_frozen"));

    first.signal(rustix::process::Signal::STOP);
    std::thread::sleep(PROBE_FAILS);
    assert!(!probe_passes(&first.alive_file()), "a frozen holder fails");
    assert!(!second.serves(), "nothing takes over from a frozen holder");
    assert_eq!(deployment.holder().as_deref(), Some("lock_frozen"));

    first.signal(rustix::process::Signal::KILL);
    second.serving_within(Duration::from_secs(5));
    assert_eq!(deployment.holder().as_deref(), Some("lock_after_freeze"));
}

/// A holder whose session ends, as in a database restart, leaves at once.
#[test]
fn a_holder_exits_at_once_when_its_lock_connection_dies() {
    let Some(deployment) = Deployment::new("lost") else {
        return;
    };
    let mut first = Pod::start(&deployment, "lock_cut_off");
    first.serving_within(START);
    deployment.terminate("lock_cut_off");
    assert_eq!(first.exit_within(Duration::from_secs(2)), Some(1));
}

/// A newer build waiting beside a live older holder leaves the schema as the holder needs it.
#[test]
fn a_waiter_migrates_nothing_until_it_holds_the_lock() {
    let Some(deployment) = Deployment::new("order") else {
        return;
    };
    let older = deployment.hold_as("lock_older");
    let waiter = Pod::start(&deployment, "lock_newer");
    std::thread::sleep(Duration::from_secs(3));
    assert!(!waiter.serves());
    assert!(
        !deployment.has_table("refinery_schema_history"),
        "migrated beside a live holder"
    );

    drop(older);
    waiter.serving_within(START);
    assert!(deployment.has_table("refinery_schema_history"));
}

/// The first controller on an empty database takes the lock, then migrates.
#[test]
fn the_first_controller_on_an_empty_database_serves() {
    let Some(deployment) = Deployment::new("empty") else {
        return;
    };
    let pod = Pod::start(&deployment, "lock_empty");
    pod.serving_within(START);
    assert_eq!(deployment.holder().as_deref(), Some("lock_empty"));
    assert!(deployment.has_table("workflow_seq"));
}

/// A waiter told to stop leaves without the lock it was waiting for.
#[test]
fn a_waiter_told_to_stop_exits_at_once() {
    let Some(deployment) = Deployment::new("stop") else {
        return;
    };
    let live = deployment.hold_as("lock_live");
    let mut waiter = Pod::start(&deployment, "lock_stopped");
    std::thread::sleep(Duration::from_secs(2));
    waiter.signal(rustix::process::Signal::TERM);
    assert_eq!(waiter.exit_within(Duration::from_secs(3)), Some(0));
    assert_eq!(deployment.holder().as_deref(), Some("lock_live"));
    drop(live);
}

/// A controller whose lock could not be held says so and does not serve.
#[test]
fn a_controller_refuses_to_start_when_it_cannot_hold_the_lock() {
    let Some(mut deployment) = Deployment::new("noschema") else {
        return;
    };
    deployment.schema.push_str("_absent");
    let mut pod = Pod::start(&deployment, "lock_nowhere");
    assert_eq!(pod.exit_within(START), Some(3));
    assert!(!pod.serves());
    deployment
        .schema
        .truncate(deployment.schema.len() - "_absent".len());
}

/// A moor run with no controller, as `kubectl exec` with `MOOR_CONTROLLER` unset would be.
fn no_controller(deployment: &Deployment, args: &[&str]) -> std::process::Output {
    let root = fresh_dir("lock-cli");
    let mut child = ChildGuard::new(
        Command::new(env!("CARGO_BIN_EXE_moor"))
            .current_dir(&*root)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", root.join("home"))
            .env("MOOR_STATE_DIR", root.join("state"))
            .env("MOOR_DATABASE_URL", deployment.url_for("lock_cli"))
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    child.wait_with_output().unwrap()
}

#[test]
fn store_migrate_is_refused_beside_a_live_holder_and_migrates_nothing() {
    let Some(deployment) = Deployment::new("migrate") else {
        return;
    };
    let live = deployment.hold_as("lock_live");
    let out = no_controller(&deployment, &["store", "migrate"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(stderr.contains("lock_live"), "{stderr}");
    assert!(stderr.contains("MOOR_CONTROLLER"), "{stderr}");
    assert!(!deployment.has_table("refinery_schema_history"));
    drop(live);
}

#[test]
fn a_command_with_no_controller_is_refused_beside_a_live_holder() {
    let Some(deployment) = Deployment::new("cli") else {
        return;
    };
    let pod = Pod::start(&deployment, "lock_serving");
    pod.serving_within(START);
    let out = no_controller(&deployment, &["list"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{stderr}");
    assert!(stderr.contains("lock_serving"), "{stderr}");
}

#[test]
fn store_migrate_runs_when_no_holder_is_live() {
    let Some(deployment) = Deployment::new("free") else {
        return;
    };
    let out = no_controller(&deployment, &["store", "migrate"]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{stderr}");
    assert!(deployment.has_table("refinery_schema_history"));
    assert_eq!(deployment.holder(), None, "freed when the command ends");
}
