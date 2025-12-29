use std::io::{self, Write};

use crossterm::cursor::MoveTo;
use crossterm::queue;
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor};
use crossterm::terminal;

use crate::app::{AppState, InputMode};
use crate::system::node::{format_uptime, NodeProcessInfo, NodeRow};

use super::bars::{format_cpu_bar, format_memory_bar, format_swap_bar};
use super::layout::{layout_for_screen, render_sidebar, render_sidebar_gap};
use super::table::{
    clear_list_area_at, fit_left, fit_right, format_separator, format_top_border,
    is_dim_mode, print_table_bar, render_help_table_rows_colored_at, render_line_at,
    render_search_box_at, render_title_at, set_dim_mode, truncate_str, HelpSegment,
};

pub fn render_node_processes(
    stdout: &mut io::Stdout,
    state: &AppState,
    processes: &[NodeProcessInfo],
    rows: &[NodeRow],
    pm2_available: bool,
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
    render_title_at(stdout, main_x, row, width_usize, "NODE.JS VIEW")?;
    row += 2;

    let pm2_status = if pm2_available { "PM2: ✓" } else { "PM2: -" };
    let header = format!(
        "Spark | View: NODE | {} | Mode: {}",
        pm2_status, mode_label
    );
    render_line_at(stdout, main_x, row, &header, width_usize)?;
    row += 1;
    render_search_box_at(stdout, main_x, row, width_usize, &state.node_filter)?;
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

    let col_widths = node_column_widths(width_usize, pm2_available);
    let show_main_table = match rows.first() {
        Some(NodeRow::UtilsSpacer)
        | Some(NodeRow::UtilsTitle)
        | Some(NodeRow::UtilsTop)
        | Some(NodeRow::UtilsHeader)
        | Some(NodeRow::UtilsSeparator) => false,
        _ => !rows.is_empty(),
    };
    if show_main_table {
        let top_border = format_top_border(&col_widths);
        render_line_at(stdout, main_x, row, &top_border, width_usize)?;
        row += 1;
        let table_header = format_node_header(&col_widths, pm2_available);
        render_line_at(stdout, main_x, row, &table_header, width_usize)?;
        row += 1;
        let sep = format_separator(&col_widths);
        render_line_at(stdout, main_x, row, &sep, width_usize)?;
        row += 1;
    }

    let list_start = row as usize;
    let footer_lines = 5usize;
    let max_rows = height_usize.saturating_sub(list_start + footer_lines);

    if max_rows > 0 {
        if rows.is_empty() {
            render_line_at(
                stdout,
                main_x,
                list_start as u16,
                "No Node.js processes found.",
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
                let is_selected = line_index == state.selected && !dim;
                let is_hovered = state.hover_row == Some(line_index) && !is_selected && !dim;

                match row {
                    NodeRow::Group { name, count } => {
                        render_node_group_row_at(
                            stdout,
                            main_x,
                            y as u16,
                            &col_widths,
                            name,
                            *count,
                            pm2_available,
                        )?;
                    }
                    NodeRow::Item { index } => {
                        let proc = &processes[*index];
                        let is_last_in_group = match rows.get(line_index + 1) {
                            None => true,
                            Some(NodeRow::Group { .. })
                            | Some(NodeRow::UtilsSpacer)
                            | Some(NodeRow::UtilsTitle)
                            | Some(NodeRow::UtilsTop)
                            | Some(NodeRow::UtilsHeader)
                            | Some(NodeRow::UtilsSeparator) => true,
                            Some(NodeRow::Item { .. }) => false,
                        };
                        let prefix = if is_last_in_group { "└─ " } else { "├─ " };
                        let name = format!("{}{}", prefix, node_display_name(proc));
                        render_node_line(
                            stdout,
                            main_x,
                            y as u16,
                            width_usize,
                            proc,
                            &col_widths,
                            pm2_available,
                            &name,
                            is_selected,
                            is_hovered,
                        )?;
                    }
                    NodeRow::UtilsSpacer => {
                        render_line_at(stdout, main_x, y as u16, "", width_usize)?;
                    }
                    NodeRow::UtilsTitle => {
                        render_line_at(stdout, main_x, y as u16, "UTILS", width_usize)?;
                    }
                    NodeRow::UtilsTop => {
                        let top_border = format_top_border(&col_widths);
                        render_line_at(stdout, main_x, y as u16, &top_border, width_usize)?;
                    }
                    NodeRow::UtilsHeader => {
                        let table_header = format_node_header(&col_widths, pm2_available);
                        render_line_at(stdout, main_x, y as u16, &table_header, width_usize)?;
                    }
                    NodeRow::UtilsSeparator => {
                        let sep = format_separator(&col_widths);
                        render_line_at(stdout, main_x, y as u16, &sep, width_usize)?;
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
        let message = state.message.as_deref().unwrap_or("");
        render_line_at(stdout, main_x, message_line, message, width_usize)?;

        let clear_key_active = HelpSegment::highlight("x");
        let clear_key_default = HelpSegment::key("x");
        let clear_label_active = HelpSegment::highlight(" clear search");
        let clear_label_default = HelpSegment::plain(" clear search");
        let clear_key =
            if state.input_mode == InputMode::Normal && !state.node_filter.is_empty() {
                clear_key_active
            } else {
                clear_key_default
            };
        let clear_label =
            if state.input_mode == InputMode::Normal && !state.node_filter.is_empty() {
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
                HelpSegment::key("e"),
                HelpSegment::plain(" env | "),
                HelpSegment::key("k"),
                HelpSegment::plain(" kill | "),
                HelpSegment::key("q"),
                HelpSegment::plain(" quit | "),
                HelpSegment::key("arrows"),
                HelpSegment::plain(" move"),
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

fn node_column_widths(width: usize, pm2_available: bool) -> Vec<usize> {
    if pm2_available {
        // PM2 mode: ID | Name | Status | PID | CPU | Memory | Restarts | Uptime | Project | Script
        let id_width = 3usize;
        let status_width = 8usize;
        let pid_width = 7usize;
        let cpu_width = 6usize;
        let mem_width = 9usize;
        let restarts_width = 4usize;
        let uptime_width = 8usize;
        let separators = 11usize;
        let fixed_width = id_width + status_width + pid_width + cpu_width + mem_width + restarts_width + uptime_width;
        let remaining = width.saturating_sub(separators + fixed_width);

        let min_name = 8usize;
        let min_project = 8usize;
        let min_script = 10usize;
        let mut name_width = (remaining * 2 / 7).max(min_name);
        let mut project_width = (remaining * 2 / 7).max(min_project);
        let mut script_width = remaining.saturating_sub(name_width + project_width).max(min_script);
        if name_width + project_width + script_width > remaining {
            script_width = remaining.saturating_sub(name_width + project_width);
        }
        if script_width < min_script {
            script_width = min_script;
            let leftover = remaining.saturating_sub(script_width);
            name_width = (leftover * 2 / 4).max(min_name);
            project_width = leftover.saturating_sub(name_width).max(min_project);
        }

        vec![
            id_width,
            name_width,
            status_width,
            pid_width,
            cpu_width,
            mem_width,
            restarts_width,
            uptime_width,
            project_width,
            script_width,
        ]
    } else {
        // Non-PM2 mode: PID | Name | CPU | Memory | Version | Uptime | Project | Script
        let pid_width = 7usize;
        let cpu_width = 6usize;
        let mem_width = 9usize;
        let version_width = 10usize;
        let uptime_width = 8usize;
        let separators = 9usize;
        let fixed_width = pid_width + cpu_width + mem_width + version_width + uptime_width;
        let remaining = width.saturating_sub(separators + fixed_width);

        let min_name = 8usize;
        let min_project = 8usize;
        let min_script = 10usize;
        let mut name_width = (remaining * 2 / 7).max(min_name);
        let mut project_width = (remaining * 2 / 7).max(min_project);
        let mut script_width = remaining.saturating_sub(name_width + project_width).max(min_script);
        if name_width + project_width + script_width > remaining {
            script_width = remaining.saturating_sub(name_width + project_width);
        }
        if script_width < min_script {
            script_width = min_script;
            let leftover = remaining.saturating_sub(script_width);
            name_width = (leftover * 2 / 4).max(min_name);
            project_width = leftover.saturating_sub(name_width).max(min_project);
        }

        vec![
            pid_width,
            name_width,
            cpu_width,
            mem_width,
            version_width,
            uptime_width,
            project_width,
            script_width,
        ]
    }
}

fn format_node_header(widths: &[usize], pm2_available: bool) -> String {
    if pm2_available {
        let cells = vec![
            fit_right("ID", widths[0]),
            fit_left("NAME", widths[1]),
            fit_left("STATUS", widths[2]),
            fit_right("PID", widths[3]),
            fit_right("CPU%", widths[4]),
            fit_right("MEMORY", widths[5]),
            fit_right("RST", widths[6]),
            fit_left("UPTIME", widths[7]),
            fit_left("PROJECT", widths[8]),
            fit_left("SCRIPT", widths[9]),
        ];
        format!(
            "│{}│{}│{}│{}│{}│{}│{}│{}│{}│{}│",
            cells[0],
            cells[1],
            cells[2],
            cells[3],
            cells[4],
            cells[5],
            cells[6],
            cells[7],
            cells[8],
            cells[9]
        )
    } else {
        let cells = vec![
            fit_right("PID", widths[0]),
            fit_left("NAME", widths[1]),
            fit_right("CPU%", widths[2]),
            fit_right("MEMORY", widths[3]),
            fit_left("VERSION", widths[4]),
            fit_left("UPTIME", widths[5]),
            fit_left("PROJECT", widths[6]),
            fit_left("SCRIPT", widths[7]),
        ];
        format!(
            "│{}│{}│{}│{}│{}│{}│{}│{}│",
            cells[0], cells[1], cells[2], cells[3], cells[4], cells[5], cells[6], cells[7]
        )
    }
}

fn render_node_line(
    stdout: &mut io::Stdout,
    x: u16,
    y: u16,
    width: usize,
    proc: &NodeProcessInfo,
    widths: &[usize],
    pm2_available: bool,
    display_name: &str,
    selected: bool,
    hovered: bool,
) -> io::Result<()> {
    queue!(stdout, MoveTo(x, y))?;

    let project_display = proc.project_name.as_deref().unwrap_or("-");

    if pm2_available {
        // PM2 mode
        let pm2 = proc.pm2.as_ref();
        let id_cell = pm2.map_or("-".to_string(), |p| p.pm_id.to_string());
        let status = pm2.map_or("-", |p| p.status.as_str());
        let restarts = pm2.map_or(0, |p| p.restarts);
        let pid_str = if proc.pid.as_u32() == 0 {
            "-".to_string()
        } else {
            proc.pid.to_string()
        };

        let id_cell = fit_right(&id_cell, widths[0]);
        let name_cell = fit_left(display_name, widths[1]);
        let status_cell = fit_left(status, widths[2]);
        let pid_cell = fit_right(&pid_str, widths[3]);
        let cpu_cell = fit_right(&format!("{:.1}", proc.cpu), widths[4]);
        let mem_cell = fit_right(&format_memory(proc.memory_bytes), widths[5]);
        let restarts_cell = fit_right(&restarts.to_string(), widths[6]);
        let uptime_secs = pm2
            .and_then(|info| info.pm2_uptime)
            .or(proc.uptime_secs);
        let uptime_cell = fit_left(&format_uptime(uptime_secs), widths[7]);
        let project_cell = fit_left(project_display, widths[8]);
        let script_cell = fit_left(&proc.script, widths[9]);

        let line = format!(
            "│{}│{}│{}│{}│{}│{}│{}│{}│{}│{}│",
            id_cell,
            name_cell,
            status_cell,
            pid_cell,
            cpu_cell,
            mem_cell,
            restarts_cell,
            uptime_cell,
            project_cell,
            script_cell,
        );

        if selected {
            queue!(
                stdout,
                SetAttribute(Attribute::Reverse),
                Print(fit_left(&line, width)),
                SetAttribute(Attribute::Reset)
            )?;
            return Ok(());
        }

        if hovered {
            queue!(
                stdout,
                SetBackgroundColor(Color::DarkGrey),
                Print(fit_left(&line, width)),
                ResetColor
            )?;
            return Ok(());
        }

        print_table_bar(stdout)?;
        print_dim_cell(stdout, &id_cell)?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &name_cell)?;
        print_table_bar(stdout)?;
        render_status_cell(stdout, status, widths[2])?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &pid_cell)?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &cpu_cell)?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &mem_cell)?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &restarts_cell)?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &uptime_cell)?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &project_cell)?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &script_cell)?;
        print_table_bar(stdout)?;
        return Ok(());
    }

    // Non-PM2 mode
    let version = proc.node_version.as_deref().unwrap_or("-");
    let pid_cell = fit_right(&proc.pid.to_string(), widths[0]);
    let name_cell = fit_left(display_name, widths[1]);
    let cpu_cell = fit_right(&format!("{:.1}", proc.cpu), widths[2]);
    let mem_cell = fit_right(&format_memory(proc.memory_bytes), widths[3]);
    let version_cell = fit_left(version, widths[4]);
    let uptime_cell = fit_left(&format_uptime(proc.uptime_secs), widths[5]);
    let project_cell = fit_left(project_display, widths[6]);
    let script_cell = fit_left(&proc.script, widths[7]);

    let line = format!(
        "│{}│{}│{}│{}│{}│{}│{}│{}│",
        pid_cell,
        name_cell,
        cpu_cell,
        mem_cell,
        version_cell,
        uptime_cell,
        project_cell,
        script_cell,
    );

    if selected {
        queue!(
            stdout,
            SetAttribute(Attribute::Reverse),
            Print(fit_left(&line, width)),
            SetAttribute(Attribute::Reset)
        )?;
        return Ok(());
    }

    if hovered {
        queue!(
            stdout,
            SetBackgroundColor(Color::DarkGrey),
            Print(fit_left(&line, width)),
            ResetColor
        )?;
        return Ok(());
    }

    print_table_bar(stdout)?;
    print_dim_cell(stdout, &pid_cell)?;
    print_table_bar(stdout)?;
    print_dim_cell(stdout, &name_cell)?;
    print_table_bar(stdout)?;
    print_dim_cell(stdout, &cpu_cell)?;
    print_table_bar(stdout)?;
    print_dim_cell(stdout, &mem_cell)?;
    print_table_bar(stdout)?;
    print_dim_cell(stdout, &version_cell)?;
    print_table_bar(stdout)?;
    print_dim_cell(stdout, &uptime_cell)?;
    print_table_bar(stdout)?;
    print_dim_cell(stdout, &project_cell)?;
    print_table_bar(stdout)?;
    print_dim_cell(stdout, &script_cell)?;
    print_table_bar(stdout)?;
    Ok(())
}

