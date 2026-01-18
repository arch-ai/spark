//! Main ratatui render function that dispatches to view-specific renderers

use std::collections::HashMap;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap},
    Frame,
};
use sysinfo::Pid;

use crate::app::{AppState, Focus, InputMode, SortBy, SortOrder, ViewMode};
use crate::system::docker::DockerSystemDf;
use crate::system::{docker, node, ports, process};

use super::widgets::{HelpBar, HelpItem, Sidebar};

const SIDEBAR_WIDTH: u16 = 20;
const MIN_MAIN_WIDTH: u16 = 40;

/// Render navigation icons (▲/▼) in table area corners for jump to top/bottom
fn render_nav_icons(frame: &mut Frame, area: Rect, scroll: usize, total: usize, visible: usize) {
    // Only show if there's content to scroll
    if total <= visible {
        return;
    }

    let can_scroll_up = scroll > 0;
    let can_scroll_down = scroll + visible < total;

    // Top-right corner: ▲ to go to top
    if can_scroll_up {
        let icon_area = Rect::new(area.x + area.width - 3, area.y, 2, 1);
        let style = Style::default().fg(Color::DarkGray);
        frame.render_widget(Paragraph::new("▲").style(style), icon_area);
    }

    // Bottom-right corner: ▼ to go to bottom
    if can_scroll_down {
        let icon_area = Rect::new(area.x + area.width - 3, area.y + area.height - 1, 2, 1);
        let style = Style::default().fg(Color::DarkGray);
        frame.render_widget(Paragraph::new("▼").style(style), icon_area);
    }
}

/// Main render function for the ratatui-based UI
pub fn render_ratatui(
    frame: &mut Frame,
    state: &AppState,
    process_cache: &HashMap<Pid, process::ProcInfo>,
    rows_cache: &[process::TreeRow],
    docker_view: &[docker::ContainerInfo],
    docker_rows: &[docker::DockerRow],
    ports_cache: &[ports::PortInfo],
    ports_rows: &[ports::PortRow],
    node_view: &[node::NodeProcessInfo],
    node_rows: &[node::NodeRow],
    pm2_view: &[node::Pm2Process],
    pm2_rows: &[usize],
) {
    let area = frame.area();

    // Determine if we should show sidebar
    let show_sidebar = area.width >= SIDEBAR_WIDTH + MIN_MAIN_WIDTH;

    let (sidebar_area, main_area) = if show_sidebar {
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(SIDEBAR_WIDTH),
                Constraint::Min(MIN_MAIN_WIDTH),
            ])
            .split(area);
        (Some(chunks[0]), chunks[1])
    } else {
        (None, area)
    };

    // Render sidebar if visible
    if let Some(sidebar_rect) = sidebar_area {
        render_sidebar(frame, state, sidebar_rect);
    }

    // Render main content based on view mode
    match state.view_mode {
        ViewMode::Process => {
            render_process_view(frame, state, main_area, process_cache, rows_cache);
        }
        ViewMode::Docker => {
            render_docker_view(frame, state, main_area, docker_view, docker_rows);
        }
        ViewMode::DockerEnv => {
            render_docker_env_view(frame, state, main_area);
        }
        ViewMode::Ports => {
            render_ports_view(frame, state, main_area, ports_cache, ports_rows);
        }
        ViewMode::Node => {
            render_node_view(frame, state, main_area, node_view, node_rows, pm2_view, pm2_rows);
        }
    }

    // Render context menu if active
    if state.context_menu.is_some() {
        render_context_menu(frame, state, main_area);
    }

    let other_modal_open = state.pending_prune.is_some()
        || state.prune_in_progress.is_some()
        || state.prune_output.is_some()
        || state.log_in_progress.is_some()
        || state.log_output.is_some();
    if other_modal_open || state.env_modal_open {
        render_modal_overlay(frame, frame.area());
    }

    if state.pending_prune.is_some() {
        render_prune_confirm(frame, state, main_area);
    }

    if let Some(label) = state.prune_in_progress.as_deref() {
        render_prune_progress(frame, state, main_area, label);
    }

    if state.prune_output.is_some() {
        render_prune_output(frame, state, main_area);
    }

    if let Some(label) = state.log_in_progress.as_deref() {
        render_log_progress(frame, state, main_area, label);
    }

    if state.log_output.is_some() {
        render_log_output(frame, state, main_area);
    }

    if state.env_modal_open && !other_modal_open {
        render_env_modal(frame, state, main_area);
    }
}

fn render_sidebar(frame: &mut Frame, state: &AppState, area: Rect) {
    let items = vec![
        "Processes",
        "Ports",
        "Docker",
        "Node JS",
    ];

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

    let sidebar = Sidebar::new(items)
        .active_index(active_index)
        .selected_index(state.sidebar_index)
        .hover_index(state.sidebar_hover)
        .has_focus(state.focus == Focus::Sidebar);

    frame.render_widget(sidebar, area);
}

fn render_process_view(
    frame: &mut Frame,
    state: &AppState,
    area: Rect,
    processes: &HashMap<Pid, process::ProcInfo>,
    rows: &[process::TreeRow],
) {
    // Layout: title, header, search, bars, table, help
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Title block
            Constraint::Length(1),  // Header info
            Constraint::Length(3),  // Search box
            Constraint::Length(4),  // System bars
            Constraint::Min(5),     // Process table
            Constraint::Length(2),  // Help bar
        ])
        .split(area);

    // Title block
    let title = Block::default()
        .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
        .title(" PROCESS VIEW ");
    frame.render_widget(title, chunks[0]);

    // Header info
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

    let header_text = format!(
        "Spark | View: PROC | Sort: {} {} | Zoom: {} | Mode: {}",
        sort_label, order_label, zoom_label, mode_label
    );
    let header = Paragraph::new(header_text);
    frame.render_widget(header, chunks[1]);

    // Search box
    render_search_box(frame, chunks[2], &state.process_filter, state.input_mode == InputMode::Filter);

    // System bars - constrain to max 60 chars width
    let bars_area = chunks[3];
    let bars_width = bars_area.width.min(60);
    let bars_rect = Rect::new(bars_area.x, bars_area.y, bars_width, bars_area.height);
    render_system_bars(frame, state, bars_rect);

    // Process table
    render_process_table(frame, state, chunks[4], processes, rows);

    // Help bar
    let help_items = vec![
        vec![
            HelpItem::key("j/k"),
            HelpItem::plain(" nav "),
            HelpItem::key("Enter"),
            HelpItem::plain(" kill "),
            HelpItem::key("/"),
            HelpItem::plain(" filter "),
            HelpItem::key("s"),
            HelpItem::plain(" sort "),
            HelpItem::key("z"),
            HelpItem::plain(" zoom "),
            HelpItem::key("q"),
            HelpItem::plain(" quit"),
        ],
    ];
    let help_bar = HelpBar::new(help_items);
    frame.render_widget(help_bar, chunks[5]);
}

