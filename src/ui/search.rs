use std::io;

use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::{
    Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
};

use super::table::fit_left;

pub(crate) fn render_search_bar_at(
    stdout: &mut io::Stdout,
    x: u16,
    y: u16,
    width: usize,
    term: &str,
) -> io::Result<()> {
    let suffix = " | Enter/Esc exit";
    let label = if term.is_empty() {
        " Search:".to_string()
    } else {
        format!(" Search: {term}")
    };
    let line = fit_left(&format!("{label}{suffix}"), width);

    queue!(
        stdout,
        MoveTo(x, y),
        SetBackgroundColor(Color::Yellow),
        SetForegroundColor(Color::Black),
        SetAttribute(Attribute::Bold),
        Print(line),
        SetAttribute(Attribute::Reset),
        ResetColor
    )?;

    Ok(())
}
