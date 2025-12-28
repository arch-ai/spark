use std::io;

use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::{cursor::MoveTo, queue};

use crate::app::{AppState, Focus, ViewMode};

use super::table::{center_text, fit_left, is_dim_mode, print_table_bar, render_line_at};

const SIDEBAR_WIDTH: usize = 20;
const SIDEBAR_GAP: usize = 1;
const MAIN_MIN_WIDTH: usize = 40;

pub(crate) struct Layout {
    pub show_sidebar: bool,
    pub sidebar_width: usize,
    pub gap: usize,
    pub main_width: usize,
    pub main_x: u16,
}

pub(crate) fn layout_for_screen(width: usize) -> Layout {
    let show_sidebar = width >= SIDEBAR_WIDTH + SIDEBAR_GAP + MAIN_MIN_WIDTH;
    let sidebar_width = if show_sidebar { SIDEBAR_WIDTH } else { 0 };
    let gap = if show_sidebar { SIDEBAR_GAP } else { 0 };
    let main_width = if show_sidebar {
        width.saturating_sub(sidebar_width + gap)
    } else {
        width
    };
    let main_x = if show_sidebar {
        (sidebar_width + gap) as u16
    } else {
        0
    };
    Layout {
        show_sidebar,
        sidebar_width,
        gap,
        main_width,
        main_x,
    }
}

pub(crate) fn render_sidebar(
    stdout: &mut io::Stdout,
    state: &AppState,
    layout: &Layout,
    height: usize,
) -> io::Result<()> {
    if !layout.show_sidebar || layout.sidebar_width < 3 || height == 0 {
        return Ok(());
    }

    let width = layout.sidebar_width;
    let inner = width.saturating_sub(2);
    let top = format!("┌{}┐", "─".repeat(inner));
    let bottom = format!("└{}┘", "─".repeat(inner));
    let title = center_text("SPARK", inner);
    let title_line = format!("│{}│", title);
    let sep = format!("├{}┤", "─".repeat(inner));
    let logo_lines = [
        "     .     .",
        "  .  |\\   /|  .",
        "     | \\ / |",
        " ---+-- * --+---",
        "     | / \\ |",
        "  .  |/   \\|  .",
        "     '     '",
    ];

    render_line_at(stdout, 0, 0, &top, width)?;
    if height == 1 {
        return Ok(());
    }

    let bottom_row = height.saturating_sub(1);
    let mut row = 1usize;
    let available_inner = bottom_row.saturating_sub(row);
    let min_after_logo = 2usize;
    let max_logo = available_inner.saturating_sub(min_after_logo);
    let logo_count = logo_lines.len().min(max_logo);

    for line in logo_lines.iter().take(logo_count) {
        let line = format!("│{}│", fit_left(line, inner));
        render_line_at(stdout, 0, row as u16, &line, width)?;
        row += 1;
        if row >= bottom_row {
            render_line_at(stdout, 0, bottom_row as u16, &bottom, width)?;
            return Ok(());
        }
    }

    if row < bottom_row {
        render_line_at(stdout, 0, row as u16, &title_line, width)?;
        row += 1;
    }
    if row < bottom_row {
        render_line_at(stdout, 0, row as u16, &sep, width)?;
        row += 1;
    }

    let items = ["Processes", "Ports", "Docker", "Node JS"];
    let active_view = if state.view_mode == ViewMode::DockerEnv {
        state.env_return_view
    } else {
        state.view_mode
    };
    let active_index = match active_view {
        ViewMode::Process => 0,
        ViewMode::Ports => 1,
        ViewMode::Docker | ViewMode::DockerEnv => 2,
        ViewMode::Node => 3,
    };
    let list_start = row;
    let mut item_idx = 0usize;

    for y in list_start..bottom_row {
        if item_idx < items.len() {
            let label = format!(" {}", items[item_idx]);
            let is_active = item_idx == active_index;
            let is_selected = item_idx == state.sidebar_index;
            render_sidebar_item(
                stdout,
                0,
                y as u16,
                width,
                &label,
                is_active,
                is_selected,
                state.focus == Focus::Sidebar,
            )?;
            item_idx += 1;
        } else {
            let empty = format!("│{}│", " ".repeat(inner));
            render_line_at(stdout, 0, y as u16, &empty, width)?;
        }
    }

    render_line_at(stdout, 0, bottom_row as u16, &bottom, width)?;
    Ok(())
}

pub(crate) fn render_sidebar_gap(
    stdout: &mut io::Stdout,
    layout: &Layout,
    height: usize,
) -> io::Result<()> {
    if !layout.show_sidebar || layout.gap == 0 {
        return Ok(());
    }
    let gap_x = layout.sidebar_width as u16;
    for y in 0..height {
        render_line_at(stdout, gap_x, y as u16, "", layout.gap)?;
    }
    Ok(())
}

fn render_sidebar_item(
    stdout: &mut io::Stdout,
    x: u16,
    y: u16,
    width: usize,
    label: &str,
    is_active: bool,
    is_selected: bool,
    focus_sidebar: bool,
) -> io::Result<()> {
    let inner = width.saturating_sub(2);
    let text = fit_left(label, inner);
    queue!(stdout, MoveTo(x, y))?;
    print_table_bar(stdout)?;
    if is_dim_mode() {
        queue!(stdout, SetForegroundColor(Color::DarkGrey), Print(text), ResetColor)?;
    } else if focus_sidebar && is_selected {
        queue!(
            stdout,
            SetAttribute(Attribute::Reverse),
            Print(text),
            SetAttribute(Attribute::Reset)
        )?;
    } else if is_active {
        queue!(stdout, SetForegroundColor(Color::Cyan), Print(text), ResetColor)?;
    } else {
        queue!(stdout, Print(text))?;
    }
    print_table_bar(stdout)?;
    Ok(())
}
