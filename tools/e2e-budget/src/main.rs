//! `e2e-budget`: reads moor's state DB and formats per-phase time-budget verdicts
//! for the real-Box end-to-end driver.

use std::collections::HashMap;
use std::process::ExitCode;
use std::time::Duration;

use e2e_budget::{
    count_kind, parse_jsonl, start_phases, timed, wake_phases, Event, Phase, StartBudgets,
    WakeBudgets,
};
use rusqlite::{Connection, OpenFlags};

/// How long a read waits on a writer holding the DB.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Parsed `--flag value` pairs following the subcommand.
struct Args(HashMap<String, String>);

impl Args {
    /// Parses `--flag value` pairs; rejects stray or valueless tokens.
    fn parse(raw: &[String]) -> Result<Self, String> {
        let mut map = HashMap::new();
        let mut it = raw.iter();
        while let Some(flag) = it.next() {
            let name = flag
                .strip_prefix("--")
                .ok_or_else(|| format!("unexpected argument `{flag}`"))?;
            let value = it.next().ok_or_else(|| format!("`{flag}` needs a value"))?;
            map.insert(name.to_owned(), value.clone());
        }
        Ok(Self(map))
    }

    /// Returns a required flag value.
    fn req(&self, name: &str) -> Result<&str, String> {
        self.0
            .get(name)
            .map(String::as_str)
            .ok_or_else(|| format!("missing --{name}"))
    }

    /// Parses a required integer flag.
    fn int(&self, name: &str) -> Result<i64, String> {
        let v = self.req(name)?;
        v.parse()
            .map_err(|_| format!("--{name}: `{v}` is not an integer"))
    }

    /// Parses an optional seconds flag (decimals allowed) into milliseconds.
    fn secs_ms(&self, name: &str, default: i64) -> Result<i64, String> {
        match self.0.get(name) {
            None => Ok(default),
            Some(v) => seconds_to_ms(v).ok_or_else(|| format!("--{name}: `{v}` is not seconds")),
        }
    }
}

/// Converts decimal seconds to whole milliseconds.
fn seconds_to_ms(text: &str) -> Option<i64> {
    let secs: f64 = text.parse().ok()?;
    if !secs.is_finite() || secs < 0.0 {
        return None;
    }
    #[allow(
        clippy::cast_possible_truncation,
        reason = "budgets are small non-negative second counts"
    )]
    Some((secs * 1000.0).round() as i64)
}

/// Reads and parses a JSONL events file.
fn read_events(args: &Args) -> Result<Vec<Event>, String> {
    let path = args.req("events")?;
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    parse_jsonl(&text)
}

/// Opens the state DB read-only with a busy timeout.
fn open_db(args: &Args) -> Result<Connection, String> {
    let path = args.req("db")?;
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("{path}: {e}"))?;
    conn.busy_timeout(BUSY_TIMEOUT).map_err(|e| e.to_string())?;
    Ok(conn)
}

