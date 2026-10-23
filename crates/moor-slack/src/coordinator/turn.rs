//! One coordinator turn: the agent's process with the context on stdin, the check of what it
//! loaded, and the time limits. Every agent runs through here; none holds a credential.

use std::time::Duration;

use super::process::{now, supervise, Left, Lines, Next};
use super::{Agent, Answer, Event, Loaded, TurnSetup};

/// How long a turn may run.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// Longest wait for the first output line, the report of what the agent loaded.
    pub(crate) start: Duration,
    /// Longest the whole turn runs before its process group is killed.
    pub(crate) turn: Duration,
}

/// The limits of every real turn.
pub(crate) const LIMITS: Limits = Limits {
    start: Duration::from_secs(30),
    turn: Duration::from_secs(600),
};

/// How a turn ended.
#[derive(Debug, PartialEq)]
pub(crate) enum Ended {
    /// The agent ended it and said how.
    Answered(Answer),
    /// The agent loaded more than the front gave it, or did not say what it loaded in time;
    /// the turn was killed. The text says why, for the log.
    Unsafe(String),
    /// The agent loaded a plugin that is not built in; the turn was killed. Holds its name.
    Plugin(String),
    /// The turn ran past [`Limits::turn`] and was killed.
    Killed,
}

/// What the watcher saw before the process ended.
enum Seen {
    /// The turn ended as this says.
    Ended(Ended),
    /// The output ended with no answer.
    Silent,
}

/// What one turn runs with.
pub(crate) struct Turn<'a> {
    /// The command's settings.
    pub(crate) setup: TurnSetup<'a>,
    /// The context, written to stdin.
    pub(crate) input: &'a str,
    /// How long the turn may run.
    pub(crate) limits: Limits,
}

/// Runs one turn of `agent`. `spawned` gets the process id, which is also its process group
/// id. `log` gets the plugins the agent loaded.
pub(crate) fn run(
    agent: &dyn Agent,
    turn: &Turn<'_>,
    spawned: &mut dyn FnMut(u32),
    log: &dyn Fn(&str),
) -> Ended {
    let cmd = match agent.turn_command(&turn.setup) {
        Ok(cmd) => cmd,
        Err(why) => return Ended::Answered(Answer::Unavailable(why)),
    };
    let server = turn.setup.tools;
    let allowed = Allowed {
        server: server.name,
        tools: server
            .tools
            .iter()
            .map(|tool| agent.tool_name(server.name, tool))
            .collect(),
    };
    let limits = turn.limits;
    let ran = supervise(cmd, turn.input.as_bytes(), None, |lines| {
        spawned(lines.pid);
        watch(agent, lines, &allowed, limits, log)
    });
    match ran {
        Ok((Seen::Ended(ended), _)) => ended,
        Ok((Seen::Silent, stderr)) => {
            let stderr = String::from_utf8_lossy(&stderr);
            let last = stderr.lines().rev().map(str::trim).find(|l| !l.is_empty());
            let why = last.unwrap_or("the agent printed no answer").to_owned();
            Ended::Answered(Answer::Unavailable(why))
        }
        Err(e) => Ended::Answered(Answer::Unavailable(e.to_string())),
    }
}

/// What the front gives the coordinator: one tool server and its tools, named as the agent
/// reports them.
struct Allowed<'a> {
    /// The tool server's name.
    server: &'a str,
    /// Its tools.
    tools: Vec<String>,
}

/// Reads the turn's output: the first line must report what the agent loaded, then the
/// answer ends the turn. An error may come first, since an agent that cannot start reports
/// nothing it loaded; a reply may not.
fn watch(
    agent: &dyn Agent,
    lines: &Lines,
    allowed: &Allowed<'_>,
    limits: Limits,
    log: &dyn Fn(&str),
) -> (Seen, Left) {
    let started = now();
    let start_by = started + limits.start.min(limits.turn);
    let end_by = started + limits.turn;
    let unsafe_turn = |why: String| (Seen::Ended(Ended::Unsafe(why)), Left::Stopped);
    match lines.next(start_by) {
        Next::Line(line) => match agent.event(&line) {
            Event::Loaded(loaded) => {
                if let Some(why) = refused(&loaded, allowed) {
                    return unsafe_turn(why);
                }
                if let Some(plugin) = loaded.plugins.iter().find(|p| !p.builtin) {
                    log(&format!("turn refused the plugin {}", plugin.name));
                    let ended = Ended::Plugin(plugin.name.clone());
                    return (Seen::Ended(ended), Left::Stopped);
                }
                if !loaded.plugins.is_empty() {
                    let names: Vec<&str> = loaded.plugins.iter().map(|p| p.name.as_str()).collect();
                    log(&format!("turn loaded plugins: {}", names.join(", ")));
                }
            }
            Event::Answer(Answer::Reply { .. }) => {
                return unsafe_turn("a reply came before the load report".into())
            }
            Event::Answer(answer) => return (Seen::Ended(Ended::Answered(answer)), Left::Ended),
            Event::Other => {
                return unsafe_turn("the first output line is not the load report".into())
            }
        },
        Next::End => return (Seen::Silent, Left::Ended),
        Next::Late if start_by < end_by => {
            let secs = limits.start.as_secs();
            return unsafe_turn(format!("no load report within {secs} s"));
        }
        Next::Late => return (Seen::Ended(Ended::Killed), Left::Stopped),
    }
    loop {
        match lines.next(end_by) {
            Next::Line(line) => {
                if let Event::Answer(answer) = agent.event(&line) {
                    return (Seen::Ended(Ended::Answered(answer)), Left::Ended);
                }
            }
            Next::End => return (Seen::Silent, Left::Ended),
            Next::Late => return (Seen::Ended(Ended::Killed), Left::Stopped),
        }
    }
}

/// Why a turn that loaded `loaded` must not run: a tool or tool server beyond `allowed`, or
/// any skill or slash command, since the front gives the coordinator none.
fn refused(loaded: &Loaded, allowed: &Allowed<'_>) -> Option<String> {
    let tools = loaded.tools.iter().filter(|t| !allowed.tools.contains(t));
    let servers = loaded.servers.iter().filter(|s| *s != allowed.server);
    let extra = [
        ("tool", tools.collect::<Vec<_>>()),
        ("tool server", servers.collect()),
        ("skill", loaded.skills.iter().collect()),
        ("slash command", loaded.commands.iter().collect()),
    ];
    extra.into_iter().find_map(|(what, names)| {
        names
            .first()
            .map(|name| format!("the agent loaded the {what} {name}"))
    })
}

#[cfg(test)]
mod tests;
