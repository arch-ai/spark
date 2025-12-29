use std::io::{self, Write};

use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor};
use crossterm::terminal;

use crate::app::{AppState, InputMode, SortBy, SortOrder};
use crate::system::process::{ProcInfo, TreeRow};

use super::bars::{format_cpu_bar, format_memory_bar, format_swap_bar};
use super::layout::{layout_for_screen, render_sidebar, render_sidebar_gap};
use super::table::{
    clear_list_area_at, fit_left, fit_right, format_separator, format_top_border,
    render_help_table_rows_colored_at, render_line_at, render_search_box_at, render_title_at,
    set_dim_mode, HelpSegment,
};

pub fn render_processes(
    stdout: &mut io::Stdout,
    state: &AppState,
    processes: &std::collections::HashMap<sysinfo::Pid, ProcInfo>,
    rows: &[TreeRow],
) -> io::Result<()> {
    let (width, height) = terminal::size().unwrap_or((80, 24));
    let screen_width = width as usize;
    let height_usize = height as usize;
    let dim = state.input_mode == InputMode::Filter;
    set_dim_mode(dim);
    let layout = layout_for_screen(screen_width);
    if layout.show_sidebar {
        render_sidebar(stdout, state, &layout, height_usize)?;
        render_sidebar_gap(stdout, &layout, height_usize)?;
    }
    let width_usize = layout.main_width;
    let main_x = layout.main_x;

    queue!(stdout, MoveTo(main_x, 0))?;

    let sort_label = match state.sort_by {
        SortBy::Cpu => "CPU",
        SortBy::Memory => "MEM",
        SortBy::Name => "NAME",
    };
    let order_label = match state.sort_order {
        SortOrder::Asc => "asc",
        SortOrder::Desc => "desc",
    };
    let mode_label = match state.input_mode {
        InputMode::Normal => "NORMAL",
        InputMode::Filter => "FILTER",
    };

    let zoom_label = if state.zoom { "ON" } else { "OFF" };
    let mut row = 0u16;
    render_line_at(
        stdout,
        main_x,
        row,
        &format!("┌{}┐", "─".repeat(width_usize.saturating_sub(2))),
        width_usize,
    )?;
    row += 1;
    render_title_at(stdout, main_x, row, width_usize, "PROCESS VIEW")?;
    row += 2;

    let header = format!(
        "Spark | View: PROC | Sort: {} {} | Zoom: {} | Mode: {}",
        sort_label, order_label, zoom_label, mode_label
    );
    render_line_at(stdout, main_x, row, &header, width_usize)?;
    row += 1;
    render_search_box_at(stdout, main_x, row, width_usize, &state.process_filter)?;
    row += 3;
    render_line_at(
        stdout,
        main_x,
        row,
        &format_cpu_bar("CPU ", state.cpu_usage, width_usize),
        width_usize,
    )?;
    row += 1;
    render_line_at(
        stdout,
        main_x,
        row,
        &format_memory_bar("MEM ", state.mem_total, state.mem_available, width_usize),
        width_usize,
    )?;
    row += 1;
    render_line_at(
        stdout,
        main_x,
        row,
        &format_swap_bar("SWAP", state.swap_total, state.swap_used, width_usize),
        width_usize,
    )?;
    row += 1;
    let mut max_user_len = 4usize;
    for row in rows {
        if let Some(proc_info) = processes.get(&row.pid) {
            max_user_len = max_user_len.max(proc_info.user.chars().count());
        }
    }
    let proc_widths = process_column_widths(width_usize, max_user_len);
    let proc_top = format_top_border(&proc_widths);
    render_line_at(stdout, main_x, row, &proc_top, width_usize)?;
    row += 1;
    let proc_header = format_process_header(&proc_widths);
    render_line_at(stdout, main_x, row, &proc_header, width_usize)?;
    row += 1;
    let proc_sep = format_separator(&proc_widths);
    render_line_at(stdout, main_x, row, &proc_sep, width_usize)?;
    row += 1;

    let list_start = row as usize;
    let footer_lines = 5usize;
    let max_rows = height_usize.saturating_sub(list_start + footer_lines);

    if max_rows > 0 {
        if processes.is_empty() {
            render_line_at(
                stdout,
                main_x,
                list_start as u16,
                "No matching processes.",
                width_usize,
            )?;
            clear_list_area_at(
                stdout,
                main_x,
                list_start + 1,
                max_rows.saturating_sub(1),
                width_usize,
            )?;
        } else {
            // Keep selection centered when possible
            let total = rows.len();
            let half = max_rows / 2;
            let scroll = if state.selected <= half {
                0
            } else if state.selected + half >= total {
                total.saturating_sub(max_rows)
            } else {
                state.selected - half
            };
            let end = (scroll + max_rows).min(rows.len());
            let mut rendered = 0usize;
            for (idx, row) in rows[scroll..end].iter().enumerate() {
                let line_index = scroll + idx;
                let y = list_start + idx;
                let Some(proc_info) = processes.get(&row.pid) else {
                    continue;
                };
                let line = format_process_line(proc_info, &proc_widths, &row.prefix);
                let is_selected = line_index == state.selected && !dim;
                let is_hovered = state.hover_row == Some(line_index) && !is_selected && !dim;
                if is_selected {
                    queue!(
                        stdout,
                        MoveTo(main_x, y as u16),
                        SetAttribute(Attribute::Reverse),
                        Print(fit_left(&line, width_usize)),
                        SetAttribute(Attribute::Reset)
                    )?;
                } else if is_hovered {
                    queue!(
                        stdout,
                        MoveTo(main_x, y as u16),
                        SetBackgroundColor(Color::DarkGrey),
                        Print(fit_left(&line, width_usize)),
                        ResetColor
                    )?;
                } else {
                    render_line_at(stdout, main_x, y as u16, &line, width_usize)?;
                }
                rendered += 1;
            }
            clear_list_area_at(
                stdout,
                main_x,
                list_start + rendered,
                max_rows.saturating_sub(rendered),
                width_usize,
            )?;
        }
    }

    if height_usize >= footer_lines {
        let message_line = height_usize.saturating_sub(footer_lines) as u16;
        let message = state.message.as_deref().unwrap_or("");
        render_line_at(stdout, main_x, message_line, message, width_usize)?;

        let clear_key_active = HelpSegment::highlight("x");
        let clear_key_default = HelpSegment::key("x");
        let clear_label_active = HelpSegment::highlight(" clear search");
        let clear_label_default = HelpSegment::plain(" clear search");
        let clear_key = if state.input_mode == InputMode::Normal && !state.process_filter.is_empty()
        {
            clear_key_active
        } else {
            clear_key_default
        };
        let clear_label = if state.input_mode == InputMode::Normal && !state.process_filter.is_empty()
        {
            clear_label_active
        } else {
            clear_label_default
        };
        let help_rows = if state.input_mode == InputMode::Filter {
            vec![
                vec![
                    HelpSegment::plain("Filters: "),
                    HelpSegment::key("/"),
                    HelpSegment::plain(" search | "),
                    HelpSegment::key("x"),
                    HelpSegment::plain(" clear search"),
                ],
                vec![
                    HelpSegment::plain("Actions: "),
                    HelpSegment::key("Enter/Esc"),
                    HelpSegment::plain(" exit | "),
                    HelpSegment::key("Backspace"),
                    HelpSegment::plain(" delete"),
                ],
            ]
        } else {
            vec![
                vec![
                    HelpSegment::plain("Filters: "),
                    HelpSegment::key("/"),
                    HelpSegment::plain(" search | "),
                    clear_key,
                    clear_label,
                ],
                vec![
                    HelpSegment::plain("Actions: "),
                    HelpSegment::key("z"),
                    HelpSegment::plain(" zoom | "),
                    HelpSegment::key("d"),
                    HelpSegment::plain(" docker | "),
                    HelpSegment::key("p"),
                    HelpSegment::plain(" ports | "),
                    HelpSegment::key("e"),
                    HelpSegment::plain(" env | "),
                    HelpSegment::key("k"),
                    HelpSegment::plain(" kill | "),
                    HelpSegment::key("q"),
                    HelpSegment::plain(" quit | "),
                    HelpSegment::key("arrows"),
                    HelpSegment::plain(" move | "),
                    HelpSegment::key("left/right"),
                    HelpSegment::plain(" focus"),
                ],
            ]
        };
        let help_start = height_usize.saturating_sub(help_rows.len() + 2) as u16;
        render_help_table_rows_colored_at(stdout, main_x, help_start, width_usize, &help_rows)?;
    }

    if dim {
        set_dim_mode(false);
        super::search::render_search_bar_at(stdout, main_x, 0, width_usize, state.active_filter())?;
    } else {
        set_dim_mode(false);
    }

    stdout.flush()?;
    Ok(())
}

