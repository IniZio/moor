//! The panel on a real terminal, with mouse capture on so herdr forwards clicks to the pane.

use std::io;
use std::time::Duration;

use moor_client::{Client, WallClock};
use moor_core::seam::clock::Clock;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseButton, MouseEventKind,
};
use ratatui::crossterm::execute;

use super::{view, Input, Ops, Panel};
use crate::boxes::{self, open_boxes, Shown};
use crate::error::Error;
use crate::herdr::{Cli, Herdr};
use crate::picker::Bench;
use crate::sync::reconcile;

/// How often the panel asks the controller for the Boxes.
const REFRESH: Duration = Duration::from_secs(2);

/// The live controller and herdr.
pub struct Live<'a> {
    /// The controller.
    pub client: Client,
    /// herdr's CLI.
    pub herdr: Cli,
    /// The workspace the panel was opened in, where the picker makes local work.
    pub workspace: String,
    /// The shell command a new Box workspace's pane runs.
    pub attach: &'a dyn Fn(&str) -> String,
}

impl Ops for Live<'_> {
    fn open(&mut self) -> Result<Vec<Shown>, Error> {
        open_boxes(&self.client)
    }

    fn attach(&mut self, name: &str) -> Result<(), Error> {
        let open = open_boxes(&self.client)?;
        let label = open
            .iter()
            .find(|b| b.name == name)
            .map(Shown::label)
            .ok_or_else(|| Error::Refused(format!("{name} is not open")))?;
        let find = |herdr: &mut Cli| -> Result<Option<String>, Error> {
            Ok(herdr
                .spaces()?
                .into_iter()
                .find(|s| s.label == label)
                .map(|s| s.id))
        };
        let space = if let Some(id) = find(&mut self.herdr)? {
            id
        } else {
            reconcile(&mut self.herdr, &open, self.attach)?;
            find(&mut self.herdr)?
                .ok_or_else(|| Error::Refused(format!("herdr shows no workspace {label}")))?
        };
        self.herdr.focus(&space)
    }

    fn wake(&mut self, name: &str) -> Result<(), Error> {
        boxes::wake(&self.client, name)
    }

    fn close(&mut self, name: &str) -> Result<(), Error> {
        let label = open_boxes(&self.client)?
            .iter()
            .find(|b| b.name == name)
            .map(Shown::label);
        boxes::close(&self.client, name)?;
        // A close from the panel is the user dismissing the card as well.
        let Some(label) = label else { return Ok(()) };
        for space in self
            .herdr
            .spaces()?
            .into_iter()
            .filter(|s| s.label == label)
        {
            self.herdr.close_space(&space.id)?;
        }
        Ok(())
    }

    fn new_box(&mut self) -> Result<(), Error> {
        self.herdr.open_picker(&self.workspace)
    }
}

/// The panel's input for a terminal event, if it means anything to it.
fn input(ev: &Event) -> Option<Input> {
    match ev {
        Event::Mouse(m) if m.kind == MouseEventKind::Down(MouseButton::Left) => {
            Some(Input::Click {
                col: m.column,
                row: m.row,
            })
        }
        Event::Key(k) if k.kind == KeyEventKind::Press => match k.code {
            KeyCode::Char('c') if k.modifiers.contains(KeyModifiers::CONTROL) => {
                Some(Input::Key('q'))
            }
            KeyCode::Up | KeyCode::Char('k') => Some(Input::Up),
            KeyCode::Down | KeyCode::Char('j') => Some(Input::Down),
            KeyCode::Enter => Some(Input::Key('\n')),
            KeyCode::Char(c) => Some(Input::Key(c)),
            _ => None,
        },
        _ => None,
    }
}

/// Runs the panel until `q`.
pub fn run(ops: &mut dyn Ops) -> Result<(), Error> {
    let mut terminal = ratatui::init();
    execute!(io::stdout(), EnableMouseCapture)?;
    let done = run_on(&mut terminal, ops);
    let _ = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
    done
}

/// The draw, wait, handle loop.
fn run_on(terminal: &mut ratatui::DefaultTerminal, ops: &mut dyn Ops) -> Result<(), Error> {
    let mut panel = Panel::default();
    panel.refresh(ops);
    let mut refreshed = WallClock.instant();
    while !panel.quit {
        terminal.draw(|f| view::draw(f, &panel, WallClock.instant()))?;
        // A short wait redraws an expired [confirm close] promptly.
        if event::poll(Duration::from_millis(250))? {
            if let Some(i) = input(&event::read()?) {
                panel.handle(i, WallClock.instant(), ops);
            }
        }
        if refreshed.elapsed() >= REFRESH {
            panel.refresh(ops);
            refreshed = WallClock.instant();
        }
    }
    Ok(())
}
