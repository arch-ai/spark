use std::io::{self, Write};

use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal;

use crate::app::{AppState, InputMode};
use crate::system::ports::PortInfo;

use super::bars::{format_cpu_bar, format_memory_bar, format_swap_bar};
use super::layout::{layout_for_screen, render_sidebar, render_sidebar_gap};
use super::table::{
    clear_list_area_at, fit_left, fit_right, format_separator, format_top_border,
    render_help_table_rows_colored_at, render_line_at, render_search_box_at, render_title_at,
    set_dim_mode, HelpSegment,
};

pub fn render_ports(
    stdout: &mut io::Stdout,
    state: &AppState,
    ports: &[PortInfo],
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
        if ports.is_empty() {
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
            let end = (scroll + max_rows).min(ports.len());
            let mut rendered = 0usize;
            for (idx, port) in ports[scroll..end].iter().enumerate() {
                let line_index = scroll + idx;
                let y = list_start + idx;
                let line = format_ports_line(port, &port_widths);
                if line_index == state.selected && !dim {
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
    let separators = 6usize;
    let content_width = width.saturating_sub(separators);
    let remaining = content_width.saturating_sub(proto_width + port_width + pid_width);
    let min_name = 10usize;
    let min_path = 10usize;

    let (name_width, path_width) = if remaining >= min_name + min_path {
        let mut name_width = remaining * 2 / 5;
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

    vec![proto_width, port_width, pid_width, name_width, path_width]
}

fn format_ports_header(widths: &[usize]) -> String {
    let cells = vec![
        fit_left("PROTO", widths[0]),
        fit_right("PORT", widths[1]),
        fit_right("PID", widths[2]),
        fit_left("NAME", widths[3]),
        fit_left("PATH", widths[4]),
    ];
    format!(
        "│{}│{}│{}│{}│{}│",
        cells[0], cells[1], cells[2], cells[3], cells[4]
    )
}

fn format_ports_line(port: &PortInfo, widths: &[usize]) -> String {
    let proto_cell = fit_left(&port.proto, widths[0]);
    let port_cell = fit_right(&port.port.to_string(), widths[1]);
    let pid_cell = if port.pid == sysinfo::Pid::from_u32(0) {
        fit_right("-", widths[2])
    } else {
        fit_right(&port.pid.to_string(), widths[2])
    };
    let name_cell = fit_left(&port.name, widths[3]);
    let path_cell = fit_left(&port.exe_path, widths[4]);

    format!(
        "│{}│{}│{}│{}│{}│",
        proto_cell, port_cell, pid_cell, name_cell, path_cell
    )
}
