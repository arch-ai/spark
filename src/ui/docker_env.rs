use std::io::{self, Write};

use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal;

use crate::app::AppState;

use super::{
    clear_list_area, fit_left, format_separator, format_top_border, render_help_table_rows_colored,
    render_line, render_title, HelpSegment,
};

pub fn render_docker_envs(
    stdout: &mut io::Stdout,
    _state: &AppState,
    envs: &[String],
    container_name: Option<&str>,
    compose_name: &str,
    compose_path: &str,
    port_public: &str,
    port_internal: &str,
    selected: usize,
) -> io::Result<()> {
    let (width, height) = terminal::size().unwrap_or((80, 24));
    let width_usize = width as usize;
    let height_usize = height as usize;

    queue!(stdout, MoveTo(0, 0))?;

    let mut row = 0u16;
    render_line(
        stdout,
        row,
        &format!("┌{}┐", "─".repeat(width_usize.saturating_sub(2))),
        width_usize,
    )?;
    row += 1;
    render_title(stdout, row, width_usize, "DOCKER ENV")?;
    row += 2;

    let compose_text = format!("Compose: {compose_name}");
    let path_text = format!("Path: {compose_path}");
    let container_title = container_name.unwrap_or("-");
    let container_text = format!("Container: {container_title}");
    let ports_text = if port_internal != "-" {
        format!("Ports: {port_public} | Int: {port_internal}")
    } else {
        format!("Ports: {port_public}")
    };

    let info_widths = env_info_widths(
        width_usize,
        &compose_text,
        &container_text,
        &path_text,
        &ports_text,
    );
    let info_top = format_top_border(&info_widths);
    render_line(stdout, row, &info_top, width_usize)?;
    row += 1;
    let info_row1 = format_env_info_row(&info_widths, &compose_text, &path_text);
    render_line(stdout, row, &info_row1, width_usize)?;
    row += 1;
    let info_sep = format_separator(&info_widths);
    render_line(stdout, row, &info_sep, width_usize)?;
    row += 1;
    let info_row2 = format_env_info_row(&info_widths, &container_text, &ports_text);
    render_line(stdout, row, &info_row2, width_usize)?;
    row += 1;
    let info_bottom = format_bottom_border(&info_widths);
    render_line(stdout, row, &info_bottom, width_usize)?;
    row += 1;

    let env_widths = env_column_widths(width_usize, envs);
    let env_top = format_top_border(&env_widths);
    render_line(stdout, row, &env_top, width_usize)?;
    row += 1;
    let env_header = format_env_header(&env_widths);
    render_line(stdout, row, &env_header, width_usize)?;
    row += 1;
    let env_sep = format_separator(&env_widths);
    render_line(stdout, row, &env_sep, width_usize)?;
    row += 1;

    let list_start = row as usize;
    let footer_lines = 5usize;
    let max_rows = height_usize.saturating_sub(list_start + footer_lines);
    if max_rows > 0 {
        let total = envs.len();
        let scroll = if selected >= max_rows {
            selected - max_rows + 1
        } else {
            0
        };
        let end = (scroll + max_rows).min(total);
        let mut rendered = 0usize;
        for (idx, env_line) in envs[scroll..end].iter().enumerate() {
            let line_index = scroll + idx;
            let y = list_start + idx;
            let (key, value) = split_env_line(env_line);
            let line = format_env_line(&env_widths, &key, &value);
            if line_index == selected {
                queue!(
                    stdout,
                    MoveTo(0, y as u16),
                    SetAttribute(Attribute::Reverse),
                    Print(fit_left(&line, width_usize)),
                    SetAttribute(Attribute::Reset)
                )?;
            } else {
                render_line(stdout, y as u16, &line, width_usize)?;
            }
            rendered += 1;
        }
        clear_list_area(
            stdout,
            list_start + rendered,
            max_rows.saturating_sub(rendered),
            width_usize,
        )?;
    }

    if height_usize >= footer_lines {
        let message_line = height_usize.saturating_sub(footer_lines) as u16;
        render_line(stdout, message_line, "Esc to return", width_usize)?;

        let help_rows = vec![vec![
            HelpSegment::plain("Actions: "),
            HelpSegment::key("Esc"),
            HelpSegment::plain(" back"),
        ]];
        let help_start = height_usize.saturating_sub(help_rows.len() + 2) as u16;
        render_help_table_rows_colored(stdout, help_start, width_usize, &help_rows)?;
    }

    stdout.flush()?;
    Ok(())
}

fn split_env_line(line: &str) -> (String, String) {
    if let Some((key, value)) = line.split_once('=') {
        (key.to_string(), value.to_string())
    } else {
        (line.to_string(), String::new())
    }
}

fn env_column_widths(width: usize, envs: &[String]) -> Vec<usize> {
    let separators = 3usize;
    let content_width = width.saturating_sub(separators);
    if content_width == 0 {
        return vec![0, 0];
    }

    let mut max_key = 3usize;
    for line in envs {
        let (key, _) = split_env_line(line);
        max_key = max_key.max(key.chars().count());
    }

    let min_key = 6usize;
    let max_key_allowed = (content_width / 2).max(min_key);
    let key_width = max_key.min(max_key_allowed);
    let value_width = content_width.saturating_sub(key_width);
    vec![key_width, value_width]
}

fn format_env_header(widths: &[usize]) -> String {
    let key = fit_left("KEY", widths[0]);
    let value = fit_left("VALUE", widths[1]);
    format!("│{}│{}│", key, value)
}

fn format_env_line(widths: &[usize], key: &str, value: &str) -> String {
    let key_cell = fit_left(key, widths[0]);
    let value_cell = fit_left(value, widths[1]);
    format!("│{}│{}│", key_cell, value_cell)
}

fn env_info_widths(
    width: usize,
    left_row1: &str,
    left_row2: &str,
    right_row1: &str,
    right_row2: &str,
) -> Vec<usize> {
    let separators = 3usize;
    let content_width = width.saturating_sub(separators);
    if content_width == 0 {
        return vec![0, 0];
    }

    let mut left_len = left_row1.chars().count().max(left_row2.chars().count());
    let mut right_len = right_row1.chars().count().max(right_row2.chars().count());
    let min_col = 12usize;

    left_len = left_len.max(min_col);
    right_len = right_len.max(min_col);

    if left_len + right_len > content_width {
        let overflow = left_len + right_len - content_width;
        let left_shrink = overflow.min(left_len.saturating_sub(min_col));
        left_len = left_len.saturating_sub(left_shrink);
        let remaining = overflow.saturating_sub(left_shrink);
        if remaining > 0 {
            let right_shrink = remaining.min(right_len.saturating_sub(min_col));
            right_len = right_len.saturating_sub(right_shrink);
        }
    }

    if left_len + right_len > content_width {
        right_len = content_width.saturating_sub(left_len);
    } else if left_len + right_len < content_width {
        right_len = content_width.saturating_sub(left_len);
    }

    vec![left_len, right_len]
}

fn format_env_info_row(widths: &[usize], left: &str, right: &str) -> String {
    let left_cell = fit_left(left, widths[0]);
    let right_cell = fit_left(right, widths[1]);
    format!("│{}│{}│", left_cell, right_cell)
}

fn format_bottom_border(widths: &[usize]) -> String {
    let mut line = String::new();
    line.push('└');
    for (idx, width) in widths.iter().enumerate() {
        line.push_str(&"─".repeat(*width));
        if idx + 1 == widths.len() {
            line.push('┘');
        } else {
            line.push('┴');
        }
    }
    line
}
