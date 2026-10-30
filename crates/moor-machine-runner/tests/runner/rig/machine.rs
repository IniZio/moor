//! The fake machine: its agent, linked to the rig's runner, runs commands in the rig's remote
//! home and the hook files in the rig's hooks dir.

use std::path::{Path, PathBuf};

use moor_machine_agent::{run, Machine, State};
use ssh_key::rand_core::OsRng;
use ssh_key::{Algorithm, LineEnding, PrivateKey};

use super::{FakeLink, MACHINE_ID};

/// Stands in for `ssh` and leaves a mark beside itself, so a test can tell it was never run.
pub(super) const NO_SSH: &str = "#!/bin/sh\ntouch \"$0.ran\"\nexit 255\n";

/// Stands in for `df -Pk`: reports the free KiB kept in `~/.free-kb`, plenty when there is
/// none, so a test sets the machine's disk by writing that file.
const FAKE_DF: &str = r#"#!/bin/sh
kb=$(cat "$HOME/.free-kb" 2>/dev/null || echo 999999999)
echo "Filesystem 1024-blocks Used Available Capacity Mounted on"
echo "/dev/fake 1000000000 1 $kb 1% $HOME"
"#;

/// Starts the agent of machine `minion` linking to the runner's agent port; it runs until the
/// returned runtime drops.
pub(super) fn start_agent(
    dir: &Path,
    remote: &Path,
    port: u16,
    link: &FakeLink,
) -> tokio::runtime::Runtime {
    let tools = dir.join("machine-bin");
    std::fs::create_dir_all(&tools).unwrap();
    moor_testkit::write_executable(&tools.join("df"), FAKE_DF).unwrap();
    let shell = tools.join("sh");
    let wrapper = format!(
        "#!/bin/sh\nPATH='{}':\"$PATH\" exec /bin/sh \"$@\"\n",
        tools.display()
    );
    moor_testkit::write_executable(&shell, &wrapper).unwrap();
    let key = PrivateKey::random(&mut OsRng, Algorithm::Ed25519).unwrap();
    *link.pubkey.lock().unwrap() = Some(key.public_key().to_openssh().unwrap());
    let state_dir = dir.join("agent");
    std::fs::create_dir_all(&state_dir).unwrap();
    let state = State {
        server: format!("http://127.0.0.1:{port}"),
        machine_id: MACHINE_ID,
        owner: "ada@example.com".to_owned(),
        name: "minion".to_owned(),
        slots: 1,
    };
    let json = serde_json::to_string(&state).unwrap();
    std::fs::write(state_dir.join("state.json"), json).unwrap();
    let text = key.to_openssh(LineEnding::LF).unwrap();
    std::fs::write(state_dir.join("id_ed25519"), text.as_bytes()).unwrap();
    let machine = Machine {
        home: remote.to_path_buf(),
        shell,
        hooks: hooks_dir(dir),
        pool: None,
        allow_exec: true,
    };
    let agent = tokio::runtime::Runtime::new().unwrap();
    agent.spawn(async move { run(&state_dir, machine, &|_| {}).await });
    agent
}

/// The fake machine's hooks dir in the rig at `dir`.
pub(super) fn hooks_dir(dir: &Path) -> PathBuf {
    dir.join("hooks")
}
