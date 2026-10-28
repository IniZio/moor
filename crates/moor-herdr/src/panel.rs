//! `moor-herdr panel`: a clickable list of the open Boxes in a herdr pane.
//!
//! herdr lets plugins add nothing to its own menus and buttons, so the panel is where a mouse
//! user attaches, wakes, closes and starts Boxes.

use std::time::{Duration, Instant};

use crate::boxes::{Activity, Phase, Shown};
use crate::error::Error;

pub mod term;
pub mod view;

/// How long an armed `[confirm close]` waits for its second click.
pub const CONFIRM_WINDOW: Duration = Duration::from_secs(4);

/// How soon after arming a second press still counts as the first click: a double click, or
/// presses herdr hands the pane at once, must not close a Box.
pub const CONFIRM_MIN: Duration = Duration::from_millis(300);

/// The row the first Box is drawn on; the toolbar and a status line sit above it.
pub const FIRST_BOX_ROW: u16 = 2;

/// Something a click or a key asks the panel to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Button {
    /// Open the picker popup.
    New,
    /// Ask the controller for the Boxes now.
    Refresh,
    /// Select the Box on this row.
    Select(usize),
    /// Bring Box `name`'s workspace to the front.
    Attach(String),
    /// Wake the asleep Box `name`.
    Wake(String),
    /// Close Box `name`; the first press only arms it.
    Close(String),
}

/// The calls the panel makes on the controller and herdr.
pub trait Ops {
    /// Every open Box.
    fn open(&mut self) -> Result<Vec<Shown>, Error>;
    /// Focuses Box `name`'s workspace, making it first when herdr has none.
    fn attach(&mut self, name: &str) -> Result<(), Error>;
    /// Wakes Box `name`.
    fn wake(&mut self, name: &str) -> Result<(), Error>;
    /// Closes Box `name` on the controller.
    fn close(&mut self, name: &str) -> Result<(), Error>;
    /// Opens the picker popup.
    fn new_box(&mut self) -> Result<(), Error>;
}

/// A close waiting for its second press.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Confirm {
    /// The Box armed, and when.
    armed: Option<(String, Instant)>,
}

impl Confirm {
    /// Presses close on `name`: `true` when this press confirms an armed close of the same Box.
    pub fn press(&mut self, name: &str, now: Instant) -> bool {
        let too_soon = self.armed.as_ref().is_some_and(|(armed, at)| {
            armed == name && now.saturating_duration_since(*at) < CONFIRM_MIN
        });
        if too_soon {
            return false;
        }
        let confirmed = self.armed(now) == Some(name);
        self.armed = (!confirmed).then(|| (name.to_owned(), now));
        confirmed
    }

    /// The Box armed at `now`, if its window is still open.
    #[must_use]
    pub fn armed(&self, now: Instant) -> Option<&str> {
        self.armed
            .as_ref()
            .filter(|(_, at)| now.saturating_duration_since(*at) < CONFIRM_WINDOW)
            .map(|(name, _)| name.as_str())
    }
}

/// What a person did, in the panel's own terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Input {
    /// A left click at a pane-relative cell.
    Click {
        /// The column, from 0.
        col: u16,
        /// The row, from 0.
        row: u16,
    },
    /// Select the Box above.
    Up,
    /// Select the Box below.
    Down,
    /// A key that names a [`Button`] for the selected Box, or `q`.
    Key(char),
}

/// One run of text on a line, maybe a button.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Piece {
    /// The text drawn.
    pub text: String,
    /// What a click on it does.
    pub button: Option<Button>,
}

impl Piece {
    /// Plain text.
    fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            button: None,
        }
    }

    /// A button labelled `[text]`.
    fn button(text: &str, button: Button) -> Self {
        Self {
            text: format!("[{text}]"),
            button: Some(button),
        }
    }

    /// The cells it takes on screen.
    #[must_use]
    pub fn width(&self) -> u16 {
        u16::try_from(self.text.chars().count()).unwrap_or(u16::MAX)
    }
}

/// The words for a Box's phase, after its icon.
#[must_use]
pub const fn phase_words(phase: Phase) -> &'static str {
    match phase {
        Phase::Starting => "starting",
        Phase::Live(Activity::Working) => "working",
        Phase::Live(Activity::Waiting) => "waiting for you",
        Phase::Live(Activity::Idle) => "idle",
        Phase::WaitingForMachine => "waiting for a machine",
        Phase::Asleep => "asleep",
        Phase::Closed => "closed",
    }
}

/// The panel: the Boxes last seen, the selection, an armed close and the last outcome.
#[derive(Debug, Default)]
pub struct Panel {
    /// The open Boxes, as the last refresh saw them.
    pub boxes: Vec<Shown>,
    /// The selected Box's index.
    pub selected: usize,
    /// A close waiting for its second press.
    pub confirm: Confirm,
    /// What the last action did, or why it failed.
    pub note: String,
    /// Set once the person asked to leave.
    pub quit: bool,
}

