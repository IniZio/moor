//! `moor.new`: asks where the new work goes. A local worktree is herdr's own; a Box is started
//! on the controller and shown as a workspace of its own, ready to attach.

use std::io::{self, BufRead, Write};

use moor_client::Client;
use moor_core::request::Request;
use serde_json::json;

use crate::boxes::{open_boxes, request, Shown};
use crate::error::Error;
use crate::herdr::Herdr;
use crate::sync::reconcile;

/// A person at a terminal: questions out, one line back.
pub trait Prompt {
    /// Asks `question`; `None` once the input ends.
    fn ask(&mut self, question: &str) -> Result<Option<String>, Error>;
    /// Shows one line.
    fn say(&mut self, line: &str) -> Result<(), Error>;
}

/// The controller calls the picker makes.
pub trait Controller {
    /// Every open Box.
    fn open(&self) -> Result<Vec<Shown>, Error>;
    /// Starts Box `name` on `repo`, its agent working on `prompt`, or waiting for a prompt without one.
    fn start(&self, name: &str, repo: &str, prompt: Option<&str>) -> Result<(), Error>;
}

/// The herdr calls only the picker makes, besides [`Herdr`]'s.
pub trait Bench: Herdr {
    /// A plain tab in `workspace`, focused.
    fn tab(&mut self, workspace: &str) -> Result<(), Error>;
    /// A git worktree on branch `name` beside `workspace`'s repo, focused.
    fn worktree(&mut self, workspace: &str, name: &str) -> Result<(), Error>;
    /// Brings `workspace` to the front.
    fn focus(&mut self, workspace: &str) -> Result<(), Error>;
}

/// The Terminal on stdin and stdout.
#[derive(Debug, Default)]
pub struct Tty;

impl Prompt for Tty {
    fn ask(&mut self, question: &str) -> Result<Option<String>, Error> {
        let mut out = io::stdout().lock();
        write!(out, "{question}")?;
        out.flush()?;
        let mut line = String::new();
        let read = io::stdin().lock().read_line(&mut line)?;
        Ok((read > 0).then(|| line.trim().to_owned()))
    }

    fn say(&mut self, line: &str) -> Result<(), Error> {
        Ok(writeln!(io::stdout().lock(), "{line}")?)
    }
}

impl Controller for Client {
    fn open(&self) -> Result<Vec<Shown>, Error> {
        open_boxes(self)
    }

    fn start(&self, name: &str, repo: &str, prompt: Option<&str>) -> Result<(), Error> {
        let reply = self.request(start_request(name, repo, prompt)?, |_| {})?;
        reply
            .error
            .map_or(Ok(()), |e| Err(Error::Refused(e.message)))
    }
}

/// The `start` every front sends for a Box on a repo; the controller picks the rest.
pub fn start_request(name: &str, repo: &str, prompt: Option<&str>) -> Result<Request, Error> {
    request(json!({"cmd": "start", "name": name, "repo": repo, "prompt": prompt}))
}

/// Asks until `parse` accepts an answer; `None` when the input ends.
fn ask_until<T>(
    prompt: &mut dyn Prompt,
    question: &str,
    mut parse: impl FnMut(&str) -> Result<T, String>,
) -> Result<Option<T>, Error> {
    while let Some(answer) = prompt.ask(question)? {
        match parse(&answer) {
            Ok(v) => return Ok(Some(v)),
            Err(why) => prompt.say(&why)?,
        }
    }
    Ok(None)
}

/// Runs the picker for the workspace `workspace` the key was pressed in.
pub fn run(
    prompt: &mut dyn Prompt,
    ctl: &dyn Controller,
    bench: &mut dyn Bench,
    workspace: &str,
    attach: &dyn Fn(&str) -> String,
) -> Result<(), Error> {
    let boxes = ctl.open()?;
    let kind = ask_until(
        prompt,
        "New: local worktree or Box? [local/box] ",
        |a| match a.to_ascii_lowercase().as_str() {
            "1" | "l" | "local" => Ok(true),
            "2" | "b" | "box" => Ok(false),
            _ => Err("Answer local or box.".to_owned()),
        },
    )?;
    match kind {
        Some(true) => local(prompt, &boxes, bench, workspace),
        Some(false) => start_box(prompt, &boxes, (ctl, bench), attach),
        None => Ok(()),
    }
}

/// A plain tab for an empty name, else a worktree, unless an open Box has the name.
fn local(
    prompt: &mut dyn Prompt,
    boxes: &[Shown],
    bench: &mut dyn Bench,
    workspace: &str,
) -> Result<(), Error> {
    let name = ask_until(prompt, "Worktree name (empty for a plain tab): ", |a| {
        boxes.iter().find(|b| b.name == a).map_or_else(
            || Ok(a.to_owned()),
            |b| {
                Err(format!(
                    "{a} is the name of the open Box {}; a worktree with it would look like that Box. Pick another name.",
                    b.label()
                ))
            },
        )
    })?;
    match name.as_deref() {
        None => Ok(()),
        Some("") => bench.tab(workspace),
        Some(name) => bench.worktree(workspace, name),
    }
}

/// Picks a repo and a name, starts the Box, syncs and brings its workspace to the front.
fn start_box(
    prompt: &mut dyn Prompt,
    boxes: &[Shown],
    (ctl, bench): (&dyn Controller, &mut dyn Bench),
    attach: &dyn Fn(&str) -> String,
) -> Result<(), Error> {
    let mut repos: Vec<&str> = boxes.iter().filter_map(|b| b.repo.as_deref()).collect();
    repos.sort_unstable();
    repos.dedup();
    for (i, repo) in repos.iter().enumerate() {
        prompt.say(&format!("{}) {repo}", i + 1))?;
    }
    let repo = ask_until(prompt, "Repo (number or owner/name): ", |a| {
        let listed = a
            .parse::<usize>()
            .ok()
            .and_then(|n| repos.get(n.checked_sub(1)?));
        match (listed, a.split_once('/')) {
            (Some(repo), _) => Ok((*repo).to_owned()),
            (None, Some((owner, name))) if !owner.is_empty() && !name.is_empty() => {
                Ok(a.to_owned())
            }
            _ => Err("Give a number from the list or owner/name.".to_owned()),
        }
    })?;
    let Some(repo) = repo else { return Ok(()) };
    let name = ask_until(prompt, "Box name: ", |a| {
        if a.is_empty() || a.contains(char::is_whitespace) {
            Err("A Box name is one word.".to_owned())
        } else {
            Ok(a.to_owned())
        }
    })?;
    let Some(name) = name else { return Ok(()) };
    let Some(first) = prompt.ask("First prompt (empty to type it once attached): ")? else {
        return Ok(());
    };
    let first = Some(first.trim()).filter(|g| !g.is_empty());
    prompt.say(&format!("Starting {name} on {repo}..."))?;
    ctl.start(&name, &repo, first)?;
    reconcile(bench, &ctl.open()?, attach)?;
    let label = crate::boxes::label(Some(&repo), &name);
    if let Some(space) = bench.spaces()?.into_iter().find(|s| s.label == label) {
        bench.focus(&space.id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