/// Fetches events of the newest task called `name` with id above `after`.
fn fetch_events(conn: &Connection, name: &str, after: i64) -> Result<Vec<Event>, String> {
    let sql = "SELECT id, kind, ts FROM events WHERE id > ?2 AND task = \
               (SELECT id FROM tasks WHERE name = ?1 ORDER BY id DESC LIMIT 1) ORDER BY id";
    let mut stmt = conn.prepare(sql).map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map((name, after), |r| {
            Ok(Event {
                id: r.get(0)?,
                kind: r.get(1)?,
                ts_ms: r.get(2)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

/// `events`: prints a task's events as JSON lines.
fn cmd_events(args: &Args) -> Result<bool, String> {
    let after = if args.0.contains_key("after-id") {
        args.int("after-id")?
    } else {
        0
    };
    let conn = open_db(args)?;
    for event in fetch_events(&conn, args.req("task")?, after)? {
        println!(
            "{}",
            serde_json::to_string(&event).map_err(|e| e.to_string())?
        );
    }
    Ok(true)
}

/// `last-id`: prints the highest event id of a task, or 0.
fn cmd_last_id(args: &Args) -> Result<bool, String> {
    let conn = open_db(args)?;
    let events = fetch_events(&conn, args.req("task")?, 0)?;
    println!("{}", events.last().map_or(0, |e| e.id));
    Ok(true)
}

/// Prints each phase line and reports whether all passed.
fn emit(phases: &[Phase]) -> bool {
    for p in phases {
        println!("{}", p.line());
    }
    e2e_budget::verdict(phases)
}

/// `start`: phases of a task start.
fn cmd_start(args: &Args) -> Result<bool, String> {
    let d = StartBudgets::default();
    let b = StartBudgets {
        box_ready_ms: args.secs_ms("box-ready", d.box_ready_ms)?,
        proxy_ms: args.secs_ms("proxy", d.proxy_ms)?,
        session_ms: args.secs_ms("session", d.session_ms)?,
        total_ms: args.secs_ms("total", d.total_ms)?,
    };
    Ok(emit(&start_phases(&read_events(args)?, &b)))
}

/// `wake`: phases of a sleep/wake cycle.
fn cmd_wake(args: &Args) -> Result<bool, String> {
    let d = WakeBudgets::default();
    let b = WakeBudgets {
        woke_ms: args.secs_ms("woke", d.woke_ms)?,
        relaunch_ms: args.secs_ms("relaunch", d.relaunch_ms)?,
        total_ms: args.secs_ms("total", d.total_ms)?,
    };
    let cycle = u32::try_from(args.int("cycle")?).map_err(|_| "--cycle out of range".to_owned())?;
    let events = read_events(args)?;
    Ok(emit(&wake_phases(&events, args.int("cmd-ms")?, cycle, &b)))
}

/// `timed`: one driver-measured wall-clock phase.
fn cmd_timed(args: &Args) -> Result<bool, String> {
    let budget = args.secs_ms("budget", -1)?;
    if budget < 0 {
        return Err("missing --budget".to_owned());
    }
    let phase = timed(args.req("name")?, args.int("took-ms")?, budget);
    Ok(emit(&[phase]))
}

/// `count`: number of events of one kind.
fn cmd_count(args: &Args) -> Result<bool, String> {
    println!("{}", count_kind(&read_events(args)?, args.req("kind")?));
    Ok(true)
}

/// Splits a phase line into its name and pass flag, if it is one.
fn parse_phase_line(line: &str) -> Option<(&str, bool)> {
    let mut tokens = line.split_whitespace();
    if tokens.next()? != "PHASE" {
        return None;
    }
    let name = tokens.next()?;
    let pass = tokens.find_map(|t| match t {
        "PASS" => Some(true),
        "FAIL" => Some(false),
        _ => None,
    })?;
    Some((name, pass))
}

/// `verdict`: summarizes previously printed phase lines.
fn cmd_verdict(args: &Args) -> Result<bool, String> {
    let path = args.req("lines")?;
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    let parsed: Vec<(&str, bool)> = text.lines().filter_map(parse_phase_line).collect();
    let failed: Vec<&str> = parsed.iter().filter(|p| !p.1).map(|p| p.0).collect();
    if parsed.is_empty() {
        println!("VERDICT FAIL (no phases)");
    } else if failed.is_empty() {
        println!("VERDICT PASS ({} phases)", parsed.len());
    } else {
        let (k, n) = (failed.len(), parsed.len());
        println!(
            "VERDICT FAIL ({k} of {n} phases failed: {})",
            failed.join(", ")
        );
    }
    Ok(!parsed.is_empty() && failed.is_empty())
}

/// Dispatches a subcommand; `Ok(true)` means all phases passed.
fn run(raw: &[String]) -> Result<bool, String> {
    let (cmd, rest) = raw
        .split_first()
        .ok_or("usage: e2e-budget <subcommand> [--flag value]...")?;
    let args = Args::parse(rest)?;
    match cmd.as_str() {
        "events" => cmd_events(&args),
        "last-id" => cmd_last_id(&args),
        "start" => cmd_start(&args),
        "wake" => cmd_wake(&args),
        "timed" => cmd_timed(&args),
        "count" => cmd_count(&args),
        "verdict" => cmd_verdict(&args),
        other => Err(format!("unknown subcommand `{other}`")),
    }
}

/// Entry point: exit 0 all pass, 1 any fail, 2 usage or IO error.
fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    match run(&raw) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(msg) => {
            eprintln!("e2e-budget: {msg}");
            ExitCode::from(2)
        }
    }
}
