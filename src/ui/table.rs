use std::io;
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor};

static DIM_MODE: AtomicBool = AtomicBool::new(false);

pub(crate) fn set_dim_mode(dim: bool) {
    DIM_MODE.store(dim, Ordering::Relaxed);
}

pub(crate) fn is_dim_mode() -> bool {
    DIM_MODE.load(Ordering::Relaxed)
}

pub(crate) struct HelpSegment {
    pub(crate) text: String,
    pub(crate) color: Option<Color>,
    pub(crate) background: Option<Color>,
}

impl HelpSegment {
    pub(crate) fn plain(text: &str) -> Self {
        Self {
            text: text.to_string(),
            color: None,
            background: None,
        }
    }

    pub(crate) fn key(text: &str) -> Self {
        Self {
            text: text.to_string(),
            color: Some(Color::Cyan),
            background: None,
        }
    }

    pub(crate) fn highlight(text: &str) -> Self {
        Self {
            text: text.to_string(),
            color: Some(Color::Black),
            background: Some(Color::Yellow),
        }
    }
}

pub(crate) fn render_line_at(
    stdout: &mut io::Stdout,
    x: u16,
    y: u16,
    text: &str,
    width: usize,
) -> io::Result<()> {
    let line = fit_left(text, width);
    queue!(stdout, MoveTo(x, y))?;

    let mut buffer = String::new();
    let mut current_table = None;

    for ch in line.chars() {
        let is_table = is_table_char(ch);
        if current_table == Some(is_table) {
            buffer.push(ch);
            continue;
        }

        if !buffer.is_empty() {
            flush_table_segment(stdout, &buffer, current_table.unwrap_or(false))?;
            buffer.clear();
        }

        buffer.push(ch);
        current_table = Some(is_table);
    }

    if !buffer.is_empty() {
        flush_table_segment(stdout, &buffer, current_table.unwrap_or(false))?;
    }

    queue!(stdout, ResetColor)?;
    Ok(())
}

pub(crate) fn render_title_at(
    stdout: &mut io::Stdout,
    x: u16,
    y: u16,
    width: usize,
    title: &str,
) -> io::Result<()> {
    let text = title.to_uppercase();
    let inner = width.saturating_sub(2);
    let line = if inner == 0 {
        text
    } else {
        let padded = center_text(&text, inner);
        format!("┌{}┐", padded)
    };
    render_line_at(stdout, x, y, &line, width)?;
    let underline = if inner == 0 {
        "─".repeat(width)
    } else {
        format!("└{}┘", "─".repeat(inner))
    };
    render_line_at(stdout, x, y + 1, &underline, width)?;
    Ok(())
}

pub(crate) fn render_search_box_at(
    stdout: &mut io::Stdout,
    x: u16,
    y: u16,
    width: usize,
    term: &str,
) -> io::Result<()> {
    if width < 2 {
        let label = if term.is_empty() {
            "Search: <none>".to_string()
        } else {
            format!("Search: {term}")
        };
        return render_line_at(stdout, x, y, &label, width);
    }

    let inner = width.saturating_sub(2);
    if inner == 0 {
        return Ok(());
    }

    let top = format!("┌{}┐", "─".repeat(inner));
    render_line_at(stdout, x, y, &top, width)?;

    let display_term = if term.is_empty() { "<none>" } else { term };
    let prefix = "Search: ";

    let prefix_display = truncate_str(prefix, inner);
    let prefix_len = prefix_display.chars().count();
    let remaining = inner.saturating_sub(prefix_len);
    let term_display = truncate_str(display_term, remaining);
    let term_len = term_display.chars().count();
    let padding = remaining.saturating_sub(term_len);

    queue!(stdout, MoveTo(x, y + 1))?;
    print_table_bar(stdout)?;
    if is_dim_mode() {
        queue!(
            stdout,
            SetForegroundColor(Color::DarkGrey),
            Print(prefix_display),
            ResetColor
        )?;
    } else {
        queue!(stdout, Print(prefix_display))?;
    }
    if term.is_empty() {
        if is_dim_mode() {
            queue!(
                stdout,
                SetForegroundColor(Color::DarkGrey),
                Print(term_display),
                ResetColor
            )?;
        } else {
            queue!(stdout, Print(term_display))?;
        }
    } else {
        queue!(
            stdout,
            SetBackgroundColor(Color::Yellow),
            SetForegroundColor(Color::Black),
            Print(term_display),
            ResetColor
        )?;
    }
    if padding > 0 {
        queue!(stdout, Print(" ".repeat(padding)))?;
    }
    print_table_bar(stdout)?;

    let bottom = format!("└{}┘", "─".repeat(inner));
    render_line_at(stdout, x, y + 2, &bottom, width)?;
    Ok(())
}