fn node_display_name(proc: &NodeProcessInfo) -> String {
    let base_name = proc
        .pm2
        .as_ref()
        .map(|pm2| pm2.name.as_str())
        .unwrap_or(proc.name.as_str());
    if proc.worker_count > 1 {
        format!("{} (x{})", base_name, proc.worker_count)
    } else {
        base_name.to_string()
    }
}


fn render_status_cell(stdout: &mut io::Stdout, status: &str, width: usize) -> io::Result<()> {
    let cell = fit_left(status, width);
    if is_dim_mode() {
        return print_dim_cell(stdout, &cell);
    }

    let color = match status {
        "online" => Color::Green,
        "stopping" | "launching" => Color::Yellow,
        "stopped" | "errored" => Color::Red,
        _ => Color::Reset,
    };

    if color != Color::Reset {
        queue!(
            stdout,
            SetForegroundColor(color),
            Print(cell),
            ResetColor
        )?;
    } else {
        queue!(stdout, Print(cell))?;
    }
    Ok(())
}

fn render_node_group_row_at(
    stdout: &mut io::Stdout,
    x: u16,
    y: u16,
    widths: &[usize],
    name: &str,
    count: usize,
    pm2_available: bool,
) -> io::Result<()> {
    let count_label = format!("{count} procs");
    let label = truncate_str(name, widths[1]);

    queue!(stdout, MoveTo(x, y))?;
    if pm2_available {
        let id_cell = fit_right("", widths[0]);
        let status_cell = fit_left("", widths[2]);
        let pid_cell = fit_right("", widths[3]);
        let cpu_cell = fit_right("", widths[4]);
        let mem_cell = fit_right("", widths[5]);
        let restarts_cell = fit_right("", widths[6]);
        let uptime_cell = fit_left("", widths[7]);
        let project_cell = fit_left("-", widths[8]);
        let script_cell = fit_left(&count_label, widths[9]);

        print_table_bar(stdout)?;
        print_dim_cell(stdout, &id_cell)?;
        print_table_bar(stdout)?;
        render_group_name_cell(stdout, &label, widths[1])?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &status_cell)?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &pid_cell)?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &cpu_cell)?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &mem_cell)?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &restarts_cell)?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &uptime_cell)?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &project_cell)?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &script_cell)?;
        print_table_bar(stdout)?;
    } else {
        let pid_cell = fit_right("", widths[0]);
        let cpu_cell = fit_right("", widths[2]);
        let mem_cell = fit_right("", widths[3]);
        let version_cell = fit_left("", widths[4]);
        let uptime_cell = fit_left("", widths[5]);
        let project_cell = fit_left("-", widths[6]);
        let script_cell = fit_left(&count_label, widths[7]);

        print_table_bar(stdout)?;
        print_dim_cell(stdout, &pid_cell)?;
        print_table_bar(stdout)?;
        render_group_name_cell(stdout, &label, widths[1])?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &cpu_cell)?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &mem_cell)?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &version_cell)?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &uptime_cell)?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &project_cell)?;
        print_table_bar(stdout)?;
        print_dim_cell(stdout, &script_cell)?;
        print_table_bar(stdout)?;
    }

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

fn format_memory(bytes: u64) -> String {
    if bytes == 0 {
        return "-".to_string();
    }

    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1}GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1}MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0}KB", bytes as f64 / KB as f64)
    } else {
        format!("{}B", bytes)
    }
}