fn format_process_line(proc_info: &ProcInfo, widths: &[usize], prefix: &str) -> String {
    let pid_str = proc_info.pid.to_string();
    let cpu = proc_info.cpu;
    let mem_gb = proc_info.memory_bytes as f64 / 1024.0 / 1024.0 / 1024.0;

    let user_width = widths[3];
    let name_width = widths[4];
    let path_width = widths[5];

    let mut name_text = proc_info.name.clone();
    if let Some(container) = proc_info.container.as_deref() {
        name_text.push_str(" @");
        name_text.push_str(container);
    }

    let prefix_len = prefix.chars().count();
    let name_space = name_width.saturating_sub(prefix_len);
    let name_body = super::table::truncate_str(&name_text, name_space);
    let name = if name_space == 0 {
        prefix.to_string()
    } else {
        format!("{prefix}{name_body}")
    };

    let pid_cell = fit_right(&pid_str, widths[0]);
    let cpu_cell = fit_right(&format!("{:.1}", cpu), widths[1]);
    let mem_cell = fit_right(&format!("{:.2}", mem_gb), widths[2]);
    let user_cell = fit_left(&proc_info.user, user_width);
    let name_cell = fit_left(&name, name_width);
    let path_cell = fit_left(&proc_info.exe_path, path_width);

    format!(
        "│{}│{}│{}│{}│{}│{}│",
        pid_cell, cpu_cell, mem_cell, user_cell, name_cell, path_cell
    )
}

