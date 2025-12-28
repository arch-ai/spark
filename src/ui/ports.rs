use std::io::{self, Write};

use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal;

use crate::app::{AppState, InputMode};
use crate::system::ports::{PortInfo, PortRow};

use super::bars::{format_cpu_bar, format_memory_bar, format_swap_bar};
use super::layout::{layout_for_screen, render_sidebar, render_sidebar_gap};
use super::table::{
    clear_list_area_at, fit_left, fit_right, format_separator, format_top_border,
    is_dim_mode, print_table_bar, render_help_table_rows_colored_at, render_line_at,
    render_search_box_at, render_title_at, set_dim_mode, truncate_str, HelpSegment,
};

pub fn render_ports(
    stdout: &mut io::Stdout,
    state: &AppState,
    ports: &[PortInfo],
    rows: &[PortRow],
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

    let mode_label = match state.input_mode {
        InputMode::Normal => "NORMAL",
        InputMode::Filter => "FILTER",
    };
    let mut row = 0u16;
    render_line_at(
        stdout,
        main_x,
        row,
        &format!("┌{}┐", "─".repeat(width_usize.saturating_sub(2))),
        width_usize,
    )?;
    row += 1;
    render_title_at(stdout, main_x, row, width_usize, "PORTS VIEW")?;
    row += 2;

    let header = format!("Spark | View: PORTS | Mode: {}", mode_label);
    render_line_at(stdout, main_x, row, &header, width_usize)?;
    row += 1;
    render_search_box_at(stdout, main_x, row, width_usize, &state.ports_filter)?;
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

    let port_widths = ports_column_widths(width_usize);
    let port_top = format_top_border(&port_widths);
    render_line_at(stdout, main_x, row, &port_top, width_usize)?;
    row += 1;
    let port_header = format_ports_header(&port_widths);
    render_line_at(stdout, main_x, row, &port_header, width_usize)?;
    row += 1;
    let port_sep = format_separator(&port_widths);
    render_line_at(stdout, main_x, row, &port_sep, width_usize)?;
    row += 1;

    let list_start = row as usize;
    let footer_lines = 5usize;
    let max_rows = height_usize.saturating_sub(list_start + footer_lines);

    if max_rows > 0 {
        if rows.is_empty() {
            render_line_at(stdout, main_x, list_start as u16, "No ports found.", width_usize)?;
            clear_list_area_at(
                stdout,
                main_x,
                list_start + 1,
                max_rows.saturating_sub(1),
                width_usize,
            )?;
        } else {
            let scroll = if state.selected >= max_rows {
                state.selected - max_rows + 1
            } else {
                0
            };
            let end = (scroll + max_rows).min(rows.len());
            let mut rendered = 0usize;
            for (idx, row) in rows[scroll..end].iter().enumerate() {
                let line_index = scroll + idx;
                let y = list_start + idx;
                let selected = line_index == state.selected && !dim;
                match row {
                    PortRow::Group { name, count } => {
                        render_port_group_row_at(
                            stdout,
                            main_x,
                            y as u16,
                            &port_widths,
                            width_usize,
                            name,
                            *count,
                            selected,
                        )?;
                    }
                    PortRow::Item { index } => {
                        let port = &ports[*index];
                        let is_last_in_group = match rows.get(line_index + 1) {
                            None => true,
                            Some(PortRow::Group { .. }) => true,
                            Some(PortRow::Item { .. }) => false,
                        };
                        let prefix = if is_last_in_group { "└─ " } else { "├─ " };
                        let name = format!("{prefix}{}", port.name);
                        let line = format_ports_line(port, &port_widths, &name);
                        if selected {
                            queue!(
                                stdout,
                                MoveTo(main_x, y as u16),
                                SetAttribute(Attribute::Reverse),
                                Print(fit_left(&line, width_usize)),
                                SetAttribute(Attribute::Reset)
                            )?;
                        } else {
                            render_line_at(stdout, main_x, y as u16, &line, width_usize)?;
                        }
                    }
                };
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
        let clear_key = if state.input_mode == InputMode::Normal && !state.ports_filter.is_empty()
        {
            clear_key_active
        } else {
            clear_key_default
        };
        let clear_label = if state.input_mode == InputMode::Normal && !state.ports_filter.is_empty()
        {
            clear_label_active
        } else {
            clear_label_default
        };
        let help_rows = vec![
            vec![
                HelpSegment::plain("Filters: "),
                HelpSegment::key("/"),
                HelpSegment::plain(" search | "),
                clear_key,
                clear_label,
            ],
            vec![
                HelpSegment::plain("Actions: "),
                HelpSegment::key("p"),
                HelpSegment::plain(" process | "),
                HelpSegment::key("d"),
                HelpSegment::plain(" docker | "),
                HelpSegment::key("k"),
                HelpSegment::plain(" kill | "),
                HelpSegment::key("q"),
                HelpSegment::plain(" quit | "),
                HelpSegment::key("arrows"),
                HelpSegment::plain(" move | "),
                HelpSegment::key("left/right"),
                HelpSegment::plain(" focus"),
            ],
        ];
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

fn ports_column_widths(width: usize) -> Vec<usize> {
    let proto_width = 5usize;
    let port_width = 6usize;
    let pid_width = 7usize;
    let separators = 7usize;
    let content_width = width.saturating_sub(separators);
    let remaining = content_width.saturating_sub(proto_width + port_width + pid_width);
    let min_name = 10usize;
    let min_project = 8usize;
    let min_path = 10usize;

    let (name_width, project_width, path_width) = if remaining >= min_name + min_project + min_path
    {
        let mut name_width = remaining * 2 / 5;
        let mut project_width = remaining / 5;
        let mut path_width = remaining - name_width - project_width;
        if name_width < min_name {
            name_width = min_name;
            path_width = remaining - name_width - project_width;
        }
        if project_width < min_project {
            project_width = min_project;
            path_width = remaining - name_width - project_width;
        }
        if path_width < min_path {
            path_width = min_path;
            let leftover = remaining.saturating_sub(path_width);
            name_width = (leftover * 2 / 3).max(min_name);
            project_width = leftover.saturating_sub(name_width).max(min_project);
        }
        (name_width, project_width, path_width)
    } else if remaining >= min_name + min_path {
        (remaining - min_path, 0, min_path)
    } else {
        (remaining, 0, 0)
    };

    vec![
        proto_width,
        port_width,
        pid_width,
        name_width,
        project_width,
        path_width,
    ]
}

fn format_ports_header(widths: &[usize]) -> String {
    let cells = vec![
        fit_left("PROTO", widths[0]),
        fit_right("PORT", widths[1]),
        fit_right("PID", widths[2]),
        fit_left("NAME", widths[3]),
        fit_left("PROJECT", widths[4]),
        fit_left("PATH", widths[5]),
    ];
    format!(
        "│{}│{}│{}│{}│{}│{}│",
        cells[0], cells[1], cells[2], cells[3], cells[4], cells[5]
    )
}

fn format_ports_line(port: &PortInfo, widths: &[usize], name: &str) -> String {
    let proto_cell = fit_left(&port.proto, widths[0]);
    let port_cell = fit_right(&port.port.to_string(), widths[1]);
    let pid_cell = if port.pid == sysinfo::Pid::from_u32(0) {
        fit_right("-", widths[2])
    } else {
        fit_right(&port.pid.to_string(), widths[2])
    };
    let name_cell = fit_left(name, widths[3]);
    let project_cell = fit_left(port.project_name.as_deref().unwrap_or("-"), widths[4]);
    let path_cell = fit_left(&port.exe_path, widths[5]);

    format!(
        "│{}│{}│{}│{}│{}│{}│",
        proto_cell, port_cell, pid_cell, name_cell, project_cell, path_cell
    )
}

fn format_ports_group_line(name: &str, count: usize, widths: &[usize]) -> String {
    let count_label = format!("{count} ports");
    let proto_cell = fit_left("", widths[0]);
    let port_cell = fit_right("", widths[1]);
    let pid_cell = fit_right("", widths[2]);
    let name_cell = fit_left(name, widths[3]);
    let project_cell = fit_left("-", widths[4]);
    let path_cell = fit_left(&count_label, widths[5]);

    format!(
        "│{}│{}│{}│{}│{}│{}│",
        proto_cell, port_cell, pid_cell, name_cell, project_cell, path_cell
    )
}

fn render_port_group_row_at(
    stdout: &mut io::Stdout,
    x: u16,
    y: u16,
    widths: &[usize],
    table_width: usize,
    name: &str,
    count: usize,
    selected: bool,
) -> io::Result<()> {
    let label = name;
    let count_label = format!("{count} ports");

    let proto_cell = fit_left("", widths[0]);
    let port_cell = fit_right("", widths[1]);
    let pid_cell = fit_right("", widths[2]);
    let project_cell = fit_left("-", widths[4]);
    let path_cell = fit_left(&count_label, widths[5]);

    if selected && !is_dim_mode() {
        let line = format_ports_group_line(label, count, widths);
        queue!(
            stdout,
            MoveTo(x, y),
            SetAttribute(Attribute::Reverse),
            Print(fit_left(&line, table_width)),
            SetAttribute(Attribute::Reset)
        )?;
        return Ok(());
    }

    queue!(stdout, MoveTo(x, y))?;
    print_table_bar(stdout)?;
    print_dim_cell(stdout, &proto_cell)?;
    print_table_bar(stdout)?;
    print_dim_cell(stdout, &port_cell)?;
    print_table_bar(stdout)?;
    print_dim_cell(stdout, &pid_cell)?;
    print_table_bar(stdout)?;
    render_group_name_cell(stdout, label, widths[3])?;
    print_table_bar(stdout)?;
    print_dim_cell(stdout, &project_cell)?;
    print_table_bar(stdout)?;
    print_dim_cell(stdout, &path_cell)?;
    print_table_bar(stdout)?;
    Ok(())
}

fn render_group_name_cell(stdout: &mut io::Stdout, label: &str, width: usize) -> io::Result<()> {
    let display = truncate_str(label, width);
    let display_len = display.chars().count();
    if is_dim_mode() {
        queue!(
            stdout,
            SetForegroundColor(Color::DarkGrey),
            Print(&display),
            ResetColor
        )?;
    } else {
        queue!(
            stdout,
            SetForegroundColor(Color::Yellow),
            Print(&display),
            ResetColor
        )?;
    }

    let remaining = width.saturating_sub(display_len);
    if remaining > 0 {
        queue!(stdout, Print(" ".repeat(remaining)))?;
    }
    Ok(())
}

fn print_dim_cell(stdout: &mut io::Stdout, text: &str) -> io::Result<()> {
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
    Ok(())
}