fn render_process_table(
    frame: &mut Frame,
    state: &AppState,
    area: Rect,
    processes: &HashMap<Pid, process::ProcInfo>,
    rows: &[process::TreeRow],
) {
    // Calculate max user length for column width
    let mut max_user_len = 4usize;
    for row in rows {
        if let Some(proc) = processes.get(&row.pid) {
            max_user_len = max_user_len.max(proc.user.len());
        }
    }
    max_user_len = max_user_len.min(12);

    // Build table rows
    let visible_height = area.height.saturating_sub(2) as usize; // Account for header and borders
    let scroll_offset = state.process_scroll;

    let table_rows: Vec<Row> = rows
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(visible_height)
        .map(|(idx, tree_row)| {
            // idx is already the absolute index since enumerate() is before skip()
            let is_selected = idx == state.selected;
            let is_hovered = state.hover_row == Some(idx);

            if let Some(proc) = processes.get(&tree_row.pid) {
                let name = format!("{}{}", tree_row.prefix, proc.name);

                let style = if is_selected {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else if is_hovered {
                    // Subtle hover background
                    Style::default().bg(Color::Rgb(40, 40, 45))
                } else {
                    Style::default()
                };

                Row::new(vec![
                    Cell::from(format!("{:>7}", tree_row.pid.as_u32())),
                    Cell::from(format!("{:>5.1}%", proc.cpu)),
                    Cell::from(format!("{:>6}", format_memory(proc.memory_bytes))),
                    Cell::from(format!("{:<width$}", proc.user, width = max_user_len)),
                    Cell::from(name),
                ])
                .style(style)
            } else {
                Row::new(vec![Cell::from(""), Cell::from(""), Cell::from(""), Cell::from(""), Cell::from("")])
            }
        })
        .collect();

    let header = Row::new(vec![
        Cell::from("PID").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("CPU").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("MEM").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("USER").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("NAME").style(Style::default().add_modifier(Modifier::BOLD)),
    ]);

    let table = Table::new(
        table_rows,
        [
            Constraint::Length(8),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(max_user_len as u16 + 1),
            Constraint::Min(20),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL));

    frame.render_widget(table, area);

    // Navigation icons
    render_nav_icons(frame, area, scroll_offset, rows.len(), visible_height);
}

fn render_docker_view(
    frame: &mut Frame,
    state: &AppState,
    area: Rect,
    docker_view: &[docker::ContainerInfo],
    docker_rows: &[docker::DockerRow],
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),  // Title
            Constraint::Length(1),  // Header
            Constraint::Length(3),  // Search
            Constraint::Length(7),  // Docker system df stats (header + 4 rows + 2 for borders)
            Constraint::Min(5),     // Table
            Constraint::Length(2),  // Help
        ])
        .split(area);

    // Title
    let title = Block::default()
        .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
        .title(" DOCKER VIEW ");
    frame.render_widget(title, chunks[0]);

    // Header
    let header_text = format!(
        "Containers: {} | Filtered out: {}",
        state.docker_total, state.docker_filtered_out
    );
    frame.render_widget(Paragraph::new(header_text), chunks[1]);

    // Search
    render_search_box(frame, chunks[2], &state.docker_filter, state.input_mode == InputMode::Filter);

    // Docker system df stats
    let df_area = chunks[3];
    render_docker_df_stats(frame, state, df_area, &state.docker_system_df);

    // Table
    render_docker_table(frame, state, chunks[4], docker_view, docker_rows);

    // Help
    let help_items = vec![
        vec![
            HelpItem::key("j/k"),
            HelpItem::plain(" nav "),
            HelpItem::key("e"),
            HelpItem::plain(" env "),
            HelpItem::key("Enter"),
            HelpItem::plain(" stop/start "),
            HelpItem::key("r"),
            HelpItem::plain(" restart "),
            HelpItem::key("a"),
            HelpItem::plain(" show "),
        ],
        vec![
            HelpItem::key("Right click"),
            HelpItem::plain(" DF prune "),
            HelpItem::key("Ctrl+B"),
            HelpItem::plain(" cache "),
            HelpItem::key("Ctrl+I"),
            HelpItem::plain(" images "),
            HelpItem::key("Ctrl+O"),
            HelpItem::plain(" volumes "),
            HelpItem::key("y/n"),
            HelpItem::plain(" confirm"),
        ],
    ];
    frame.render_widget(HelpBar::new(help_items), chunks[5]);
}

