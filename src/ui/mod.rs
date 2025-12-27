use std::io::{self, Write};

use crossterm::cursor::MoveTo;
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal;
use crossterm::queue;

use crate::app::{AppState, InputMode, SortBy, SortOrder};
use crate::system::docker::{ContainerInfo, DockerRow};
use crate::system::ports::PortInfo;
use crate::system::process::{ProcInfo, TreeRow};

mod docker_env;
pub use docker_env::render_docker_envs;

pub fn render_processes(
    stdout: &mut io::Stdout,
    state: &AppState,
    processes: &std::collections::HashMap<sysinfo::Pid, ProcInfo>,
    rows: &[TreeRow],
) -> io::Result<()> {
    let (width, height) = terminal::size().unwrap_or((80, 24));
    let width_usize = width as usize;
    let height_usize = height as usize;

    queue!(stdout, MoveTo(0, 0))?;

    let filter_display = if state.process_filter.is_empty() {
        "<none>"
    } else {
        state.process_filter.as_str()
    };
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
    render_line(stdout, row, &format!("┌{}┐", "─".repeat(width_usize.saturating_sub(2))), width_usize)?;
    row += 1;
    render_title(stdout, row, width_usize, "PROCESS VIEW")?;
    row += 2;

    let header = format!(
        "Rust Task Manager | View: PROC | Filter: {} | Sort: {} {} | Zoom: {} | Mode: {}",
        filter_display, sort_label, order_label, zoom_label, mode_label
    );
    render_line(stdout, row, &header, width_usize)?;
    row += 1;
    render_line(
        stdout,
        row,
        &format_cpu_bar("CPU ", state.cpu_usage, width_usize),
        width_usize,
    )?;
    row += 1;
    render_line(
        stdout,
        row,
        &format_memory_bar("MEM ", state.mem_total, state.mem_available, width_usize),
        width_usize,
    )?;
    row += 1;
    render_line(
        stdout,
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
    render_line(stdout, row, &proc_top, width_usize)?;
    row += 1;
    let proc_header = format_process_header(&proc_widths);
    render_line(stdout, row, &proc_header, width_usize)?;
    row += 1;
    let proc_sep = format_separator(&proc_widths);
    render_line(stdout, row, &proc_sep, width_usize)?;
    row += 1;

    let list_start = row as usize;
    let footer_lines = 5usize;
    let max_rows = height_usize.saturating_sub(list_start + footer_lines);

    if max_rows > 0 {
        if processes.is_empty() {
            render_line(stdout, list_start as u16, "No matching processes.", width_usize)?;
            clear_list_area(stdout, list_start + 1, max_rows.saturating_sub(1), width_usize)?;
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
                let Some(proc_info) = processes.get(&row.pid) else {
                    continue;
                };
                let line = format_process_line(proc_info, &proc_widths, &row.prefix);
                if line_index == state.selected {
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
    }

    if height_usize >= footer_lines {
        let message_line = height_usize.saturating_sub(footer_lines) as u16;
        let message = state.message.as_deref().unwrap_or("");
        render_line(stdout, message_line, message, width_usize)?;

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
                    HelpSegment::key("x"),
                    HelpSegment::plain(" clear search"),
                ],
                vec![
                    HelpSegment::plain("Actions: "),
                    HelpSegment::key("z"),
                    HelpSegment::plain(" zoom | "),
                    HelpSegment::key("d"),
                    HelpSegment::plain(" docker | "),
                    HelpSegment::key("p"),
                    HelpSegment::plain(" ports | "),
                    HelpSegment::key("k"),
                    HelpSegment::plain(" kill | "),
                    HelpSegment::key("q"),
                    HelpSegment::plain(" quit | "),
                    HelpSegment::key("arrows"),
                    HelpSegment::plain(" move"),
                ],
            ]
        };
        let help_start = height_usize.saturating_sub(help_rows.len() + 2) as u16;
        render_help_table_rows_colored(stdout, help_start, width_usize, &help_rows)?;
    }

    stdout.flush()?;
    Ok(())
}