impl Panel {
    /// Reloads the Boxes, keeping the selection on the same Box where it can.
    pub fn refresh(&mut self, ops: &mut dyn Ops) {
        let was = self.boxes.get(self.selected).map(|b| b.name.clone());
        match ops.open() {
            Ok(boxes) => self.boxes = boxes,
            Err(e) => self.note = e.to_string(),
        }
        self.selected = was
            .and_then(|n| self.boxes.iter().position(|b| b.name == n))
            .unwrap_or(self.selected)
            .min(self.boxes.len().saturating_sub(1));
    }

    /// The lines drawn, top to bottom; a click lands on the piece under it.
    #[must_use]
    pub fn lines(&self, now: Instant) -> Vec<Vec<Piece>> {
        let mut lines = vec![
            vec![
                Piece::button("+ New Box", Button::New),
                Piece::text(" "),
                Piece::button("refresh", Button::Refresh),
            ],
            vec![Piece::text(self.confirm.armed(now).map_or_else(
                || self.note.clone(),
                |name| format!("Close {name}? Press [confirm close] again."),
            ))],
        ];
        if self.boxes.is_empty() {
            lines.push(vec![Piece::text("No open Boxes.")]);
        }
        let armed = self.confirm.armed(now);
        for (i, b) in self.boxes.iter().enumerate() {
            let mark = if i == self.selected { "›" } else { " " };
            lines.push(vec![Piece {
                text: format!("{mark} {}", b.label()),
                button: Some(Button::Select(i)),
            }]);
            let icon = match b.phase.icon() {
                "" => "●",
                icon => icon,
            };
            let mut row = vec![
                Piece::text(format!("    {icon} {}  ", phase_words(b.phase))),
                Piece::button("attach", Button::Attach(b.name.clone())),
            ];
            if b.phase == Phase::Asleep {
                row.extend([
                    Piece::text(" "),
                    Piece::button("wake", Button::Wake(b.name.clone())),
                ]);
            }
            let close = if armed == Some(b.name.as_str()) {
                "confirm close"
            } else {
                "close"
            };
            row.extend([
                Piece::text(" "),
                Piece::button(close, Button::Close(b.name.clone())),
            ]);
            lines.push(row);
        }
        lines
    }

    /// The button drawn at (`col`, `row`).
    #[must_use]
    pub fn hit(&self, col: u16, row: u16, now: Instant) -> Option<Button> {
        let lines = self.lines(now);
        let mut x = 0u16;
        lines
            .get(usize::from(row))?
            .iter()
            .find(|p| {
                let start = x;
                x = x.saturating_add(p.width());
                (start..x).contains(&col)
            })
            .and_then(|p| p.button.clone())
    }

    /// The button a key names for the selected Box.
    fn key(&self, key: char) -> Option<Button> {
        let name = || self.boxes.get(self.selected).map(|b| b.name.clone());
        match key {
            'n' => Some(Button::New),
            'r' => Some(Button::Refresh),
            '\n' => name().map(Button::Attach),
            'w' => self
                .boxes
                .get(self.selected)
                .filter(|b| b.phase == Phase::Asleep)
                .map(|b| Button::Wake(b.name.clone())),
            'c' => name().map(Button::Close),
            _ => None,
        }
    }

    /// Handles one input at `now`.
    pub fn handle(&mut self, input: Input, now: Instant, ops: &mut dyn Ops) {
        let button = match input {
            Input::Click { col, row } => self.hit(col, row, now),
            Input::Up => Some(Button::Select(self.selected.saturating_sub(1))),
            Input::Down => Some(Button::Select(self.selected.saturating_add(1))),
            Input::Key('q') => {
                self.quit = true;
                None
            }
            Input::Key(k) => self.key(k),
        };
        if let Some(button) = button {
            self.press(&button, now, ops);
        }
    }

    /// Does what `button` says and notes the outcome.
    fn press(&mut self, button: &Button, now: Instant, ops: &mut dyn Ops) {
        let done = match button {
            Button::Select(i) => {
                self.selected = (*i).min(self.boxes.len().saturating_sub(1));
                return;
            }
            Button::Refresh => {
                self.note.clear();
                self.refresh(ops);
                return;
            }
            Button::New => ops.new_box().map(|()| String::new()),
            Button::Attach(name) => ops.attach(name).map(|()| format!("Attached {name}.")),
            Button::Wake(name) => ops.wake(name).map(|()| format!("Waking {name}...")),
            Button::Close(name) if !self.confirm.press(name, now) => {
                self.note.clear();
                return;
            }
            Button::Close(name) => ops.close(name).map(|()| format!("Closed {name}.")),
        };
        self.note = done.unwrap_or_else(|e| e.to_string());
        if matches!(button, Button::Wake(_) | Button::Close(_)) {
            self.refresh(ops);
        }
    }
}

#[cfg(test)]
mod tests;
