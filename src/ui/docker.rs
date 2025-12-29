use std::io::{self, Write};

use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::{
    Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::terminal;

use crate::app::{AppState, InputMode, SortBy, SortOrder};
use crate::system::docker::{ContainerInfo, DockerRow};

use super::bars::{format_cpu_bar, format_memory_bar, format_swap_bar};
use super::layout::{layout_for_screen, render_sidebar, render_sidebar_gap};
use super::table::{
    clear_list_area_at, fit_left, fit_right, format_separator, format_top_border, is_dim_mode,
    render_help_table_rows_colored_at, render_line_at, render_search_box_at, render_title_at,
    set_dim_mode, split_at_chars, truncate_str, HelpSegment, print_table_bar,
};

pub fn render_containers(
    stdout: &mut io::Stdout,
    state: &AppState,
    containers: &[ContainerInfo],
    rows: &[DockerRow],
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

    let mut row = 0u16;
    render_line_at(
        stdout,
        main_x,
        row,
        &format!("┌{}┐", "─".repeat(width_usize.saturating_sub(2))),
        width_usize,
    )?;
    row += 1;
    render_title_at(stdout, main_x, row, width_usize, "DOCKER VIEW")?;
    row += 2;

    let header = format!(
        "Spark | View: DOCKER | Sort: {} {} | Mode: {}",
        sort_label, order_label, mode_label
    );
    render_line_at(stdout, main_x, row, &header, width_usize)?;
    row += 1;
    render_search_box_at(stdout, main_x, row, width_usize, &state.docker_filter)?;
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
    let docker_widths = docker_column_widths(width_usize);
    let docker_top = format_top_border(&docker_widths);
    render_line_at(stdout, main_x, row, &docker_top, width_usize)?;
    row += 1;
    let docker_header = format_docker_header(&docker_widths);
    render_line_at(stdout, main_x, row, &docker_header, width_usize)?;
    row += 1;
    let docker_sep = format_separator(&docker_widths);
    render_line_at(stdout, main_x, row, &docker_sep, width_usize)?;
    row += 1;

    let list_start = row as usize;
    let footer_lines = 5usize;
    let max_rows = height_usize.saturating_sub(list_start + footer_lines);

    if max_rows > 0 {
        if rows.is_empty() {
            render_line_at(
                stdout,
                main_x,
                list_start as u16,
                "No running containers or Docker stats unavailable.",
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
            let selected_row = state.docker_selected_row;
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
                let is_selected = line_index == selected_row && !dim;
                match row {
                    DockerRow::Group { name, path, count } => {
                        render_group_row_at(
                            stdout,
                            main_x,
                            y as u16,
                            &docker_widths,
                            name,
                            path.as_deref(),
                            *count,
                            is_selected,
                        )?;
                    }
                    DockerRow::Separator => {
                        let line = format_separator(&docker_widths);
                        render_line_at(stdout, main_x, y as u16, &line, width_usize)?;
                    }
                    DockerRow::Item { index, prefix } => {
                        let Some(container) = containers.get(*index) else {
                            continue;
                        };
                        let line = format_container_line(container, width_usize, prefix);
                        if is_selected {
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
        if !state.docker_filter.is_empty() {
            render_docker_search_status_at(
                stdout,
                main_x,
                message_line,
                width_usize,
                &state.docker_filter,
                state.docker_filtered_out,
            )?;
        } else {
            let message = state.message.as_deref().unwrap_or("");
            render_line_at(stdout, main_x, message_line, message, width_usize)?;
        }

        let clear_key_active = HelpSegment::highlight("x");
        let clear_key_default = HelpSegment::key("x");
        let clear_label_active = HelpSegment::highlight(" clear search");
        let clear_label_default = HelpSegment::plain(" clear search");
        let clear_key = if state.input_mode == InputMode::Normal && !state.docker_filter.is_empty()
        {
            clear_key_active
        } else {
            clear_key_default
        };
        let clear_label = if state.input_mode == InputMode::Normal && !state.docker_filter.is_empty()
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

fn render_group_row_at(
    stdout: &mut io::Stdout,
    x: u16,
    y: u16,
    widths: &[usize],
    name: &str,
    path: Option<&str>,
    count: usize,
    selected: bool,
) -> io::Result<()> {
    let label = format!("{name}");
    let path_label = path.unwrap_or("-");
    let status_label = format!("{count} containers");

    let id_cell = fit_right("", widths[0]);
    let cpu_cell = fit_right("", widths[1]);
    let mem_cell = fit_right("", widths[2]);
    let name_cell = fit_left(&label, widths[3]);
    let image_cell = fit_left(path_label, widths[4]);
    let port_cell = fit_left("-", widths[5]);
    let int_port_cell = fit_left("-", widths[6]);
    let status_cell = fit_left(&status_label, widths[7]);

    if selected {
        let line = format!(
            "│{}│{}│{}│{}│{}│{}│{}│{}│",
            id_cell, cpu_cell, mem_cell, name_cell, image_cell, port_cell, int_port_cell, status_cell
        );
        let total_width: usize = widths.iter().sum::<usize>() + 9; // 9 separators
        queue!(
            stdout,
            MoveTo(x, y),
            SetAttribute(Attribute::Reverse),
            Print(fit_left(&line, total_width)),
            SetAttribute(Attribute::Reset)
        )?;
    } else {
        queue!(stdout, MoveTo(x, y))?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &id_cell)?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &cpu_cell)?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &mem_cell)?;
        print_table_bar(stdout)?;
        render_group_name_cell(stdout, &label, widths[3])?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &image_cell)?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &port_cell)?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &int_port_cell)?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &status_cell)?;
        print_table_bar(stdout)?;
    }
    Ok(())
}

fn render_group_name_cell(stdout: &mut io::Stdout, label: &str, width: usize) -> io::Result<()> {
    let display = truncate_str(label, width);
    if is_dim_mode() {
        let display_len = display.chars().count();
        queue!(
            stdout,
            SetForegroundColor(Color::DarkGrey),
            Print(&display),
            ResetColor
        )?;
        let remaining = width.saturating_sub(display_len);
        if remaining > 0 {
            queue!(stdout, Print(" ".repeat(remaining)))?;
        }
        return Ok(());
    }
    let prefix = "";
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

fn render_docker_search_status_at(
    stdout: &mut io::Stdout,
    x: u16,
    y: u16,
    width: usize,
    term: &str,
    filtered_out: usize,
) -> io::Result<()> {
    if width == 0 {
        return Ok(());
    }

    let prefix = "Search: ";
    let suffix_full = format!(" | {} docker filtered out | x clear search", filtered_out);
    let prefix_len = prefix.chars().count();
    let mut suffix_display = suffix_full.clone();
    let min_term = if term.is_empty() { 0 } else { 1 };

    if prefix_len + suffix_display.chars().count() + min_term > width {
        let max_suffix = width.saturating_sub(prefix_len + min_term);
        suffix_display = truncate_str(&suffix_full, max_suffix);
    }

    let suffix_len = suffix_display.chars().count();
    let available = width.saturating_sub(prefix_len + suffix_len);
    let term_display = truncate_str(term, available);
    let term_len = term_display.chars().count();

    if is_dim_mode() {
        queue!(
            stdout,
            MoveTo(x, y),
            SetForegroundColor(Color::DarkGrey),
            Print(prefix),
            ResetColor
        )?;
    } else {
        queue!(stdout, MoveTo(x, y), Print(prefix))?;
    }
    if is_dim_mode() {
        queue!(
            stdout,
            SetForegroundColor(Color::DarkGrey),
            Print(term_display),
            Print(suffix_display),
            ResetColor
        )?;
    } else {
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
        if let Some(pos) = suffix_display.find('x') {
            let (before, rest) = suffix_display.split_at(pos);
            let (x_char, after) = rest.split_at(1);
            queue!(stdout, Print(before))?;
            queue!(
                stdout,
                SetBackgroundColor(Color::Yellow),
                SetForegroundColor(Color::Black),
                Print(x_char),
                ResetColor
            )?;
            queue!(stdout, Print(after))?;
        } else {
            queue!(stdout, Print(suffix_display))?;
        }
    }

    let printed = prefix_len + term_len + suffix_len;
    if printed < width {
        queue!(stdout, Print(" ".repeat(width - printed)))?;
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