pub fn render_containers(
    stdout: &mut io::Stdout,
    state: &AppState,
    containers: &[ContainerInfo],
    rows: &[DockerRow],
) -> io::Result<()> {
    let (width, height) = terminal::size().unwrap_or((80, 24));
    let width_usize = width as usize;
    let height_usize = height as usize;

    queue!(stdout, MoveTo(0, 0))?;

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

    let mut row = 0u16;
    render_line(stdout, row, &format!("┌{}┐", "─".repeat(width_usize.saturating_sub(2))), width_usize)?;
    row += 1;
    render_title(stdout, row, width_usize, "DOCKER VIEW")?;
    row += 2;

    let header = format!(
        "Rust Task Manager | View: DOCKER | Sort: {} {} | Mode: {}",
        sort_label, order_label, mode_label
    );
    render_line(stdout, row, &header, width_usize)?;
    row += 1;
    render_line(
        stdout,
        row,
        &format_cpu_bar("CPU ", state.cpu_usage, width_usize),
        width_usize,
    )?;
    row += 1;
    render_line(
        stdout,
        row,
        &format_memory_bar("MEM ", state.mem_total, state.mem_available, width_usize),
        width_usize,
    )?;
    row += 1;
    render_line(
        stdout,
        row,
        &format_swap_bar("SWAP", state.swap_total, state.swap_used, width_usize),
        width_usize,
    )?;
    row += 1;
    let docker_widths = docker_column_widths(width_usize);
    let docker_top = format_top_border(&docker_widths);
    render_line(stdout, row, &docker_top, width_usize)?;
    row += 1;
    let docker_header = format_docker_header(&docker_widths);
    render_line(stdout, row, &docker_header, width_usize)?;
    row += 1;
    let docker_sep = format_separator(&docker_widths);
    render_line(stdout, row, &docker_sep, width_usize)?;
    row += 1;

    let list_start = row as usize;
    let footer_lines = 5usize;
    let max_rows = height_usize.saturating_sub(list_start + footer_lines);

    if max_rows > 0 {
        if rows.is_empty() {
            render_line(
                stdout,
                list_start as u16,
                "No running containers or Docker stats unavailable.",
                width_usize,
            )?;
            clear_list_area(stdout, list_start + 1, max_rows.saturating_sub(1), width_usize)?;
        } else {
            let selected_row = find_selected_row(rows, state.selected).unwrap_or(0);
            let scroll = if selected_row >= max_rows {
                selected_row.saturating_sub(max_rows - 1)
            } else {
                0
            };
            let end = (scroll + max_rows).min(rows.len());
            let mut rendered = 0usize;
            for (idx, row) in rows[scroll..end].iter().enumerate() {
                let line_index = scroll + idx;
                let y = list_start + idx;
                match row {
                    DockerRow::Group { name, path, count } => {
                        render_group_row(
                            stdout,
                            y as u16,
                            &docker_widths,
                            name,
                            path.as_deref(),
                            *count,
                        )?;
                    }
                    DockerRow::Separator => {
                        let line = format_separator(&docker_widths);
                        render_line(stdout, y as u16, &line, width_usize)?;
                    }
                    DockerRow::Item { index, prefix } => {
                        let Some(container) = containers.get(*index) else {
                            continue;
                        };
                        let line = format_container_line(container, width_usize, prefix);
                        if line_index == selected_row {
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
                    }
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
    }

    if height_usize >= footer_lines {
        let message_line = height_usize.saturating_sub(footer_lines) as u16;
        if !state.docker_filter.is_empty() {
            render_docker_search_status(
                stdout,
                message_line,
                width_usize,
                &state.docker_filter,
                state.docker_filtered_out,
            )?;
        } else {
            let message = state.message.as_deref().unwrap_or("");
            render_line(stdout, message_line, message, width_usize)?;
        }

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
                    HelpSegment::key("x"),
                    HelpSegment::plain(" clear search"),
                ],
                vec![
                    HelpSegment::plain("Actions: "),
                    HelpSegment::key("d"),
                    HelpSegment::plain(" process | "),
                    HelpSegment::key("p"),
                    HelpSegment::plain(" ports | "),
                    HelpSegment::key("enter"),
                    HelpSegment::plain(" shell | "),
                    HelpSegment::key("l"),
                    HelpSegment::plain(" logs | "),
                    HelpSegment::key("e"),
                    HelpSegment::plain(" env | "),
                    HelpSegment::key("q"),
                    HelpSegment::plain(" quit | "),
                    HelpSegment::key("arrows"),
                    HelpSegment::plain(" move"),
                ],
            ]
        };
        let help_start = height_usize.saturating_sub(help_rows.len() + 2) as u16;
        render_help_table_rows_colored(stdout, help_start, width_usize, &help_rows)?;
    }

    stdout.flush()?;
    Ok(())
}

pub fn render_ports(
    stdout: &mut io::Stdout,
    state: &AppState,
    ports: &[PortInfo],
) -> io::Result<()> {
    let (width, height) = terminal::size().unwrap_or((80, 24));
    let width_usize = width as usize;
    let height_usize = height as usize;

    queue!(stdout, MoveTo(0, 0))?;

    let mode_label = match state.input_mode {
        InputMode::Normal => "NORMAL",
        InputMode::Filter => "FILTER",
    };
    let filter_display = if state.ports_filter.is_empty() {
        "<none>"
    } else {
        state.ports_filter.as_str()
    };

    let mut row = 0u16;
    render_line(
        stdout,
        row,
        &format!("┌{}┐", "─".repeat(width_usize.saturating_sub(2))),
        width_usize,
    )?;
    row += 1;
    render_title(stdout, row, width_usize, "PORTS VIEW")?;
    row += 2;

    let header = format!(
        "Rust Task Manager | View: PORTS | Filter: {} | Mode: {}",
        filter_display, mode_label
    );
    render_line(stdout, row, &header, width_usize)?;
    row += 1;
    render_line(
        stdout,
        row,
        &format_cpu_bar("CPU ", state.cpu_usage, width_usize),
        width_usize,
    )?;
    row += 1;
    render_line(
        stdout,
        row,
        &format_memory_bar("MEM ", state.mem_total, state.mem_available, width_usize),
        width_usize,
    )?;
    row += 1;
    render_line(
        stdout,
        row,
        &format_swap_bar("SWAP", state.swap_total, state.swap_used, width_usize),
        width_usize,
    )?;
    row += 1;

    let port_widths = ports_column_widths(width_usize);
    let port_top = format_top_border(&port_widths);
    render_line(stdout, row, &port_top, width_usize)?;
    row += 1;
    let port_header = format_ports_header(&port_widths);
    render_line(stdout, row, &port_header, width_usize)?;
    row += 1;
    let port_sep = format_separator(&port_widths);
    render_line(stdout, row, &port_sep, width_usize)?;
    row += 1;

    let list_start = row as usize;
    let footer_lines = 5usize;
    let max_rows = height_usize.saturating_sub(list_start + footer_lines);

    if max_rows > 0 {
        if ports.is_empty() {
            render_line(stdout, list_start as u16, "No ports found.", width_usize)?;
            clear_list_area(stdout, list_start + 1, max_rows.saturating_sub(1), width_usize)?;
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
                if line_index == state.selected {
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
    }

    if height_usize >= footer_lines {
        let message_line = height_usize.saturating_sub(footer_lines) as u16;
        let message = state.message.as_deref().unwrap_or("");
        render_line(stdout, message_line, message, width_usize)?;

        let help_rows = vec![
            vec![
                HelpSegment::plain("Filters: "),
                HelpSegment::key("/"),
                HelpSegment::plain(" search | "),
                HelpSegment::key("x"),
                HelpSegment::plain(" clear search"),
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
                HelpSegment::plain(" move"),
            ],
        ];
        let help_start = height_usize.saturating_sub(help_rows.len() + 2) as u16;
        render_help_table_rows_colored(stdout, help_start, width_usize, &help_rows)?;
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
    let name_body = truncate_str(&name_text, name_space);
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

fn format_container_line(container: &ContainerInfo, width: usize, prefix: &str) -> String {
    let widths = docker_column_widths(width);
    let id = if container.id.len() > 12 {
        &container.id[..12]
    } else {
        container.id.as_str()
    };
    let mem_gb = container.memory_bytes as f64 / 1024.0 / 1024.0 / 1024.0;

    let id_cell = fit_right(id, widths[0]);
    let cpu_cell = fit_right(&format!("{:.1}", container.cpu), widths[1]);
    let mem_cell = fit_right(&format!("{:.2}", mem_gb), widths[2]);
    let name_text = format!("{prefix}{}", container.name);
    let name_cell = fit_left(&name_text, widths[3]);
    let image_cell = fit_left(&container.image, widths[4]);
    let port_cell = fit_left(&container.port_public, widths[5]);
    let int_port_cell = fit_left(&container.port_internal, widths[6]);
    let status_cell = fit_left(&container.status, widths[7]);

    format!(
        "│{}│{}│{}│{}│{}│{}│{}│{}│",
        id_cell, cpu_cell, mem_cell, name_cell, image_cell, port_cell, int_port_cell, status_cell
    )
}

fn render_group_row(
    stdout: &mut io::Stdout,
    y: u16,
    widths: &[usize],
    name: &str,
    path: Option<&str>,
    count: usize,
) -> io::Result<()> {
    let label = format!("group: {name}");
    let path_label = path.unwrap_or("-");
    let status_label = format!("{count} containers");

    let id_cell = fit_right("", widths[0]);
    let cpu_cell = fit_right("", widths[1]);
    let mem_cell = fit_right("", widths[2]);
    let image_cell = fit_left(path_label, widths[4]);
    let port_cell = fit_left("-", widths[5]);
    let int_port_cell = fit_left("-", widths[6]);
    let status_cell = fit_left(&status_label, widths[7]);

    queue!(stdout, MoveTo(0, y))?;
    print_table_bar(stdout)?;
    queue!(stdout, Print(id_cell))?;
    print_table_bar(stdout)?;
    queue!(stdout, Print(cpu_cell))?;
    print_table_bar(stdout)?;
    queue!(stdout, Print(mem_cell))?;
    print_table_bar(stdout)?;
    render_group_name_cell(stdout, &label, widths[3])?;
    print_table_bar(stdout)?;
    queue!(stdout, Print(image_cell))?;
    print_table_bar(stdout)?;
    queue!(stdout, Print(port_cell))?;
    print_table_bar(stdout)?;
    queue!(stdout, Print(int_port_cell))?;
    print_table_bar(stdout)?;
    queue!(stdout, Print(status_cell))?;
    print_table_bar(stdout)?;
    Ok(())
}

pub(super) fn render_line(stdout: &mut io::Stdout, y: u16, text: &str, width: usize) -> io::Result<()> {
    let line = fit_left(text, width);
    queue!(stdout, MoveTo(0, y))?;

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

fn render_docker_search_status(
    stdout: &mut io::Stdout,
    y: u16,
    width: usize,
    term: &str,
    filtered_out: usize,
) -> io::Result<()> {
    if width == 0 {
        return Ok(());
    }

    let prefix = "Search: ";
    let suffix = format!(" | {} docker filtered out | x clear search", filtered_out);
    let prefix_len = prefix.chars().count();
    let mut suffix_display = suffix.clone();
    let min_term = if term.is_empty() { 0 } else { 1 };

    if prefix_len + suffix_display.chars().count() + min_term > width {
        let max_suffix = width.saturating_sub(prefix_len + min_term);
        suffix_display = truncate_str(&suffix, max_suffix);
    }

    let suffix_len = suffix_display.chars().count();
    let available = width.saturating_sub(prefix_len + suffix_len);
    let term_display = truncate_str(term, available);
    let term_len = term_display.chars().count();

    queue!(stdout, MoveTo(0, y), Print(prefix))?;
    if !term_display.is_empty() {
        queue!(
            stdout,
            SetForegroundColor(Color::Blue),
            SetAttribute(Attribute::Bold),
            Print(term_display),
            SetAttribute(Attribute::Reset),
            ResetColor
        )?;
    }
    queue!(stdout, Print(suffix_display))?;

    let printed = prefix_len + term_len + suffix_len;
    if printed < width {
        queue!(stdout, Print(" ".repeat(width - printed)))?;
    }
    Ok(())
}

fn find_selected_row(rows: &[DockerRow], selected_container: usize) -> Option<usize> {
    for (idx, row) in rows.iter().enumerate() {
        if let DockerRow::Item { index, .. } = row {
            if *index == selected_container {
                return Some(idx);
            }
        }
    }
    None
}

pub(super) fn truncate_str(input: &str, max_len: usize) -> String {
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
        queue!(stdout, Print(text))?;
    }
    Ok(())
}

fn split_at_chars(input: &str, count: usize) -> (String, String) {
    let mut iter = input.chars();
    let left: String = iter.by_ref().take(count).collect();
    let right: String = iter.collect();
    (left, right)
}

pub(super) fn render_title(stdout: &mut io::Stdout, y: u16, width: usize, title: &str) -> io::Result<()> {
    let text = title.to_uppercase();
    let inner = width.saturating_sub(2);
    let line = if inner == 0 {
        text
    } else {
        let padded = center_text(&text, inner);
        format!("┌{}┐", padded)
    };
    render_line(stdout, y, &line, width)?;
    let underline = if inner == 0 {
        "─".repeat(width)
    } else {
        format!("└{}┘", "─".repeat(inner))
    };
    render_line(stdout, y + 1, &underline, width)?;
    Ok(())
}

fn center_text(text: &str, width: usize) -> String {
    let len = text.chars().count();
    if len >= width {
        return truncate_str(text, width);
    }
    let left = (width - len) / 2;
    let right = width - len - left;
    format!("{}{}{}", " ".repeat(left), text, " ".repeat(right))
}

pub(super) fn render_help_table_rows_colored(
    stdout: &mut io::Stdout,
    y: u16,
    width: usize,
    rows: &[Vec<HelpSegment>],
) -> io::Result<()> {
    let inner = width.saturating_sub(2);
    let top = format!("┌{}┐", "─".repeat(inner));
    render_line(stdout, y, &top, width)?;
    for (idx, row) in rows.iter().enumerate() {
        let line_y = y + 1 + idx as u16;
        queue!(stdout, MoveTo(0, line_y))?;
        print_table_bar(stdout)?;
        let mut remaining = inner;
        for segment in row {
            if remaining == 0 {
                break;
            }
            let text = take_prefix(&segment.text, remaining);
            let len = text.chars().count();
            if let Some(color) = segment.color {
                queue!(stdout, SetForegroundColor(color), Print(text), ResetColor)?;
            } else {
                queue!(stdout, Print(text))?;
            }
            remaining = remaining.saturating_sub(len);
        }
        if remaining > 0 {
            queue!(stdout, Print(" ".repeat(remaining)))?;
        }
        print_table_bar(stdout)?;
    }
    let bottom = format!("└{}┘", "─".repeat(inner));
    render_line(stdout, y + 1 + rows.len() as u16, &bottom, width)?;
    Ok(())
}

fn render_group_name_cell(stdout: &mut io::Stdout, label: &str, width: usize) -> io::Result<()> {
    let display = truncate_str(label, width);
    let prefix = "group: ";
    let prefix_len = prefix.chars().count();
    let display_len = display.chars().count();
    let prefix_visible = prefix_len.min(display_len);
    let (prefix_part, name_part) = split_at_chars(&display, prefix_visible);

    queue!(stdout, Print(prefix_part))?;
    if !name_part.is_empty() {
        queue!(stdout, SetForegroundColor(Color::Yellow), Print(name_part), ResetColor)?;
    }

    let remaining = width.saturating_sub(display_len);
    if remaining > 0 {
        queue!(stdout, Print(" ".repeat(remaining)))?;
    }
    Ok(())
}

fn print_table_bar(stdout: &mut io::Stdout) -> io::Result<()> {
    queue!(
        stdout,
        SetForegroundColor(Color::DarkGrey),
        Print("│"),
        ResetColor
    )?;
    Ok(())
}


pub(super) fn clear_list_area(
    stdout: &mut io::Stdout,
    start: usize,
    count: usize,
    width: usize,
) -> io::Result<()> {
    for idx in 0..count {
        render_line(stdout, (start + idx) as u16, "", width)?;
    }
    Ok(())
}

pub(super) fn fit_left(text: &str, width: usize) -> String {
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

fn fit_right(text: &str, width: usize) -> String {
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

pub(super) struct HelpSegment {
    text: String,
    color: Option<Color>,
}

impl HelpSegment {
    fn plain(text: &str) -> Self {
        Self {
            text: text.to_string(),
            color: None,
        }
    }

    fn key(text: &str) -> Self {
        Self {
            text: text.to_string(),
            color: Some(Color::Cyan),
        }
    }
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

fn docker_column_widths(width: usize) -> Vec<usize> {
    let fixed = 12 + 6 + 9;
    let separators = 9usize;
    let content_width = width.saturating_sub(separators);
    let remaining = content_width.saturating_sub(fixed);
    let min_name = 10usize;
    let min_image = 10usize;
    let min_ports = 8usize;
    let min_int_ports = 8usize;
    let min_status = 10usize;

    let (name_width, image_width, ports_width, int_ports_width, status_width) = if remaining
        >= min_name + min_image + min_ports + min_int_ports + min_status
    {
        let mut name_width = remaining * 3 / 12;
        let mut image_width = remaining * 3 / 12;
        let mut ports_width = remaining * 2 / 12;
        let mut int_ports_width = remaining * 2 / 12;
        let mut status_width =
            remaining - name_width - image_width - ports_width - int_ports_width;
        if status_width < min_status {
            status_width = min_status;
            let leftover = remaining - status_width;
            name_width = leftover * 3 / 9;
            image_width = leftover * 3 / 9;
            ports_width = leftover * 2 / 9;
            int_ports_width = leftover - name_width - image_width - ports_width;
        }
        if name_width < min_name {
            name_width = min_name;
            let leftover = remaining - name_width;
            image_width = leftover * 3 / 8;
            ports_width = leftover * 2 / 8;
            int_ports_width = leftover * 2 / 8;
            status_width = leftover - image_width - ports_width - int_ports_width;
        }
        if image_width < min_image {
            image_width = min_image;
            let leftover = remaining - image_width;
            name_width = leftover * 3 / 8;
            ports_width = leftover * 2 / 8;
            int_ports_width = leftover * 2 / 8;
            status_width = leftover - name_width - ports_width - int_ports_width;
        }
        if ports_width < min_ports {
            ports_width = min_ports;
            let leftover = remaining - ports_width;
            name_width = leftover * 3 / 8;
            image_width = leftover * 3 / 8;
            int_ports_width = leftover * 2 / 8;
            status_width = leftover - name_width - image_width - int_ports_width;
        }
        if int_ports_width < min_int_ports {
            int_ports_width = min_int_ports;
            let leftover = remaining - int_ports_width;
            name_width = leftover * 3 / 8;
            image_width = leftover * 3 / 8;
            ports_width = leftover * 2 / 8;
            status_width = leftover - name_width - image_width - ports_width;
        }
        (
            name_width,
            image_width,
            ports_width,
            int_ports_width,
            status_width,
        )
    } else {
        let mut name_width = remaining;
        let mut image_width = 0usize;
        let mut ports_width = 0usize;
        let mut int_ports_width = 0usize;
        let mut status_width = 0usize;
        if name_width >= min_name + min_image + min_ports + min_int_ports + min_status {
            name_width = min_name;
            image_width = min_image;
            ports_width = min_ports;
            int_ports_width = min_int_ports;
            status_width = remaining - name_width - image_width - ports_width - int_ports_width;
        }
        (name_width, image_width, ports_width, int_ports_width, status_width)
    };

    vec![
        12,
        6,
        9,
        name_width,
        image_width,
        ports_width,
        int_ports_width,
        status_width,
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

fn format_docker_header(widths: &[usize]) -> String {
    let cells = vec![
        fit_right("CONTAINER", widths[0]),
        fit_right("CPU%", widths[1]),
        fit_right("MEM(GB)", widths[2]),
        fit_left("NAME", widths[3]),
        fit_left("IMAGE", widths[4]),
        fit_left("PORT", widths[5]),
        fit_left("INT PORT", widths[6]),
        fit_left("STATUS", widths[7]),
    ];
    format!(
        "│{}│{}│{}│{}│{}│{}│{}│{}│",
        cells[0], cells[1], cells[2], cells[3], cells[4], cells[5], cells[6], cells[7]
    )
}

pub(super) fn format_separator(widths: &[usize]) -> String {
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

pub(super) fn format_top_border(widths: &[usize]) -> String {
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

fn format_memory_bar(label: &str, total: u64, available: u64, width: usize) -> String {
    let used = total.saturating_sub(available);
    let bar_width = bar_width_for_screen(width);
    let bar = progress_bar(used, total, bar_width);
    let used_gb = bytes_to_gb(used);
    let total_gb = bytes_to_gb(total);
    let free_gb = bytes_to_gb(available);
    format!(
        "{label}[{bar}] used {used_gb:.2} / {total_gb:.2} GB (free {free_gb:.2} GB)"
    )
}

fn format_cpu_bar(label: &str, usage: f32, width: usize) -> String {
    let bar_width = bar_width_for_screen(width);
    let ratio = (usage as f64 / 100.0).clamp(0.0, 1.0);
    let bar = progress_bar_ratio(ratio, bar_width);
    format!("{label}[{bar}] {usage:.1}%")
}

fn format_swap_bar(label: &str, total: u64, used: u64, width: usize) -> String {
    let free = total.saturating_sub(used);
    let bar_width = bar_width_for_screen(width);
    let bar = progress_bar(used, total, bar_width);
    let used_gb = bytes_to_gb(used);
    let total_gb = bytes_to_gb(total);
    let free_gb = bytes_to_gb(free);
    format!(
        "{label}[{bar}] used {used_gb:.2} / {total_gb:.2} GB (free {free_gb:.2} GB)"
    )
}

fn progress_bar(used: u64, total: u64, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let ratio = if total == 0 { 0.0 } else { used as f64 / total as f64 };
    progress_bar_ratio(ratio, width)
}

fn progress_bar_ratio(ratio: f64, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let ratio = ratio.clamp(0.0, 1.0);
    let filled = ((ratio * width as f64).round() as usize).min(width);
    let empty = width.saturating_sub(filled);
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

fn bar_width_for_screen(width: usize) -> usize {
    let target = width.saturating_sub(40);
    target.clamp(10, 30)
}

fn bytes_to_gb(bytes: u64) -> f64 {
    bytes as f64 / 1024.0 / 1024.0 / 1024.0
}