pub(crate) fn render_help_table_rows_colored_at(
    stdout: &mut io::Stdout,
    x: u16,
    y: u16,
    width: usize,
    rows: &[Vec<HelpSegment>],
) -> io::Result<()> {
    let inner = width.saturating_sub(2);
    let dim = is_dim_mode();
    let top = format!("┌{}┐", "─".repeat(inner));
    render_line_at(stdout, x, y, &top, width)?;
    for (idx, row) in rows.iter().enumerate() {
        let line_y = y + 1 + idx as u16;
        queue!(stdout, MoveTo(x, line_y))?;
        print_table_bar(stdout)?;
        let mut remaining = inner;
        for segment in row {
            if remaining == 0 {
                break;
            }
            let text = take_prefix(&segment.text, remaining);
            let len = text.chars().count();
            if dim {
                queue!(
                    stdout,
                    SetForegroundColor(Color::DarkGrey),
                    Print(text),
                    ResetColor
                )?;
            } else {
                if let Some(bg) = segment.background {
                    queue!(stdout, SetBackgroundColor(bg))?;
                }
                if let Some(color) = segment.color {
                    queue!(stdout, SetForegroundColor(color))?;
                }
                queue!(stdout, Print(text), ResetColor)?;
            }
            remaining = remaining.saturating_sub(len);
        }
        if remaining > 0 {
            queue!(stdout, Print(" ".repeat(remaining)))?;
        }
        print_table_bar(stdout)?;
    }
    let bottom = format!("└{}┘", "─".repeat(inner));
    render_line_at(stdout, x, y + 1 + rows.len() as u16, &bottom, width)?;
    Ok(())
}

pub(crate) fn clear_list_area_at(
    stdout: &mut io::Stdout,
    x: u16,
    start: usize,
    count: usize,
    width: usize,
) -> io::Result<()> {
    for idx in 0..count {
        render_line_at(stdout, x, (start + idx) as u16, "", width)?;
    }
    Ok(())
}

pub(crate) fn fit_left(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let truncated = truncate_str(text, width);
    let len = truncated.chars().count();
    if len >= width {
        truncated
    } else {
        format!("{truncated}{:width$}", "", width = width - len)
    }
}

pub(crate) fn fit_right(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let truncated = truncate_str(text, width);
    let len = truncated.chars().count();
    if len >= width {
        truncated
    } else {
        format!("{:width$}{truncated}", "", width = width - len)
    }
}

pub(crate) fn truncate_str(input: &str, max_len: usize) -> String {
    if max_len == 0 {
        return String::new();
    }

    let mut chars: Vec<char> = input.chars().collect();
    if chars.len() <= max_len {
        return input.to_string();
    }

    if max_len <= 3 {
        chars.truncate(max_len);
        return chars.into_iter().collect();
    }

    chars.truncate(max_len - 3);
    let mut result: String = chars.into_iter().collect();
    result.push_str("...");
    result
}

pub(crate) fn center_text(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len >= width {
        return truncate_str(text, width);
    }
    let left = (width - len) / 2;
    let right = width - len - left;
    format!("{}{}{}", " ".repeat(left), text, " ".repeat(right))
}

pub(crate) fn format_separator(widths: &[usize]) -> String {
    let mut line = String::new();
    line.push('├');
    for (idx, width) in widths.iter().enumerate() {
        line.push_str(&"─".repeat(*width));
        if idx + 1 == widths.len() {
            line.push('┤');
        } else {
            line.push('┼');
        }
    }
    line
}

pub(crate) fn format_top_border(widths: &[usize]) -> String {
    let mut line = String::new();
    line.push('┌');
    for (idx, width) in widths.iter().enumerate() {
        line.push_str(&"─".repeat(*width));
        if idx + 1 == widths.len() {
            line.push('┐');
        } else {
            line.push('┬');
        }
    }
    line
}

pub(crate) fn split_at_chars(input: &str, count: usize) -> (String, String) {
    let mut iter = input.chars();
    let left: String = iter.by_ref().take(count).collect();
    let right: String = iter.collect();
    (left, right)
}

pub(crate) fn print_table_bar(stdout: &mut io::Stdout) -> io::Result<()> {
    queue!(
        stdout,
        SetForegroundColor(Color::DarkGrey),
        Print("│"),
        ResetColor
    )?;
    Ok(())
}

fn take_prefix(input: &str, max_len: usize) -> String {
    input.chars().take(max_len).collect()
}

fn is_table_char(ch: char) -> bool {
    matches!(
        ch,
        '│' | '─' | '┌' | '┐' | '└' | '┘' | '├' | '┤' | '┬' | '┴' | '┼' | '┏' | '┓' | '┗'
            | '┛' | '━' | '┳' | '┻'
    )
}

fn flush_table_segment(
    stdout: &mut io::Stdout,
    text: &str,
    is_table: bool,
) -> io::Result<()> {
    if is_table {
        queue!(stdout, SetForegroundColor(Color::DarkGrey), Print(text), ResetColor)?;
    } else {
        if is_dim_mode() {
            queue!(
                stdout,
                SetForegroundColor(Color::DarkGrey),
                Print(text),
                ResetColor
            )?;
        } else {
            queue!(stdout, Print(text))?;
        }
    }
    Ok(())
}
