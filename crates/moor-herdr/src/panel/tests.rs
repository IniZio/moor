use std::time::{Duration, Instant};

use ratatui::backend::TestBackend;
use ratatui::Terminal;

use super::{view, Button, Confirm, Input, Ops, Panel, CONFIRM_WINDOW, FIRST_BOX_ROW};
use moor_client::WallClock;
use moor_core::seam::clock::Clock;

use crate::boxes::{Activity, Phase, Shown};
use crate::error::Error;

fn clock() -> Instant {
    WallClock.instant()
}

fn shown(name: &str, repo: &str, phase: Phase) -> Shown {
    Shown {
        name: name.to_owned(),
        repo: Some(repo.to_owned()),
        branch: None,
        phase,
    }
}

#[derive(Default)]
struct Fake {
    boxes: Vec<Shown>,
    calls: Vec<String>,
}

impl Ops for Fake {
    fn open(&mut self) -> Result<Vec<Shown>, Error> {
        Ok(self.boxes.clone())
    }

    fn attach(&mut self, name: &str) -> Result<(), Error> {
        self.calls.push(format!("attach {name}"));
        Ok(())
    }

    fn wake(&mut self, name: &str) -> Result<(), Error> {
        self.calls.push(format!("wake {name}"));
        Ok(())
    }

    fn close(&mut self, name: &str) -> Result<(), Error> {
        self.calls.push(format!("close {name}"));
        self.boxes.retain(|b| b.name != name);
        Ok(())
    }

    fn new_box(&mut self) -> Result<(), Error> {
        self.calls.push("new".to_owned());
        Ok(())
    }
}

fn fake() -> Fake {
    Fake {
        boxes: vec![
            shown("fix", "acme/app", Phase::Live(Activity::Working)),
            shown("nap", "acme/site", Phase::Asleep),
        ],
        calls: Vec::new(),
    }
}

fn loaded(ops: &mut Fake) -> Panel {
    let mut panel = Panel::default();
    panel.refresh(ops);
    panel
}

fn screen(panel: &Panel, now: Instant) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(76, 5)).unwrap();
    terminal.draw(|f| view::draw(f, panel, now)).unwrap();
    let buffer = terminal.backend().buffer();
    buffer
        .content()
        .chunks(usize::from(buffer.area.width))
        .map(|row| {
            row.iter()
                .map(ratatui::buffer::Cell::symbol)
                .collect::<String>()
        })
        .map(|s| s.trim_end().to_owned())
        .collect()
}

fn column_of(panel: &Panel, row: u16, text: &str, now: Instant) -> u16 {
    let lines = panel.lines(now);
    let line = lines.get(usize::from(row)).unwrap();
    let at = line.iter().position(|p| p.text == text).unwrap();
    line.iter().take(at).map(super::Piece::width).sum()
}

#[test]
fn each_box_is_a_row_with_its_label_state_and_buttons() {
    let now = clock();
    let panel = loaded(&mut fake());
    assert_eq!(
        screen(&panel, now),
        [
            "[+ New Box] [refresh]",
            "",
            "› ◈ app/fix  ● working               [attach] [close]",
            "  ◈ site/nap ◐ asleep                [attach] [wake] [close]",
            "",
        ]
    );
}

#[test]
fn an_armed_close_reads_confirm_close() {
    let now = clock();
    let mut panel = loaded(&mut fake());
    panel.confirm.press("fix", now);
    assert!(screen(&panel, now)
        .get(2)
        .unwrap()
        .ends_with("[attach] [confirm close]"));
}

#[test]
fn a_click_on_the_toolbar_lands_on_new_or_refresh() {
    let now = clock();
    let panel = loaded(&mut fake());
    assert_eq!(panel.hit(0, 0, now), Some(Button::New));
    assert_eq!(panel.hit(10, 0, now), Some(Button::New));
    assert_eq!(panel.hit(11, 0, now), None);
    assert_eq!(panel.hit(12, 0, now), Some(Button::Refresh));
    assert_eq!(panel.hit(0, 1, now), None);
}

