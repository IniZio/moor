//! Draws the panel's lines, styling buttons so they read as clickable.

use std::time::Instant;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{Button, Panel, Piece};

/// How `piece` is drawn.
const fn style(piece: &Piece) -> Style {
    match &piece.button {
        None => Style::new(),
        Some(Button::Select(_)) => Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD),
        Some(Button::Close(_)) => Style::new().fg(Color::Red),
        Some(_) => Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
    }
}

/// Draws `panel` at `now` over the whole frame.
pub fn draw(frame: &mut Frame<'_>, panel: &Panel, now: Instant) {
    let lines: Vec<Line<'_>> = panel
        .lines(now)
        .into_iter()
        .map(|pieces| {
            Line::from(
                pieces
                    .into_iter()
                    .map(|p| Span::styled(p.text.clone(), style(&p)))
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), frame.area());
}