fn render_docker_table(
    frame: &mut Frame,
    state: &AppState,
    area: Rect,
    docker_view: &[docker::ContainerInfo],
    docker_rows: &[docker::DockerRow],
) {
    let visible_height = area.height.saturating_sub(2) as usize;
    let scroll_offset = state.docker_scroll;

    let table_rows: Vec<Row> = docker_rows
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(visible_height)
        .map(|(idx, row)| {
            // idx is already the absolute index since enumerate() is before skip()
            let is_selected = idx == state.docker_selected_row;
            let is_hovered = state.hover_row == Some(idx);

            match row {
                docker::DockerRow::Group { name, count, running_count, .. } => {
                    // Determine group status icon based on running containers
                    let (status_icon, status_color) = if *running_count == *count {
                        ("●", Color::Green) // All running
                    } else if *running_count > 0 {
                        ("◐", Color::Yellow) // Some running
                    } else {
                        ("○", Color::DarkGray) // None running
                    };

                    // Subtle hover background for group rows
                    let row_style = if is_hovered {
                        Style::default().bg(Color::Rgb(40, 40, 45))
                    } else {
                        Style::default()
                    };

                    Row::new(vec![
                        Cell::from(status_icon).style(Style::default().fg(status_color)),
                        Cell::from(name.clone()).style(Style::default().fg(Color::Rgb(255, 191, 0)).add_modifier(Modifier::BOLD)),
                        Cell::from(""),
                        Cell::from(""),
                        Cell::from(format!("{}/{}", running_count, count)).style(Style::default().fg(Color::DarkGray)),
                    ])
                    .style(row_style)
                }
                docker::DockerRow::Separator => {
                    Row::new(vec![Cell::from(""), Cell::from(""), Cell::from(""), Cell::from(""), Cell::from("")])
                }
                docker::DockerRow::Item { index, prefix } => {
                    let container = &docker_view[*index];
                    let is_loading = state.pending_operations.contains_key(&container.id);
                    let health_color = parse_health_status(&container.status);

                    let base_style = if is_selected {
                        Style::default().add_modifier(Modifier::REVERSED)
                    } else if is_hovered {
                        // Subtle hover background
                        Style::default().bg(Color::Rgb(40, 40, 45))
                    } else if !container.running {
                        Style::default().fg(Color::DarkGray)
                    } else {
                        Style::default()
                    };

                    // Only show spinner when loading, otherwise empty
                    let status_display = if is_loading {
                        state.spinner_char().to_string()
                    } else {
                        String::new()
                    };

                    let status_text = truncate(&container.status, 20);
                    let status_style = if let Some(color) = health_color {
                        Style::default().fg(color)
                    } else {
                        Style::default()
                    };

                    let short_id = if container.id.len() > 12 {
                        &container.id[..12]
                    } else {
                        &container.id
                    };

                    Row::new(vec![
                        Cell::from(status_display).style(Style::default().fg(Color::Cyan)),
                        Cell::from(short_id.to_string()),
                        Cell::from(format!("{}{}", prefix, container.name)),
                        Cell::from(container.image.clone()),
                        Cell::from(container.port_public.clone()),
                        Cell::from(status_text).style(status_style),
                    ])
                    .style(base_style)
                }
            }
        })
        .collect();

    let header = Row::new(vec![
        Cell::from("").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("ID").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("NAME").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("IMAGE").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("PORTS").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("STATUS").style(Style::default().add_modifier(Modifier::BOLD)),
    ]);

    let table = Table::new(
        table_rows,
        [
            Constraint::Length(3),
            Constraint::Length(12),
            Constraint::Percentage(22),
            Constraint::Percentage(28),
            Constraint::Percentage(17),
            Constraint::Percentage(20),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL));

    frame.render_widget(table, area);

    // Navigation icons
    render_nav_icons(frame, area, scroll_offset, docker_rows.len(), visible_height);
}

fn render_docker_env_view(frame: &mut Frame, state: &AppState, area: Rect) {
    use super::widgets::EnvView;

    let env_view = EnvView::new(&state.env_title, &state.env_vars)
        .info(
            &state.env_info_left1,
            &state.env_info_right1,
            &state.env_info_left2,
            &state.env_info_right2,
        )
        .selected(state.env_selected);

    frame.render_widget(env_view, area);
}

fn render_ports_view(
    frame: &mut Frame,
    state: &AppState,
    area: Rect,
    ports_cache: &[ports::PortInfo],
    ports_rows: &[ports::PortRow],
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(2),
        ])
        .split(area);

    // Title
    let title = Block::default()
        .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
        .title(" PORTS VIEW ");
    frame.render_widget(title, chunks[0]);

    // Header
    let header_text = format!("Listening ports: {}", ports_cache.len());
    frame.render_widget(Paragraph::new(header_text), chunks[1]);

    // Search
    render_search_box(frame, chunks[2], &state.ports_filter, state.input_mode == InputMode::Filter);

    // Table
    render_ports_table(frame, state, chunks[3], ports_cache, ports_rows);

    // Help
    let help_items = vec![
        vec![
            HelpItem::key("j/k"),
            HelpItem::plain(" nav "),
            HelpItem::key("Enter"),
            HelpItem::plain(" kill "),
            HelpItem::key("/"),
            HelpItem::plain(" filter "),
        ],
    ];
    frame.render_widget(HelpBar::new(help_items), chunks[4]);
}

fn render_ports_table(
    frame: &mut Frame,
    state: &AppState,
    area: Rect,
    ports_cache: &[ports::PortInfo],
    ports_rows: &[ports::PortRow],
) {
    let visible_height = area.height.saturating_sub(2) as usize;
    let scroll_offset = state.ports_scroll;

    let table_rows: Vec<Row> = ports_rows
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(visible_height)
        .map(|(idx, row)| {
            // idx is already the absolute index since enumerate() is before skip()
            let is_selected = idx == state.selected;
            let is_hovered = state.hover_row == Some(idx);

            match row {
                ports::PortRow::Group { name } => {
                    // Subtle hover background for group rows
                    let row_style = if is_hovered {
                        Style::default().bg(Color::Rgb(40, 40, 45))
                    } else {
                        Style::default()
                    };

                    Row::new(vec![
                        Cell::from(name.clone()).style(Style::default().fg(Color::Rgb(255, 191, 0)).add_modifier(Modifier::BOLD)),
                        Cell::from(""),
                        Cell::from(""),
                        Cell::from(""),
                    ])
                    .style(row_style)
                }
                ports::PortRow::Item { index, prefix } => {
                    let port = &ports_cache[*index];
                    let style = if is_selected {
                        Style::default().add_modifier(Modifier::REVERSED)
                    } else if is_hovered {
                        // Subtle hover background
                        Style::default().bg(Color::Rgb(40, 40, 45))
                    } else {
                        Style::default()
                    };

                    let pid_display = if port.pid.as_u32() == 0 {
                        "-".to_string()
                    } else {
                        format!("{}", port.pid.as_u32())
                    };

                    Row::new(vec![
                        Cell::from(format!("{}{}", prefix, port.binding_display())),
                        Cell::from(pid_display),
                        Cell::from(port.name.clone()),
                        Cell::from(port.exe_path.clone()),
                    ])
                    .style(style)
                }
            }
        })
        .collect();

    let header = Row::new(vec![
        Cell::from("EXT:INT").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("PID").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("NAME").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("COMMAND").style(Style::default().add_modifier(Modifier::BOLD)),
    ]);

    let table = Table::new(
        table_rows,
        [
            Constraint::Length(12),
            Constraint::Length(8),
            Constraint::Percentage(30),
            Constraint::Min(20),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL));

    frame.render_widget(table, area);

    // Navigation icons
    render_nav_icons(frame, area, scroll_offset, ports_rows.len(), visible_height);
}