#[test]
fn a_click_lands_on_the_button_under_it_and_nowhere_else() {
    let now = clock();
    let panel = loaded(&mut fake());
    let row = FIRST_BOX_ROW + 1;
    let attach = column_of(&panel, row, "[attach]", now);
    let wake = column_of(&panel, row, "[wake]", now);
    assert_eq!(
        panel.hit(attach, row, now),
        Some(Button::Attach("nap".into()))
    );
    assert_eq!(
        panel.hit(attach + 7, row, now),
        Some(Button::Attach("nap".into()))
    );
    assert_eq!(panel.hit(attach + 8, row, now), None);
    assert_eq!(panel.hit(wake, row, now), Some(Button::Wake("nap".into())));
    assert_eq!(panel.hit(1, row, now), Some(Button::Select(1)));
    assert_eq!(panel.hit(0, 9, now), None);
    assert_eq!(panel.hit(200, row, now), None);
}

#[test]
fn a_live_box_has_no_wake_button() {
    let now = clock();
    let panel = loaded(&mut fake());
    let lines = panel.lines(now);
    let line = lines.get(usize::from(FIRST_BOX_ROW)).unwrap();
    assert!(!line
        .iter()
        .any(|p| matches!(p.button, Some(Button::Wake(_)))));
}

#[test]
fn close_confirms_only_on_a_second_press_of_the_same_box_within_the_window() {
    let t0 = clock();
    let mut c = Confirm::default();
    assert!(!c.press("fix", t0));
    assert_eq!(c.armed(t0), Some("fix"));
    assert!(c.press("fix", t0 + Duration::from_secs(1)));
    assert_eq!(c.armed(t0 + Duration::from_secs(1)), None);

    assert!(!c.press("fix", t0));
    assert!(!c.press("nap", t0), "another Box re-arms instead");
    assert_eq!(c.armed(t0), Some("nap"));
}

#[test]
fn an_armed_close_disarms_once_its_window_passes() {
    let t0 = clock();
    let mut late = Confirm::default();
    assert!(!late.press("fix", t0));
    assert_eq!(late.armed(t0 + CONFIRM_WINDOW), None, "the window disarms");
    assert!(
        !late.press("fix", t0 + CONFIRM_WINDOW),
        "a late press arms again"
    );
}

#[test]
fn clicking_close_twice_closes_the_box_and_once_does_not() {
    let now = clock();
    let mut ops = fake();
    let mut panel = loaded(&mut ops);
    let col = column_of(&panel, FIRST_BOX_ROW, "[close]", now);
    let click = Input::Click {
        col,
        row: FIRST_BOX_ROW,
    };
    panel.handle(click, now, &mut ops);
    assert!(ops.calls.is_empty());
    assert!(panel.note.contains("confirm close"));
    panel.handle(click, now + Duration::from_secs(1), &mut ops);
    assert_eq!(ops.calls, ["close fix"]);
    assert_eq!(panel.boxes.len(), 1, "refreshed after closing");
}

#[test]
fn each_button_calls_its_operation() {
    let now = clock();
    let mut ops = fake();
    let mut panel = loaded(&mut ops);
    let row = FIRST_BOX_ROW + 1;
    for text in ["[attach]", "[wake]"] {
        let col = column_of(&panel, row, text, now);
        panel.handle(Input::Click { col, row }, now, &mut ops);
    }
    panel.handle(Input::Click { col: 0, row: 0 }, now, &mut ops);
    assert_eq!(ops.calls, ["attach nap", "wake nap", "new"]);
}

#[test]
fn keys_drive_the_selected_box() {
    let now = clock();
    let mut ops = fake();
    let mut panel = loaded(&mut ops);
    panel.handle(Input::Key('w'), now, &mut ops);
    panel.handle(Input::Down, now, &mut ops);
    panel.handle(Input::Down, now, &mut ops);
    assert_eq!(panel.selected, 1, "the selection stops at the last Box");
    for key in ['\n', 'w', 'n', 'c'] {
        panel.handle(Input::Key(key), now, &mut ops);
    }
    assert_eq!(ops.calls, ["attach nap", "wake nap", "new"]);
    panel.handle(Input::Key('c'), now, &mut ops);
    assert_eq!(ops.calls.last().map(String::as_str), Some("close nap"));
    panel.handle(Input::Up, now, &mut ops);
    assert_eq!(panel.selected, 0);
    panel.handle(Input::Key('q'), now, &mut ops);
    assert!(panel.quit);
}