fn process_column_widths(width: usize, max_user_len: usize) -> Vec<usize> {
    let pid_width = 7usize;
    let cpu_width = 6usize;
    let mem_width = 9usize;
    let mut user_width = max_user_len.max(4);
    let separators = 7usize;
    let content_width = width.saturating_sub(separators);
    let fixed = pid_width + cpu_width + mem_width;
    let available = content_width.saturating_sub(fixed);
    if available == 0 {
        return vec![pid_width, cpu_width, mem_width, 0, 0, 0];
    }
    if user_width > available {
        user_width = available;
    }
    let mut remaining = available.saturating_sub(user_width);
    let min_name = 10usize;
    let min_path = 10usize;
    let min_user = 4usize;

    if remaining < min_name + min_path {
        let deficit = (min_name + min_path).saturating_sub(remaining);
        let shrink = deficit.min(user_width.saturating_sub(min_user));
        user_width = user_width.saturating_sub(shrink);
        remaining = available.saturating_sub(user_width);
    }

    let (name_width, path_width) = if remaining >= min_name + min_path {
        let mut name_width = remaining * 2 / 3;
        let mut path_width = remaining - name_width;
        if name_width < min_name {
            name_width = min_name;
            path_width = remaining - name_width;
        }
        if path_width < min_path {
            path_width = min_path;
            name_width = remaining - path_width;
        }
        (name_width, path_width)
    } else {
        (remaining, 0)
    };

    vec![
        pid_width,
        cpu_width,
        mem_width,
        user_width,
        name_width,
        path_width,
    ]
}

fn format_process_header(widths: &[usize]) -> String {
    let cells = vec![
        fit_right("PID", widths[0]),
        fit_right("CPU%", widths[1]),
        fit_right("MEM(GB)", widths[2]),
        fit_left("USER", widths[3]),
        fit_left("NAME@CTR", widths[4]),
        fit_left("PATH", widths[5]),
    ];
    format!(
        "│{}│{}│{}│{}│{}│{}│",
        cells[0], cells[1], cells[2], cells[3], cells[4], cells[5]
    )
}