fn render_node_view(
    frame: &mut Frame,
    state: &AppState,
    area: Rect,
    node_view: &[node::NodeProcessInfo],
    node_rows: &[node::NodeRow],
    pm2_view: &[node::Pm2Process],
    pm2_rows: &[usize],
) {
    let title_height = 3u16;
    let header_height = 1u16;
    let search_height = 3u16;
    let help_height = 2u16;

    let title_area = Rect::new(area.x, area.y, area.width, title_height);
    let header_area = Rect::new(area.x, area.y + title_height, area.width, header_height);
    let search_area = Rect::new(
        area.x,
        area.y + title_height + header_height,
        area.width,
        search_height,
    );
    let table_top = area.y + title_height + header_height + search_height;
    let available = area.height.saturating_sub(title_height + header_height + search_height + help_height);
    let help_area = Rect::new(
        area.x,
        area.y + area.height.saturating_sub(help_height),
        area.width,
        help_height,
    );

    let (pm2_area, node_area) = if state.pm2_available {
        let mut pm2_height = available / 2;
        if pm2_height < 5 {
            pm2_height = available.min(5);
        }
        let mut node_height = available.saturating_sub(pm2_height);
        if node_height < 5 {
            let deficit = 5u16.saturating_sub(node_height);
            if pm2_height > deficit {
                pm2_height = pm2_height.saturating_sub(deficit);
                node_height = node_height.saturating_add(deficit);
            }
        }
        (
            Rect::new(area.x, table_top, area.width, pm2_height),
            Rect::new(area.x, table_top + pm2_height, area.width, node_height),
        )
    } else {
        (
            Rect::new(area.x, table_top, area.width, 0),
            Rect::new(area.x, table_top, area.width, available),
        )
    };

    // Title
    let title = Block::default()
        .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
        .title(" NODE VIEW ");
    frame.render_widget(title, title_area);

    // Header
    let pm2_status = if state.pm2_available { "available" } else { "not running" };
    let header_text = format!("Node processes: {} | PM2: {}", node_view.len(), pm2_status);
    frame.render_widget(Paragraph::new(header_text), header_area);

    // Search
    render_search_box(frame, search_area, &state.node_filter, state.input_mode == InputMode::Filter);

    // Tables
    if state.pm2_available {
        render_pm2_table(frame, state, pm2_area, pm2_view, pm2_rows);
    }
    render_node_table(frame, state, node_area, node_view, node_rows);

    // Help
    let help_items = if state.pm2_available {
        vec![vec![
            HelpItem::key("j/k"),
            HelpItem::plain(" nav "),
            HelpItem::key("Enter"),
            HelpItem::plain(" kill "),
            HelpItem::key("e"),
            HelpItem::plain(" env "),
            HelpItem::key("Ctrl+R"),
            HelpItem::plain(" pm2 restart "),
            HelpItem::key("Ctrl+S"),
            HelpItem::plain(" stop "),
            HelpItem::key("Ctrl+T"),
            HelpItem::plain(" start "),
            HelpItem::key("Ctrl+O"),
            HelpItem::plain(" open dir"),
        ]]
    } else {
        vec![vec![
            HelpItem::key("j/k"),
            HelpItem::plain(" nav "),
            HelpItem::key("Enter"),
            HelpItem::plain(" kill "),
            HelpItem::key("e"),
            HelpItem::plain(" env "),
        ]]
    };
    frame.render_widget(HelpBar::new(help_items), help_area);
}

fn render_pm2_table(
    frame: &mut Frame,
    state: &AppState,
    area: Rect,
    pm2_view: &[node::Pm2Process],
    pm2_rows: &[usize],
) {
    if area.height < 3 {
        return;
    }

    let visible_height = area.height.saturating_sub(2) as usize;
    let scroll_offset = state.pm2_scroll;

    let table_rows: Vec<Row> = pm2_rows
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(visible_height)
        .map(|(idx, row_index)| {
            let is_hovered = state.pm2_hover_row == Some(idx + scroll_offset);
            let style = if is_hovered {
                Style::default().bg(Color::Rgb(40, 40, 45))
            } else {
                Style::default()
            };
            let proc = &pm2_view[*row_index];
            let status_color = match proc.status.to_lowercase().as_str() {
                "online" => Color::Green,
                "stopped" => Color::Red,
                "errored" => Color::Red,
                "starting" => Color::Yellow,
                _ => Color::Cyan,
            };
            let pid = proc.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".to_string());
            let cpu = proc.cpu.map(|c| format!("{:.1}%", c)).unwrap_or_else(|| "-".to_string());
            let mem = proc.memory_bytes.map(format_memory).unwrap_or_else(|| "-".to_string());
            let uptime = proc
                .uptime_ms
                .map(|ms| format_uptime(ms / 1000))
                .unwrap_or_else(|| "-".to_string());
            Row::new(vec![
                Cell::from(proc.pm_id.to_string()),
                Cell::from(truncate(&proc.name, 20)),
                Cell::from(proc.status.clone()).style(Style::default().fg(status_color)),
                Cell::from(truncate(&proc.mode, 8)),
                Cell::from(pid),
                Cell::from(cpu),
                Cell::from(mem),
                Cell::from(uptime),
            ])
            .style(style)
        })
        .collect();

    let header = Row::new(vec![
        Cell::from("ID").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("NAME").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("STATUS").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("MODE").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("PID").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("CPU").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("MEM").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("UPTIME").style(Style::default().add_modifier(Modifier::BOLD)),
    ]);

    let table = Table::new(
        table_rows,
        [
            Constraint::Length(4),
            Constraint::Percentage(22),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(7),
            Constraint::Length(8),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title(" PM2 "));

    frame.render_widget(table, area);
    render_nav_icons(frame, area, scroll_offset, pm2_rows.len(), visible_height);
}

fn render_node_table(
    frame: &mut Frame,
    state: &AppState,
    area: Rect,
    node_view: &[node::NodeProcessInfo],
    node_rows: &[node::NodeRow],
) {
    if area.height < 3 {
        return;
    }
    let visible_height = area.height.saturating_sub(2) as usize;
    let scroll_offset = state.node_scroll;

    let table_rows: Vec<Row> = node_rows
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(visible_height)
        .map(|(idx, row)| {
            // idx is already the absolute index since enumerate() is before skip()
            let is_selected = idx == state.selected;
            let is_hovered = state.hover_row == Some(idx);

            match row {
                node::NodeRow::Group { name, count } => {
                    Row::new(vec![
                        Cell::from(format!("{} ({})", name, count))
                            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                        Cell::from(""),
                        Cell::from(""),
                        Cell::from(""),
                        Cell::from(""),
                    ])
                }
                node::NodeRow::Item { index } => {
                    let proc = &node_view[*index];
                    let style = if is_selected {
                        Style::default().add_modifier(Modifier::REVERSED)
                    } else if is_hovered {
                        // Subtle hover background
                        Style::default().bg(Color::Rgb(40, 40, 45))
                    } else {
                        Style::default()
                    };

                    let script_display = if proc.script.is_empty() {
                        proc.project_name.as_deref().unwrap_or("-")
                    } else {
                        &proc.script
                    };

                    Row::new(vec![
                        Cell::from(format!("{}", proc.pid.as_u32())),
                        Cell::from(format!("{:.1}%", proc.cpu)),
                        Cell::from(format_memory(proc.memory_bytes)),
                        Cell::from(proc.name.clone()),
                        Cell::from(truncate(script_display, 30)),
                    ])
                    .style(style)
                }
                node::NodeRow::UtilsTitle => {
                    Row::new(vec![
                        Cell::from("--- Node Utilities ---").style(Style::default().fg(Color::Yellow)),
                        Cell::from(""),
                        Cell::from(""),
                        Cell::from(""),
                        Cell::from(""),
                    ])
                }
                node::NodeRow::UtilsHeader => {
                    Row::new(vec![
                        Cell::from("PID").style(Style::default().add_modifier(Modifier::BOLD)),
                        Cell::from("CPU").style(Style::default().add_modifier(Modifier::BOLD)),
                        Cell::from("MEM").style(Style::default().add_modifier(Modifier::BOLD)),
                        Cell::from("NAME").style(Style::default().add_modifier(Modifier::BOLD)),
                        Cell::from("SCRIPT").style(Style::default().add_modifier(Modifier::BOLD)),
                    ])
                }
                _ => Row::new(vec![Cell::from(""), Cell::from(""), Cell::from(""), Cell::from(""), Cell::from("")]),
            }
        })
        .collect();

    let header = Row::new(vec![
        Cell::from("PID").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("CPU").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("MEM").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("NAME").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("SCRIPT").style(Style::default().add_modifier(Modifier::BOLD)),
    ]);

    let table = Table::new(
        table_rows,
        [
            Constraint::Length(8),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Percentage(30),
            Constraint::Min(20),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL));

    frame.render_widget(table, area);

    // Navigation icons
    render_nav_icons(frame, area, scroll_offset, node_rows.len(), visible_height);
}

fn render_search_box(frame: &mut Frame, area: Rect, filter: &str, is_active: bool) {
    let style = if is_active {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    };

    let search_text = if filter.is_empty() {
        "/ to filter...".to_string()
    } else {
        format!("Filter: {}", filter)
    };

    let search = Paragraph::new(search_text)
        .style(style)
        .block(Block::default().borders(Borders::ALL).title("Search"));

    frame.render_widget(search, area);
}

fn render_context_menu(frame: &mut Frame, state: &AppState, main_area: Rect) {
    let menu = match &state.context_menu {
        Some(m) => m,
        None => return,
    };

    let items = &menu.items;
    if items.is_empty() {
        return;
    }

    // Get labels for width calculation
    let labels: Vec<&str> = items.iter().map(|a| a.label(menu.is_group)).collect();
    let max_label_len = labels.iter().map(|s| s.len()).max().unwrap_or(10);
    let header_len = menu.header.as_ref().map(|h| h.len()).unwrap_or(0);
    let max_label_len = max_label_len.max(header_len);
    let menu_width = max_label_len + 6; // 2 for borders + 2 for padding on each side
    let header_rows = if menu.header.is_some() { 1 } else { 0 };
    let menu_height = items.len() + header_rows + 2;

    // menu.x and menu.y are in absolute terminal coordinates
    // Convert to main_area-relative coordinates
    let relative_x = menu.x.saturating_sub(main_area.x);
    let relative_y = menu.y.saturating_sub(main_area.y);

    // Clamp within main_area bounds
    let x = relative_x.min(main_area.width.saturating_sub(menu_width as u16));
    let y = relative_y.min(main_area.height.saturating_sub(menu_height as u16));

    let menu_area = Rect::new(
        main_area.x + x,
        main_area.y + y,
        menu_width as u16,
        menu_height as u16,
    );

    // Clear the entire menu area first to prevent text bleeding through
    frame.render_widget(ratatui::widgets::Clear, menu_area);

    // Render bordered box with background
    let block = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::Black).fg(Color::White));
    frame.render_widget(block, menu_area);

    // Render items - full width with consistent padding and explicit background
    let inner = Rect::new(menu_area.x + 1, menu_area.y + 1, menu_area.width - 2, menu_area.height - 2);
    let inner_width = inner.width as usize;
    let mut row_y = inner.y;

    if let Some(header) = menu.header.as_ref() {
        let padded = format!(" {:<width$}", header, width = inner_width.saturating_sub(1));
        let line = Line::from(Span::styled(
            padded,
            Style::default().bg(Color::Black).fg(Color::White).add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(Paragraph::new(line), Rect::new(inner.x, row_y, inner.width, 1));
        row_y = row_y.saturating_add(1);
    }

    for (i, label) in labels.iter().enumerate() {
        let is_hovered = menu.hover == Some(i);
        let style = if is_hovered {
            Style::default().bg(Color::White).fg(Color::Black)
        } else {
            Style::default().bg(Color::Black).fg(Color::White)
        };
        let y = row_y + i as u16;
        if y < inner.y + inner.height {
            // Pad label to full width: " Label" + spaces to fill
            let padded = format!(" {:<width$}", label, width = inner_width.saturating_sub(1));
            let line = Line::from(Span::styled(padded, style));
            frame.render_widget(Paragraph::new(line), Rect::new(inner.x, y, inner.width, 1));
        }
    }
}

fn render_prune_confirm(frame: &mut Frame, state: &AppState, main_area: Rect) {
    let label = match state.pending_prune {
        Some(crate::app::ContextMenuAction::PruneBuildCache) => "build cache",
        Some(crate::app::ContextMenuAction::PruneDanglingImages) => "dangling images",
        Some(crate::app::ContextMenuAction::PruneVolumes) => "volumes",
        _ => return,
    };

    let mut text = vec![Line::from(vec![
        Span::styled("Confirm prune ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(label),
        Span::raw("?"),
    ])];

    if label == "volumes" {
        text.push(Line::from("WARNING! This will remove all volumes not used by at least one container."));
        text.push(Line::from("Are you sure you want to continue?"));
    } else if label == "dangling images" {
        text.push(Line::from("WARNING! This will remove all images without at least one container associated to them."));
        text.push(Line::from("Are you sure you want to continue?"));
    } else if label == "build cache" {
        text.push(Line::from("WARNING! This will remove all unused build cache."));
        text.push(Line::from("Are you sure you want to continue?"));
    }

    text.push(Line::from(vec![
        Span::styled("Y", Style::default().fg(Color::Yellow)),
        Span::raw(" = yes, "),
        Span::styled("N/Esc", Style::default().fg(Color::Yellow)),
        Span::raw(" = cancel"),
    ]));

    let width = 54u16.max(label.len() as u16 + 28);
    let height = 9u16;
    let x = main_area.x + (main_area.width.saturating_sub(width)) / 2;
    let y = main_area.y + (main_area.height.saturating_sub(height)) / 2;
    let area = Rect::new(x, y, width, height);

    frame.render_widget(ratatui::widgets::Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Confirm Prune ");
    frame.render_widget(block, area);

    let inner = Rect::new(area.x + 2, area.y + 2, area.width.saturating_sub(4), area.height.saturating_sub(6));
    frame.render_widget(Paragraph::new(text), inner);

    let buttons_y = area.y + area.height - 3;
    let button_w = 10u16;
    let gap = 4u16;
    let total_buttons_w = button_w * 2 + gap;
    let buttons_x = area.x + (area.width.saturating_sub(total_buttons_w)) / 2;

    let yes_hover = state.pending_prune_hover == Some(crate::app::PruneConfirmChoice::Yes);
    let no_hover = state.pending_prune_hover == Some(crate::app::PruneConfirmChoice::No);
    let yes_style = if yes_hover {
        Style::default().bg(Color::Cyan).fg(Color::Black)
    } else {
        Style::default().bg(Color::Black).fg(Color::White)
    };
    let no_style = if no_hover {
        Style::default().bg(Color::Cyan).fg(Color::Black)
    } else {
        Style::default().bg(Color::Black).fg(Color::White)
    };

    let yes_text = Line::from(Span::styled(" [ Yes ] ", yes_style));
    let no_text = Line::from(Span::styled(" [ No ] ", no_style));
    frame.render_widget(Paragraph::new(yes_text), Rect::new(buttons_x, buttons_y, button_w, 1));
    frame.render_widget(Paragraph::new(no_text), Rect::new(buttons_x + button_w + gap, buttons_y, button_w, 1));
}

fn render_prune_progress(frame: &mut Frame, state: &AppState, main_area: Rect, label: &str) {
    let spinner = state.spinner_char();
    let text = vec![
        Line::from(vec![
            Span::styled("Pruning ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(label),
            Span::raw("..."),
        ]),
        Line::from(vec![
            Span::styled(spinner.to_string(), Style::default().fg(Color::Cyan)),
            Span::raw(" working in background"),
        ]),
    ];

    let width = 38u16.max(label.len() as u16 + 20);
    let height = 6u16;
    let x = main_area.x + (main_area.width.saturating_sub(width)) / 2;
    let y = main_area.y + (main_area.height.saturating_sub(height)) / 2;
    let area = Rect::new(x, y, width, height);

    frame.render_widget(ratatui::widgets::Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Prune In Progress ");
    frame.render_widget(block, area);

    let inner = Rect::new(area.x + 2, area.y + 2, area.width.saturating_sub(4), area.height.saturating_sub(4));
    frame.render_widget(Paragraph::new(text), inner);
}

fn render_prune_output(frame: &mut Frame, state: &AppState, main_area: Rect) {
    let output = match state.prune_output.as_ref() {
        Some(o) => o,
        None => return,
    };
    let title = format!(" Prune Output: {} ", output.label);
    let width = (main_area.width.saturating_mul(85) / 100).max(72);
    let height = (main_area.height.saturating_mul(75) / 100).max(12);
    let x = main_area.x + (main_area.width.saturating_sub(width)) / 2;
    let y = main_area.y + (main_area.height.saturating_sub(height)) / 2;
    let area = Rect::new(x, y, width, height);

    frame.render_widget(ratatui::widgets::Clear, area);
    let block = Block::default().borders(Borders::ALL).title(title);
    frame.render_widget(block, area);

    let inner = Rect::new(
        area.x + 2,
        area.y + 2,
        area.width.saturating_sub(4),
        area.height.saturating_sub(6),
    );
    let text = if output.output.trim().is_empty() {
        "No output from docker.".to_string()
    } else {
        output.output.clone()
    };
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), inner);

    let button_w = 10u16;
    let button_y = area.y + area.height.saturating_sub(3);
    let button_x = area.x + (area.width.saturating_sub(button_w)) / 2;
    let hover = state.prune_output_hover;
    let style = if hover {
        Style::default().bg(Color::Cyan).fg(Color::Black)
    } else {
        Style::default().bg(Color::Black).fg(Color::White)
    };
    let line = Line::from(Span::styled(" [ Close ] ", style));
    frame.render_widget(Paragraph::new(line), Rect::new(button_x, button_y, button_w, 1));
}

fn render_log_progress(frame: &mut Frame, state: &AppState, main_area: Rect, label: &str) {
    let mut text = vec![Line::from(vec![
        Span::styled("Loading logs for ", Style::default().fg(Color::White)),
        Span::styled(label, Style::default().fg(Color::Yellow)),
        Span::raw("..."),
    ])];
    text.push(Line::from(""));
    text.push(Line::from(Span::styled(
        format!("{}  working", state.spinner_char()),
        Style::default().fg(Color::Cyan),
    )));

    let width = (main_area.width.saturating_mul(60) / 100).max(50);
    let height = 6u16;
    let x = main_area.x + (main_area.width.saturating_sub(width)) / 2;
    let y = main_area.y + (main_area.height.saturating_sub(height)) / 2;
    let area = Rect::new(x, y, width, height);

    frame.render_widget(ratatui::widgets::Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Logs ");
    frame.render_widget(block, area);

    let inner = Rect::new(
        area.x + 2,
        area.y + 1,
        area.width.saturating_sub(4),
        area.height.saturating_sub(2),
    );
    frame.render_widget(Paragraph::new(text), inner);
}

fn render_log_output(frame: &mut Frame, state: &AppState, main_area: Rect) {
    let output = match state.log_output.as_ref() {
        Some(output) => output,
        None => return,
    };
    let max_width = main_area.width.saturating_sub(2).max(4);
    let max_height = main_area.height.saturating_sub(2).max(6);
    let width = (main_area.width.saturating_mul(92) / 100)
        .max(80)
        .max(output.title.len() as u16 + 24)
        .min(max_width);
    let height = (main_area.height.saturating_mul(85) / 100)
        .max(14)
        .min(max_height);
    let x = main_area.x + (main_area.width.saturating_sub(width)) / 2;
    let y = main_area.y + (main_area.height.saturating_sub(height)) / 2;
    let area = Rect::new(x, y, width, height);

    frame.render_widget(ratatui::widgets::Clear, area);
    let title_max = width.saturating_sub(4) as usize;
    let title = if title_max > 0 {
        truncate(&output.title, title_max)
    } else {
        String::new()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", title));
    frame.render_widget(block, area);

    let inner = Rect::new(
        area.x + 2,
        area.y + 2,
        area.width.saturating_sub(4),
        area.height.saturating_sub(6),
    );
    let max_scroll = state.log_max_scroll(inner.width, inner.height);
    let scroll_offset = if state.log_follow {
        max_scroll
    } else {
        state.log_scroll.min(max_scroll)
    };
    let lines = if state.log_lines.is_empty() {
        vec![Line::from(state.log_display_text().to_string())]
    } else {
        let start = scroll_offset as usize;
        let end = (start + inner.height as usize).min(state.log_lines.len());
        state.log_lines[start..end].to_vec()
    };
    frame.render_widget(ratatui::widgets::Clear, inner);
    frame.render_widget(
        Paragraph::new(lines),
        inner,
    );

    let button_w = 10u16;
    let button_y = area.y + area.height.saturating_sub(3);
    let button_x = area.x + (area.width.saturating_sub(button_w)) / 2;
    let hover = state.log_output_hover;
    let style = if hover {
        Style::default().bg(Color::Cyan).fg(Color::Black)
    } else {
        Style::default().bg(Color::Black).fg(Color::White)
    };
    let line = Line::from(Span::styled(" [ Close ] ", style));
    frame.render_widget(Paragraph::new(line), Rect::new(button_x, button_y, button_w, 1));
}

fn render_env_modal(frame: &mut Frame, state: &AppState, main_area: Rect) {
    use super::widgets::EnvView;

    let max_width = main_area.width.saturating_sub(2).max(4);
    let max_height = main_area.height.saturating_sub(2).max(6);
    let width = (main_area.width.saturating_mul(90) / 100)
        .max(70)
        .max(state.env_title.len() as u16 + 24)
        .min(max_width);
    let height = (main_area.height.saturating_mul(85) / 100)
        .max(14)
        .min(max_height);
    let x = main_area.x + (main_area.width.saturating_sub(width)) / 2;
    let y = main_area.y + (main_area.height.saturating_sub(height)) / 2;
    let area = Rect::new(x, y, width, height);

    frame.render_widget(ratatui::widgets::Clear, area);
    let block = Block::default()
        .borders(Borders::ALL);
    frame.render_widget(block, area);

    let inner = Rect::new(
        area.x + 2,
        area.y + 2,
        area.width.saturating_sub(4),
        area.height.saturating_sub(6),
    );
    frame.render_widget(ratatui::widgets::Clear, inner);

    let env_view = EnvView::new(&state.env_title, &state.env_vars)
        .info(
            &state.env_info_left1,
            &state.env_info_right1,
            &state.env_info_left2,
            &state.env_info_right2,
        )
        .selected(state.env_selected);
    frame.render_widget(env_view, inner);

    let button_w = 10u16;
    let button_y = area.y + area.height.saturating_sub(3);
    let button_x = area.x + (area.width.saturating_sub(button_w)) / 2;
    let hover = state.env_modal_hover;
    let style = if hover {
        Style::default().bg(Color::Cyan).fg(Color::Black)
    } else {
        Style::default().bg(Color::Black).fg(Color::White)
    };
    let line = Line::from(Span::styled(" [ Close ] ", style));
    frame.render_widget(Paragraph::new(line), Rect::new(button_x, button_y, button_w, 1));
}

fn render_modal_overlay(frame: &mut Frame, area: Rect) {
    let style = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::DIM);
    frame.buffer_mut().set_style(area, style);
}

fn parse_health_status(status: &str) -> Option<Color> {
    let status_lower = status.to_lowercase();
    if status_lower.contains("(healthy)") {
        Some(Color::Green)
    } else if status_lower.contains("(unhealthy)") {
        Some(Color::Red)
    } else if status_lower.contains("health: starting") {
        Some(Color::Yellow)
    } else {
        None
    }
}

fn render_system_bars(frame: &mut Frame, state: &AppState, area: Rect) {
    if area.height < 4 || area.width < 34 {
        return;
    }

    let mem_used = state.mem_total.saturating_sub(state.mem_available);
    let cpu_ratio = (state.cpu_usage / 100.0).clamp(0.0, 1.0) as f64;
    let mem_ratio = if state.mem_total > 0 {
        (mem_used as f64 / state.mem_total as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let swap_ratio = if state.swap_total > 0 {
        (state.swap_used as f64 / state.swap_total as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };

    const BAR_WIDTH: usize = 24;
    let disk_used = state.disk_total.saturating_sub(state.disk_available);

    let cpu_line = format!(
        "CPU: {} {:>5.1}%",
        build_bar(cpu_ratio, BAR_WIDTH),
        state.cpu_usage
    );
    let mem_line = format!(
        "MEM: {} {}/{}",
        build_bar(mem_ratio, BAR_WIDTH),
        format_bytes(mem_used),
        format_bytes(state.mem_total)
    );
    let swap_line = format!(
        "SWP: {} {}/{}",
        build_bar(swap_ratio, BAR_WIDTH),
        format_bytes(state.swap_used),
        format_bytes(state.swap_total)
    );
    let disk_line = format!(
        "DSK: {} {}/{}",
        build_bar(
            if state.disk_total > 0 {
                (disk_used as f64 / state.disk_total as f64).clamp(0.0, 1.0)
            } else {
                0.0
            },
            BAR_WIDTH
        ),
        format_bytes(disk_used),
        format_bytes(state.disk_total)
    );

    frame.render_widget(Paragraph::new(cpu_line), Rect::new(area.x, area.y, area.width, 1));
    frame.render_widget(Paragraph::new(mem_line), Rect::new(area.x, area.y + 1, area.width, 1));
    frame.render_widget(Paragraph::new(swap_line), Rect::new(area.x, area.y + 2, area.width, 1));
    frame.render_widget(Paragraph::new(disk_line), Rect::new(area.x, area.y + 3, area.width, 1));
}

fn format_bytes(bytes: u64) -> String {
    const GB: u64 = 1024 * 1024 * 1024;
    const MB: u64 = 1024 * 1024;

    if bytes >= GB {
        format!("{:.1}G", bytes as f64 / GB as f64)
    } else {
        format!("{:.0}M", bytes as f64 / MB as f64)
    }
}

fn build_bar(ratio: f64, width: usize) -> String {
    let filled = (ratio * width as f64).round().clamp(0.0, width as f64) as usize;
    let empty = width.saturating_sub(filled);
    format!("{}{}", "▓".repeat(filled), "░".repeat(empty))
}

// Helper functions
#[allow(dead_code)]
fn calculate_scroll_offset(selected: usize, visible_height: usize, total: usize) -> usize {
    if total <= visible_height {
        return 0;
    }
    if selected < visible_height / 2 {
        return 0;
    }
    let max_offset = total.saturating_sub(visible_height);
    let ideal_offset = selected.saturating_sub(visible_height / 2);
    ideal_offset.min(max_offset)
}

fn format_memory(bytes: u64) -> String {
    const GB: u64 = 1024 * 1024 * 1024;
    const MB: u64 = 1024 * 1024;
    const KB: u64 = 1024;

    if bytes >= GB {
        format!("{:.1}G", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.0}M", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.0}K", bytes as f64 / KB as f64)
    } else {
        format!("{}B", bytes)
    }
}

fn format_uptime(secs: u64) -> String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 {
        format!("{}d{}h", days, hours)
    } else if hours > 0 {
        format!("{}h{}m", hours, mins)
    } else {
        format!("{}m", mins.max(1))
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}...", &s[..max_len.saturating_sub(3)])
    }
}

/// Render docker system df stats as a table showing disk usage for images, containers, volumes, and build cache
fn render_docker_df_stats(frame: &mut Frame, state: &AppState, area: Rect, df: &DockerSystemDf) {
    let hover_row = state.docker_df_hover;

    // Build table rows - 5 rows total (header + 4 data)
    let rows = [
        ("Images", df.images_total, df.images_active, &df.images_size, &df.images_reclaimable, &df.images_reclaimable_pct),
        ("Containers", df.containers_total, df.containers_active, &df.containers_size, &df.containers_reclaimable, &df.containers_reclaimable_pct),
        ("Volumes", df.volumes_total, df.volumes_active, &df.volumes_size, &df.volumes_reclaimable, &df.volumes_reclaimable_pct),
        ("Build Cache", df.build_cache_total as u32, 0, &df.build_cache_size, &df.build_cache_reclaimable, &df.build_cache_reclaimable_pct),
    ];

    let table_rows: Vec<Row> = rows.iter().enumerate().map(|(i, &(name, total, active, size, reclaimable, reclaimable_pct))| {
        let reclaimable_display = if !reclaimable_pct.is_empty() {
            format!("{} ({})", reclaimable, reclaimable_pct)
        } else {
            reclaimable.to_string()
        };

        let is_hovered = hover_row == Some(i);
        let row_style = if is_hovered {
            Style::default().bg(Color::Rgb(40, 40, 45))
        } else {
            Style::default()
        };

        Row::new(vec![
            Cell::from(name).style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Cell::from(total.to_string()),
            Cell::from(active.to_string()),
            Cell::from(size.clone()),
            Cell::from(reclaimable_display).style(Style::default().fg(Color::DarkGray)),
        ])
        .style(row_style)
    }).collect();

    let header = Row::new(vec![
        Cell::from("Type").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Total").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Active").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Size").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Reclaimable").style(Style::default().add_modifier(Modifier::BOLD)),
    ]);

    // Use fixed widths instead of percentages to ensure consistent rendering
    let widths = vec![
        Constraint::Length(14),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(14),
        Constraint::Length(20),
    ];

    let table = Table::new(table_rows, widths)
        .header(header)
        .block(Block::default().title(" Docker Disk Usage ").borders(Borders::ALL));

    frame.render_widget(table, area);
}
