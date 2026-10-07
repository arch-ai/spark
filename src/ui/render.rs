//! Main ratatui render function that dispatches to view-specific renderers

use std::collections::HashMap;

use ratatui::{
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Tabs, Wrap},
    Frame,
};
use sysinfo::Pid;

use crate::app::sorting::{RenderedHeaders, SortField, SortHeaderHit, SortTarget};
use crate::app::{
    AppState, DeleteConfirmChoice, DeleteKind, DockerListKind, Focus, InputMode, LogOutputMode,
    NodeTab, SortBy, SortOrder, ViewMode,
};
use crate::system::docker::DockerSystemDf;
use crate::system::{docker, node, ports, process};

use super::widgets::{HelpBar, HelpItem, Sidebar};

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;

/// Uses the same constraints and spacing as Table, including dynamic column widths.
pub(super) fn sortable_header(
    state: &AppState,
    area: Rect,
    target: SortTarget,
    labels: &[&str],
    fields: &[Option<SortField>],
    widths: &[Constraint],
) -> Row<'static> {
    let columns = Layout::horizontal(widths.iter().copied())
        .flex(Flex::Start)
        .spacing(1)
        .split(Rect::new(area.x, area.y, area.width, area.height.min(1)));
    let sort = state.sort_for(target);
    let mut hits = state.rendered_headers.borrow_mut();
    let cells = labels
        .iter()
        .zip(fields)
        .zip(columns.iter())
        .map(|((label, field), column)| {
            if let Some(field) = field {
                if column.width > 0 && column.height > 0 {
                    hits.hits.push(SortHeaderHit {
                        area: *column,
                        target,
                        field: *field,
                    });
                }
            }
            let active = *field == Some(sort.field);
            let text = if active {
                let arrow = if sort.order == SortOrder::Asc {
                    "▲"
                } else {
                    "▼"
                };
                if column.width >= 2 {
                    let label = label
                        .chars()
                        .take(column.width.saturating_sub(2) as usize)
                        .collect::<String>();
                    format!("{label} {arrow}")
                } else {
                    arrow.to_owned()
                }
            } else {
                (*label).to_owned()
            };
            Cell::from(text).style(
                Style::default()
                    .fg(if active { Color::Cyan } else { Color::Gray })
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect::<Vec<_>>();
    Row::new(cells)
}

/// Render navigation icons (▲/▼) in table area corners for jump to top/bottom
fn render_nav_icons(frame: &mut Frame, area: Rect, scroll: usize, total: usize, visible: usize) {
    if area.width < 3 || area.height == 0 {
        return;
    }
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
    *state.rendered_headers.borrow_mut() = RenderedHeaders {
        bounds: area,
        view: Some(state.view_mode),
        resource: if state.docker_list_open {
            state.docker_list_kind
        } else {
            None
        },
        node_tab: (state.view_mode == ViewMode::Node).then_some(state.node_tab),
        hits: Vec::new(),
    };

    if area.width < 30 || area.height < 10 {
        frame.render_widget(
            Paragraph::new("Spark: enlarge terminal (30×10 minimum). Ctrl+C quits.")
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    let full_main_area = super::layout::main_area(area);
    let (main_area, inspector_area) =
        super::workspace::panes(full_main_area, state.workspace.inspector.is_some());
    let sidebar_area = (main_area.x > area.x)
        .then(|| Rect::new(area.x, area.y, main_area.x - area.x, area.height));

    // Render sidebar if visible
    if let Some(sidebar_rect) = sidebar_area {
        render_sidebar(frame, state, sidebar_rect);
    }

    // Render main content based on view mode
    if main_area.width > 0 && main_area.height > 0 {
        match state.view_mode {
            ViewMode::Projects => super::workspace::render_projects(frame, state, main_area),
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
                render_node_view(
                    frame, state, main_area, node_view, node_rows, pm2_view, pm2_rows,
                );
            }
        }
    }
    if let Some(inspector_area) = inspector_area {
        super::workspace::render_inspector(frame, state, inspector_area);
    }
    super::workspace::render_dialogs(frame, state, full_main_area);

    let blocking_modal_open = state.pending_delete.is_some()
        || state.pending_prune.is_some()
        || state.log_in_progress.is_some()
        || state.log_output.is_some();
    let any_modal_open = blocking_modal_open || state.env_modal_open || state.docker_list_open;
    if any_modal_open {
        render_modal_overlay(frame, frame.area());
    }

    if state.pending_delete.is_some() {
        render_delete_confirm(frame, state, main_area);
    }

    if state.pending_prune.is_some() {
        render_prune_confirm(frame, state, main_area);
    }

    if let Some(label) = state.log_in_progress.as_deref() {
        render_log_progress(frame, state, main_area, label);
    }

    if state.log_output.is_some() {
        render_log_output(frame, state, main_area);
    }

    if state.env_modal_open && !blocking_modal_open {
        render_env_modal(frame, state, main_area);
    }
    if state.docker_list_open && !blocking_modal_open && !state.env_modal_open {
        state.rendered_headers.borrow_mut().hits.clear();
        render_docker_list_modal(frame, state, main_area);
    }

    // Render context menu if active
    if state.context_menu.is_some() {
        render_context_menu(frame, state, main_area);
    }
    let status = if matches!(
        state.workspace.cleanup,
        crate::app::workspace::Cleanup::Running(_)
    ) && !state.workspace.cleanup_open
    {
        Some(("Volume cleanup running · F6 progress".into(), Color::Yellow))
    } else if let crate::app::workspace::Cleanup::Result(_, failed) = &state.workspace.cleanup {
        if !state.workspace.cleanup_open {
            Some((
                format!(
                    "Volume cleanup {} · F6 details",
                    if *failed { "failed" } else { "completed" }
                ),
                if *failed {
                    Color::LightRed
                } else {
                    Color::Cyan
                },
            ))
        } else {
            None
        }
    } else if let Some(delete) = &state.delete_in_progress {
        let elapsed = delete.started_at.elapsed().as_secs();
        Some((
            format!(
                "{} Deleting {}:{:02} | {}",
                state.spinner_char(),
                elapsed / 60,
                elapsed % 60,
                delete.label
            ),
            Color::Yellow,
        ))
    } else if let Some(label) = &state.prune_in_progress {
        Some((
            format!(
                "{} Pruning {label} · running in background",
                state.spinner_char()
            ),
            Color::Yellow,
        ))
    } else if !state.pending_operations.is_empty() {
        Some((
            format!(
                "{} {} action(s) in progress",
                state.spinner_char(),
                state.pending_operations.len()
            ),
            Color::Yellow,
        ))
    } else {
        state
            .message
            .as_ref()
            .map(|message| (message.clone(), Color::Cyan))
    };
    if let Some((status, color)) = status {
        if !blocking_modal_open && !state.env_modal_open {
            let area = Rect::new(
                main_area.x,
                main_area.bottom().saturating_sub(1),
                main_area.width,
                1,
            );
            frame.render_widget(
                Paragraph::new(truncate(&status, area.width as usize))
                    .style(Style::default().bg(Color::Black).fg(color)),
                area,
            );
        }
    }
    if state.sort_menu.is_some() {
        render_modal_overlay(frame, frame.area());
        render_sort_menu(frame, state, main_area);
    }
}

fn render_sort_menu(frame: &mut Frame, state: &AppState, bounds: Rect) {
    let Some(menu) = state.sort_menu else {
        return;
    };
    let fields = menu.target.fields();
    let current = state.sort_for(menu.target);
    let (area, scroll) = super::layout::sort_menu_area(bounds, fields.len(), menu.selected);
    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" Sort · {} ", menu.target.sort_label(current)))
            .title_bottom(" Enter apply · r reverse · Esc "),
        area,
    );
    for (row, field) in fields
        .iter()
        .enumerate()
        .skip(scroll)
        .take(area.height.saturating_sub(2) as usize)
    {
        let style = if row == menu.selected {
            Style::default().bg(Color::Cyan).fg(Color::Black)
        } else {
            Style::default()
        };
        let label = format!(" {}", menu.target.field_label(*field));
        frame.render_widget(
            Paragraph::new(format!(
                "{label:<width$}",
                width = area.width.saturating_sub(2) as usize
            ))
            .style(style),
            Rect::new(
                area.x + 1,
                area.y + 1 + (row - scroll) as u16,
                area.width.saturating_sub(2),
                1,
            ),
        );
    }
}

fn render_sidebar(frame: &mut Frame, state: &AppState, area: Rect) {
    let items = vec![
        "1 Processes",
        "2 Ports",
        "3 Docker",
        "4 Node JS",
        "5 Projects",
    ];

    let active_view = if state.view_mode == ViewMode::DockerEnv {
        state.env_return_view
    } else {
        state.view_mode
    };

    let active_index = match active_view {
        ViewMode::Projects => 4,
        ViewMode::Process => 0,
        ViewMode::Ports => 1,
        ViewMode::Docker | ViewMode::DockerEnv => 2,
        ViewMode::Node => 3,
    };

    let sidebar = Sidebar::new(items)
        .active_index(active_index)
        .selected_index(state.sidebar_index)
        .hover_index(state.sidebar_hover)
        .logo_frame(state.logo_frame)
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
    // Header with search, system bars, table, help.
    let chunks = super::layout::process_layout(area);

    // Header info
    let sort_label = match state.sort_by {
        SortBy::Cpu => "CPU",
        SortBy::Memory => "TREE RAM",
        SortBy::Name => "NAME",
        SortBy::Pid => "PID",
        SortBy::SelfMemory => "RAM",
        SortBy::Swap => "TREE SWAP",
        SortBy::User => "USER",
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
        "Sort: {} {} | Tree: {} | {}",
        sort_label, order_label, zoom_label, mode_label
    );
    render_collection_header(
        frame,
        super::layout::collection_header(area, ViewMode::Process),
        "PROCESS VIEW",
        vec![Line::raw(header_text)],
        &state.process_filter,
        state.input_mode == InputMode::Filter,
    );

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
            HelpItem::key("s"),
            HelpItem::plain(" sort "),
            HelpItem::key("?"),
            HelpItem::plain(" help · "),
            HelpItem::key("↑/↓"),
            HelpItem::plain(" nav "),
            HelpItem::key("k"),
            HelpItem::plain(" kill "),
            HelpItem::key("/"),
            HelpItem::plain(" filter "),
            HelpItem::key("F10"),
            HelpItem::plain(" actions "),
            HelpItem::key("z"),
            HelpItem::plain(" tree "),
            HelpItem::key("q"),
            HelpItem::plain(" quit"),
        ],
        vec![HelpItem::plain("RAM: PSS, ~=RSS | TREE: +children")],
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
    let show_cpu = area.width >= 40;
    let show_self = area.width >= 52;
    let show_swap = area.width >= 76;
    let show_user = area.width >= 92;
    let max_user_len = if show_user {
        rows.iter()
            .filter_map(|row| processes.get(&row.pid))
            .map(|info| info.user.len())
            .max()
            .unwrap_or(4)
            .clamp(6, 12)
    } else {
        0
    };
    let visible_height = area.height.saturating_sub(3) as usize;
    let scroll_offset = state.process_scroll;
    let table_rows: Vec<Row> = rows
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(visible_height)
        .filter_map(|(idx, tree_row)| {
            let proc = processes.get(&tree_row.pid)?;
            let style = if idx == state.selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else if state.hover_row == Some(idx) {
                Style::default().bg(Color::Rgb(40, 40, 45))
            } else {
                Style::default()
            };
            let mut cells = vec![Cell::from(tree_row.pid.as_u32().to_string())];
            if show_cpu {
                cells.push(Cell::from(format!("{:.1}%", proc.cpu)));
            }
            if show_self {
                cells.push(Cell::from(format_process_memory(
                    proc.memory_bytes,
                    proc.memory_estimated,
                )));
            }
            cells.push(Cell::from(format_process_memory(
                proc.tree_memory_bytes,
                proc.tree_memory_estimated,
            )));
            if show_swap {
                cells.push(Cell::from(
                    proc.tree_swap_bytes
                        .map(format_memory)
                        .unwrap_or_else(|| "?".into()),
                ));
            }
            if show_user {
                cells.push(Cell::from(proc.user.clone()));
            }
            cells.push(Cell::from(format!("{}{}", tree_row.prefix, proc.name)));
            Some(Row::new(cells).style(style))
        })
        .collect();
    let mut headers = vec!["PID"];
    let mut fields = vec![Some(SortField::Pid)];
    let mut widths = vec![Constraint::Length(8)];
    if show_cpu {
        headers.push("CPU");
        fields.push(Some(SortField::Cpu));
        widths.push(Constraint::Length(7));
    }
    if show_self {
        headers.push("RAM");
        fields.push(Some(SortField::SelfMemory));
        widths.push(Constraint::Length(8));
    }
    headers.push("TREE");
    fields.push(Some(SortField::Memory));
    widths.push(Constraint::Length(8));
    if show_swap {
        headers.push("SWAPtree");
        fields.push(Some(SortField::Swap));
        widths.push(Constraint::Length(10));
    }
    if show_user {
        headers.push("USER");
        fields.push(Some(SortField::User));
        widths.push(Constraint::Length(max_user_len as u16));
    }
    headers.push("NAME");
    fields.push(Some(SortField::Name));
    widths.push(Constraint::Fill(1));
    let header = sortable_header(
        state,
        Block::default().borders(Borders::ALL).inner(area),
        SortTarget::Process,
        &headers,
        &fields,
        &widths,
    );
    let table = Table::new(table_rows, widths)
        .column_spacing(1)
        .header(header)
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(table, area);
    if rows.is_empty() {
        render_collection_empty(
            frame,
            area,
            state.process_loaded,
            &state.process_filter,
            "No processes found.",
        );
    }
    render_nav_icons(frame, area, scroll_offset, rows.len(), visible_height);
}

fn format_process_memory(bytes: u64, estimated: bool) -> String {
    format!(
        "{}{}",
        if estimated { "~" } else { "" },
        format_memory(bytes)
    )
}

fn render_docker_view(
    frame: &mut Frame,
    state: &AppState,
    area: Rect,
    docker_view: &[docker::ContainerInfo],
    docker_rows: &[docker::DockerRow],
) {
    let chunks = super::layout::docker_layout(area);
    let header = super::layout::collection_header(area, ViewMode::Docker);
    let running = docker_view
        .iter()
        .filter(|container| container.running)
        .count();
    let stopped = docker_view.len() - running;
    let summary = if header.details.width >= 54 {
        format!(
            "{} shown / {} total   {} running   {} stopped",
            docker_view.len(),
            state.docker_total,
            running,
            stopped
        )
    } else {
        format!(
            "{}/{} shown · {} run · {} stop",
            docker_view.len(),
            state.docker_total,
            running,
            stopped
        )
    };
    let (notice, color) = if let Some(error) = &state.docker_error {
        (
            format!(
                "{}: {error}  | F5 retry",
                if state.docker_updated_at.is_some() {
                    "STALE"
                } else {
                    "UNAVAILABLE"
                }
            ),
            Color::Yellow,
        )
    } else if let Some(message) = &state.message {
        (message.clone(), Color::Cyan)
    } else if let Some(notice) = state.docker_memory.notice() {
        (notice, Color::Yellow)
    } else if let Some(updated) = state.docker_updated_at {
        (
            format!("Updated {}s ago · F5 refresh", updated.elapsed().as_secs()),
            Color::Gray,
        )
    } else {
        ("Connecting to Docker...".into(), Color::Cyan)
    };
    let title = if header.columns {
        "DOCKER".to_string()
    } else {
        format!(
            "DOCKER  {}/{} shown · {} running",
            docker_view.len(),
            state.docker_total,
            running
        )
    };
    let status = Line::styled(notice, Style::default().fg(color));
    let details = if header.columns {
        vec![Line::raw(summary), status]
    } else {
        vec![status]
    };
    render_collection_header(
        frame,
        header,
        &title,
        details,
        &state.docker_filter,
        state.input_mode == InputMode::Filter,
    );
    if chunks[3].height > 0 {
        render_docker_df_stats(frame, state, chunks[3], &state.docker_system_df);
    }
    render_docker_table(frame, state, chunks[4], docker_view, docker_rows);
    let help_items = if area.width < 60 {
        vec![
            vec![
                HelpItem::key("↑↓"),
                HelpItem::plain(" move "),
                HelpItem::key("↵"),
                HelpItem::plain(" shell "),
                HelpItem::key("F2"),
                HelpItem::plain(" details "),
                HelpItem::key("l"),
                HelpItem::plain(" logs "),
                HelpItem::key("e"),
                HelpItem::plain(" env"),
            ],
            vec![
                HelpItem::key("i/v/a"),
                HelpItem::plain(" lists "),
                HelpItem::key("/"),
                HelpItem::plain(" filter "),
                HelpItem::key("F5"),
                HelpItem::plain(" refresh "),
                HelpItem::key("q"),
                HelpItem::plain(" quit"),
            ],
        ]
    } else {
        vec![
            vec![
                HelpItem::key("s"),
                HelpItem::plain(" sort "),
                HelpItem::key("?"),
                HelpItem::plain(" help · "),
                HelpItem::key("↑/↓"),
                HelpItem::plain(" select "),
                HelpItem::key("Enter"),
                HelpItem::plain(" shell "),
                HelpItem::key("F2"),
                HelpItem::plain(" details "),
                HelpItem::key("l"),
                HelpItem::plain(" logs "),
                HelpItem::key("e"),
                HelpItem::plain(" env "),
                HelpItem::key("/"),
                HelpItem::plain(" filter"),
            ],
            vec![
                HelpItem::key("i/v/a"),
                HelpItem::plain(" images/volumes/containers "),
                HelpItem::key("Right click"),
                HelpItem::plain(" actions "),
                HelpItem::key("q"),
                HelpItem::plain(" quit"),
            ],
        ]
    };
    frame.render_widget(HelpBar::new(help_items), chunks[5]);
}

fn render_docker_table(
    frame: &mut Frame,
    state: &AppState,
    area: Rect,
    docker_view: &[docker::ContainerInfo],
    docker_rows: &[docker::DockerRow],
) {
    let visible_height = area.height.saturating_sub(3) as usize;
    let scroll_offset = state.docker_scroll;
    let title = if let Some((name, _)) = &state.docker_volume_scope {
        format!(
            " Volume: {name} · x clears · {} ",
            state
                .sort_for(crate::app::sorting::SortTarget::Docker)
                .label()
        )
    } else {
        format!(
            " Containers · {} · s sort ",
            state
                .sort_for(crate::app::sorting::SortTarget::Docker)
                .label()
        )
    };
    let block = Block::default().borders(Borders::ALL).title(title);
    if docker_rows.is_empty() {
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let message = if let Some(error) = &state.docker_error {
            format!("Unable to refresh containers.\n{error}\nF5 retries; check the Docker daemon, context and permissions.")
        } else if state.docker_updated_at.is_none() {
            "Loading containers...".into()
        } else if !state.docker_filter.is_empty() {
            "No containers match this filter. Press x to clear it.".into()
        } else {
            "No containers in this Docker context. Press F5 to refresh.".into()
        };
        frame.render_widget(
            Paragraph::new(message)
                .wrap(Wrap { trim: true })
                .style(Style::default().fg(Color::Gray)),
            inner,
        );
        return;
    }
    let show_status = area.width >= 40;
    let show_ports = area.width >= 62;
    let show_image = area.width >= 92;
    let show_id = area.width >= 124;
    let mut widths = vec![
        Constraint::Length(2),
        Constraint::Fill(3),
        Constraint::Length(9),
    ];
    let mut labels = vec!["", "NAME / PROJECT", "RAM"];
    let mut fields = vec![None, Some(SortField::Name), Some(SortField::Memory)];
    if show_status {
        widths.push(if area.width < 62 {
            Constraint::Length(11)
        } else {
            Constraint::Fill(3)
        });
        labels.push("STATUS");
        fields.push(Some(SortField::Status));
    }
    if show_ports {
        widths.push(Constraint::Fill(2));
        labels.push("HOST PORTS");
        fields.push(Some(SortField::Port));
    }
    if show_image {
        widths.push(Constraint::Fill(3));
        labels.push("IMAGE");
        fields.push(Some(SortField::Image));
    }
    if show_id {
        widths.push(Constraint::Length(12));
        labels.push("ID");
        fields.push(Some(SortField::Id));
    }
    let table_rows: Vec<Row> = docker_rows
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(visible_height)
        .map(|(idx, row)| {
            let selected = idx == state.docker_selected_row;
            let style = if selected {
                Style::default()
                    .bg(Color::Rgb(35, 65, 80))
                    .add_modifier(Modifier::BOLD)
            } else if state.hover_row == Some(idx) {
                Style::default().bg(Color::Rgb(40, 40, 45))
            } else {
                Style::default()
            };
            let mut cells = match row {
                docker::DockerRow::Group {
                    name,
                    count,
                    running_count,
                    ..
                } => {
                    let mut cells = vec![
                        Cell::from(if *running_count == *count {
                            "●"
                        } else {
                            "◐"
                        })
                        .style(Style::default().fg(Color::Cyan)),
                        Cell::from(name.clone()).style(
                            Style::default()
                                .fg(Color::Cyan)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Cell::from(""),
                    ];
                    if show_status {
                        cells.push(
                            Cell::from(format!("{running_count}/{count} running"))
                                .style(Style::default().fg(Color::Gray)),
                        );
                    }
                    cells
                }
                docker::DockerRow::Separator => vec![Cell::from(""); 3],
                docker::DockerRow::Item { index, prefix } => {
                    let Some(container) = docker_view.get(*index) else {
                        return Row::default();
                    };
                    let loading = state.pending_operations.contains_key(&container.id);
                    let color =
                        parse_health_status(&container.status).unwrap_or(if container.running {
                            Color::Green
                        } else {
                            Color::Gray
                        });
                    let marker = if loading {
                        state.spinner_char().to_string()
                    } else if container.running {
                        "●".into()
                    } else {
                        "○".into()
                    };
                    let mut cells = vec![
                        Cell::from(marker).style(Style::default().fg(color)),
                        Cell::from(format!(
                            "{}{}",
                            if area.width < 60 { "" } else { prefix },
                            container.name
                        )),
                        Cell::from(if !container.running {
                            "-".into()
                        } else if let Some(memory) = container.memory {
                            format!(
                                "{}{}",
                                format_memory(memory.used_bytes),
                                if memory.stale { "*" } else { "" }
                            )
                        } else {
                            "?".into()
                        })
                        .style(Style::default().fg(
                            if container.memory.is_some_and(|m| m.stale) {
                                Color::Yellow
                            } else {
                                Color::Gray
                            },
                        )),
                    ];
                    if show_status {
                        cells.push(
                            Cell::from(if loading {
                                "Working...".into()
                            } else {
                                docker_status_text(&container.status)
                            })
                            .style(Style::default().fg(color)),
                        );
                    }
                    if show_ports {
                        cells.push(Cell::from(container.port_public.clone()));
                    }
                    if show_image {
                        cells.push(Cell::from(container.image.clone()));
                    }
                    if show_id {
                        cells.push(Cell::from(
                            container.id.chars().take(12).collect::<String>(),
                        ));
                    }
                    cells
                }
            };
            cells.resize(labels.len(), Cell::from(""));
            Row::new(cells).style(style)
        })
        .collect();
    let header = sortable_header(
        state,
        block.inner(area),
        SortTarget::Docker,
        &labels,
        &fields,
        &widths,
    );
    frame.render_widget(
        Table::new(table_rows, widths)
            .header(header)
            .block(block)
            .column_spacing(1),
        area,
    );
    render_nav_icons(
        frame,
        area,
        scroll_offset,
        docker_rows.len(),
        visible_height,
    );
}

fn docker_status_text(status: &str) -> String {
    if let Some((uptime, health)) = status.split_once(" (") {
        if status.starts_with("Up") {
            return format!(
                "{} · {}",
                health.trim_end_matches(')'),
                uptime.trim_start_matches("Up ")
            );
        }
    }
    status.to_string()
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
    let chunks = super::layout::resource_layout(area);
    let header_text = if let Some(error) = &state.ports_error {
        format!("{error} | F5 retry")
    } else if state.ports_loaded {
        format!(
            "Listening bindings: {} · {} · s sort",
            ports_cache.len(),
            state
                .sort_for(crate::app::sorting::SortTarget::Ports)
                .label()
        )
    } else {
        "Loading listening ports...".into()
    };
    render_collection_header(
        frame,
        super::layout::collection_header(area, ViewMode::Ports),
        "PORTS VIEW",
        vec![Line::styled(
            header_text,
            Style::default().fg(if state.ports_error.is_some() {
                Color::Yellow
            } else {
                Color::Reset
            }),
        )],
        &state.ports_filter,
        state.input_mode == InputMode::Filter,
    );

    // Table
    render_ports_table(frame, state, chunks[3], ports_cache, ports_rows);

    // Help
    let help_items = vec![
        vec![
            HelpItem::key("s"),
            HelpItem::plain(" sort "),
            HelpItem::key("?"),
            HelpItem::plain(" help · "),
            HelpItem::key("↑/↓"),
            HelpItem::plain(" nav "),
            HelpItem::key("k"),
            HelpItem::plain(" kill "),
            HelpItem::key("/"),
            HelpItem::plain(" filter "),
            HelpItem::key("F10"),
            HelpItem::plain(" actions"),
        ],
        vec![HelpItem::plain("1-5 views · F5 refresh · q quit")],
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
    let capacity = area.height.saturating_sub(3) as usize;
    let show_pid = area.width >= 55;
    let show_command = area.width >= 85;
    let rows: Vec<Row> = ports_rows
        .iter()
        .enumerate()
        .skip(state.ports_scroll)
        .take(capacity)
        .map(|(idx, row)| match row {
            ports::PortRow::Group { name } => Row::new(vec![Cell::from(truncate(name, 12)).style(
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )]),
            ports::PortRow::Item { index } => {
                let port = &ports_cache[*index];
                let mut cells = vec![
                    Cell::from(port.binding_display()),
                    Cell::from(port.proto.clone()),
                ];
                if show_pid {
                    cells.push(Cell::from(if port.pid.as_u32() == 0 {
                        "-".into()
                    } else {
                        port.pid.to_string()
                    }));
                }
                cells.push(Cell::from(port.name.clone()));
                if show_command {
                    cells.push(Cell::from(port.exe_path.clone()));
                }
                Row::new(cells).style(selection_style(
                    idx == state.selected,
                    state.hover_row == Some(idx),
                ))
            }
        })
        .collect();
    let mut labels = vec!["EXT:INT", "PROTO"];
    let mut fields = vec![Some(SortField::Port), Some(SortField::Protocol)];
    let mut widths = vec![
        Constraint::Length(11),
        Constraint::Length(if area.width >= 40 { 7 } else { 5 }),
    ];
    if show_pid {
        labels.push("PID");
        fields.push(Some(SortField::Pid));
        widths.push(Constraint::Length(8));
    }
    labels.push("NAME");
    fields.push(Some(SortField::Name));
    widths.push(Constraint::Fill(1));
    if show_command {
        labels.push("COMMAND");
        fields.push(Some(SortField::Command));
        widths.push(Constraint::Fill(1));
    }
    let header = sortable_header(
        state,
        Block::default().borders(Borders::ALL).inner(area),
        SortTarget::Ports,
        &labels,
        &fields,
        &widths,
    );
    frame.render_widget(
        Table::new(rows, widths)
            .column_spacing(1)
            .header(header)
            .block(Block::default().borders(Borders::ALL)),
        area,
    );
    if ports_rows.is_empty() {
        render_collection_empty(
            frame,
            area,
            state.ports_loaded || state.ports_error.is_some(),
            &state.ports_filter,
            if state.ports_error.is_some() {
                "Unable to refresh. F5 retries."
            } else {
                "No listening ports found."
            },
        );
    }
    for (offset, row) in ports_rows
        .iter()
        .skip(state.ports_scroll)
        .take(capacity)
        .enumerate()
    {
        if let ports::PortRow::Group { name } = row {
            frame.render_widget(
                Paragraph::new(name.as_str()).style(
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Rect::new(
                    area.x + 1,
                    area.y + 2 + offset as u16,
                    area.width.saturating_sub(2),
                    1,
                ),
            );
        }
    }
    render_nav_icons(frame, area, state.ports_scroll, ports_rows.len(), capacity);
}

fn selection_style(selected: bool, hovered: bool) -> Style {
    if selected {
        Style::default().add_modifier(Modifier::REVERSED)
    } else if hovered {
        Style::default().bg(Color::Rgb(40, 40, 45))
    } else {
        Style::default()
    }
}

fn render_collection_empty(frame: &mut Frame, area: Rect, loaded: bool, filter: &str, empty: &str) {
    let text = if !loaded {
        "Loading..."
    } else if !filter.is_empty() {
        "No matches. x clears the filter."
    } else {
        empty
    };
    let inner = Rect::new(
        area.x + 1,
        area.y + 2,
        area.width.saturating_sub(2),
        area.height.saturating_sub(3),
    );
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);
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
    let chunks = super::layout::node_layout(area);
    let (pm2_area, node_area) = super::layout::node_tables(area, state.node_tab);
    let help_area = chunks[5];
    let summary = match state.node_tab {
        NodeTab::Processes if !state.node_loaded => "Loading Node.js processes...".into(),
        NodeTab::Processes => format!("{} Node.js processes · Tab switches tabs", node_view.len()),
        NodeTab::Pm2 => {
            if let Some(error) = &state.pm2_error {
                format!("PM2: {error} | F5 retry")
            } else if state.pm2_loading {
                "Loading PM2...".into()
            } else {
                format!("{} PM2 processes · Tab switches tabs", pm2_rows.len())
            }
        }
    };
    let summary = Line::styled(
        summary,
        Style::default().fg(
            if state.node_tab == NodeTab::Pm2 && state.pm2_error.is_some() {
                Color::Yellow
            } else {
                Color::Reset
            },
        ),
    );
    render_collection_header(
        frame,
        super::layout::collection_header(area, ViewMode::Node),
        "NODE VIEW",
        vec![Line::default(), summary],
        &state.node_filter,
        state.input_mode == InputMode::Filter,
    );
    frame.render_widget(
        Tabs::new(super::layout::NODE_TAB_LABELS)
            .divider(super::layout::NODE_TAB_DIVIDER)
            .padding(" ", " ")
            .select(usize::from(state.node_tab == NodeTab::Pm2))
            .style(Style::default().fg(Color::Gray))
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
        chunks[1],
    );

    // Tables
    match state.node_tab {
        NodeTab::Pm2 => render_pm2_table(frame, state, pm2_area, pm2_view, pm2_rows),
        NodeTab::Processes => render_node_table(frame, state, node_area, node_view, node_rows),
    }

    let help_items = vec![
        vec![
            HelpItem::key("Tab"),
            HelpItem::plain(" tabs · "),
            HelpItem::key("s"),
            HelpItem::plain(" sort "),
            HelpItem::key("?"),
            HelpItem::plain(" help · "),
            HelpItem::key("↑/↓"),
            HelpItem::plain(" nav "),
            HelpItem::key("F10"),
            HelpItem::plain(" actions "),
            HelpItem::key("/"),
            HelpItem::plain(" filter"),
        ],
        vec![HelpItem::plain("k stop/kill · e env · F5 refresh")],
    ];
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
    let show_id = area.width >= 36;
    let show_pid = area.width >= 55;
    let show_cpu = area.width >= 65;
    let show_mode = area.width >= 80;
    let show_uptime = area.width >= 100;
    let capacity = area.height.saturating_sub(3) as usize;
    let rows: Vec<Row> = pm2_rows
        .iter()
        .enumerate()
        .skip(state.pm2_scroll)
        .take(capacity)
        .map(|(idx, source)| {
            let proc = &pm2_view[*source];
            let pending = state
                .pending_operations
                .contains_key(&format!("pm2::{}", proc.pm_id));
            let status = if pending {
                "working"
            } else {
                proc.status.as_str()
            };
            let color = match status {
                "online" => Color::Green,
                "stopped" | "errored" => Color::Red,
                "working" => Color::Yellow,
                _ => Color::Reset,
            };
            let mut cells = vec![
                Cell::from(proc.name.clone()),
                Cell::from(status.to_owned()).style(Style::default().fg(color)),
                Cell::from(
                    proc.memory_bytes
                        .map(format_memory)
                        .unwrap_or_else(|| "-".into()),
                ),
            ];
            if show_id {
                cells.insert(0, Cell::from(proc.pm_id.to_string()));
            }
            if show_pid {
                cells.push(Cell::from(
                    proc.pid
                        .map(|pid| pid.to_string())
                        .unwrap_or_else(|| "-".into()),
                ));
            }
            if show_cpu {
                cells.push(Cell::from(
                    proc.cpu
                        .map(|cpu| format!("{cpu:.1}%"))
                        .unwrap_or_else(|| "-".into()),
                ));
            }
            if show_mode {
                cells.push(Cell::from(proc.mode.clone()));
            }
            if show_uptime {
                cells.push(Cell::from(
                    proc.uptime_ms
                        .map(|ms| format_uptime(ms / 1000))
                        .unwrap_or_else(|| "-".into()),
                ));
            }
            Row::new(cells).style(selection_style(
                idx == state.pm2_selected,
                state.pm2_hover_row == Some(idx),
            ))
        })
        .collect();
    let mut labels = vec!["NAME", "STATUS", "RSS"];
    let mut fields = vec![
        Some(SortField::Name),
        Some(SortField::Status),
        Some(SortField::Memory),
    ];
    let mut widths = vec![
        Constraint::Fill(1),
        Constraint::Length(9),
        Constraint::Length(7),
    ];
    if show_id {
        labels.insert(0, "ID");
        fields.insert(0, Some(SortField::Id));
        widths.insert(0, Constraint::Length(4));
    }
    for (show, label, field, width) in [
        (show_pid, "PID", SortField::Pid, 8),
        (show_cpu, "CPU", SortField::Cpu, 7),
        (show_mode, "MODE", SortField::Mode, 8),
        (show_uptime, "UPTIME", SortField::Uptime, 10),
    ] {
        if show {
            labels.push(label);
            fields.push(Some(field));
            widths.push(Constraint::Length(width));
        }
    }
    let title = if state.pm2_error.is_some() {
        " PM2 · cached · F5 retry "
    } else {
        " PM2 "
    };
    let title = format!(
        "{}· {} · s sort ",
        title,
        crate::app::sorting::SortTarget::Pm2
            .sort_label(state.sort_for(crate::app::sorting::SortTarget::Pm2))
    );
    let header = sortable_header(
        state,
        Block::default().borders(Borders::ALL).inner(area),
        SortTarget::Pm2,
        &labels,
        &fields,
        &widths,
    );
    frame.render_widget(
        Table::new(rows, widths)
            .column_spacing(1)
            .header(header)
            .block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
    if pm2_rows.is_empty() {
        let error = state
            .pm2_error
            .as_ref()
            .map(|error| format!("{error} · F5 retry"));
        render_collection_empty(
            frame,
            area,
            state.pm2_error.is_some() || (state.node_loaded && !state.pm2_loading),
            if error.is_some() || !state.pm2_available {
                ""
            } else {
                &state.node_filter
            },
            error.as_deref().unwrap_or(if state.pm2_available {
                "No PM2 processes."
            } else {
                "PM2 unavailable. F5 retries."
            }),
        );
    }
    render_nav_icons(frame, area, state.pm2_scroll, pm2_rows.len(), capacity);
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
    let show_cpu = area.width >= 55;
    let show_script = area.width >= 80;
    let capacity = area.height.saturating_sub(3) as usize;
    let rows: Vec<Row> = node_rows
        .iter()
        .enumerate()
        .skip(state.node_scroll)
        .take(capacity)
        .map(|(idx, row)| match row {
            node::NodeRow::Item { index } => {
                let proc = &node_view[*index];
                let mut cells = vec![
                    Cell::from(proc.pid.to_string()),
                    Cell::from(format_memory(proc.memory_bytes)),
                    Cell::from(proc.name.clone()),
                ];
                if show_cpu {
                    cells.push(Cell::from(format!("{:.1}%", proc.cpu)));
                }
                if show_script {
                    cells.push(Cell::from(proc.script.clone()));
                }
                Row::new(cells).style(selection_style(
                    state.selected == idx,
                    state.hover_row == Some(idx),
                ))
            }
            node::NodeRow::Group { name, count } => {
                Row::new(vec![Cell::from(format!("{name} ({count})")).style(
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                )])
            }
        })
        .collect();
    let mut labels = vec!["PID", "RSS", "NAME"];
    let mut fields = vec![
        Some(SortField::Pid),
        Some(SortField::Memory),
        Some(SortField::Name),
    ];
    let mut widths = vec![
        Constraint::Length(8),
        Constraint::Length(7),
        Constraint::Fill(1),
    ];
    if show_cpu {
        labels.push("CPU");
        fields.push(Some(SortField::Cpu));
        widths.push(Constraint::Length(7));
    }
    if show_script {
        labels.push("SCRIPT");
        fields.push(Some(SortField::Script));
        widths.push(Constraint::Fill(1));
    }
    let header = sortable_header(
        state,
        Block::default().borders(Borders::ALL).inner(area),
        SortTarget::Node,
        &labels,
        &fields,
        &widths,
    );
    frame.render_widget(
        Table::new(rows, widths)
            .column_spacing(1)
            .header(header)
            .block(Block::default().borders(Borders::ALL).title(format!(
                    " Node.js · {} · s sort ",
                    crate::app::sorting::SortTarget::Node
                        .sort_label(state.sort_for(crate::app::sorting::SortTarget::Node))
                ))),
        area,
    );
    if node_rows.is_empty() {
        render_collection_empty(
            frame,
            area,
            state.node_loaded,
            &state.node_filter,
            "No Node processes.",
        );
    }
    for (offset, row) in node_rows
        .iter()
        .skip(state.node_scroll)
        .take(capacity)
        .enumerate()
    {
        if let node::NodeRow::Group { name, count } = row {
            frame.render_widget(
                Paragraph::new(format!("{name} ({count})")).style(
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ),
                Rect::new(
                    area.x + 1,
                    area.y + 2 + offset as u16,
                    area.width.saturating_sub(2),
                    1,
                ),
            );
        }
    }
    render_nav_icons(frame, area, state.node_scroll, node_rows.len(), capacity);
}

/// Draws the shared two-column header, or the compact stacked version.
pub(super) fn render_collection_header(
    frame: &mut Frame,
    layout: super::layout::CollectionHeader,
    title: &str,
    details: Vec<Line<'_>>,
    filter: &str,
    active: bool,
) {
    let title_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    if layout.columns {
        frame.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {title} "))
                .border_style(Style::default().fg(Color::Cyan)),
            layout.area,
        );
        let divider_x = layout.details.right() + 1;
        for y in layout.area.y..layout.area.bottom() {
            let symbol = if y == layout.area.y {
                "┬"
            } else if y == layout.area.bottom() - 1 {
                "┴"
            } else {
                "│"
            };
            frame.render_widget(
                Paragraph::new(symbol).style(Style::default().fg(Color::Cyan)),
                Rect::new(divider_x, y, 1, 1),
            );
        }
        frame.render_widget(
            Paragraph::new(" Search ").style(if active {
                title_style
            } else {
                Style::default().fg(Color::Gray)
            }),
            Rect::new(layout.search.x, layout.area.y, layout.search.width, 1),
        );
    } else {
        frame.render_widget(
            Paragraph::new(format!(" {title}")).style(title_style),
            Rect::new(layout.area.x, layout.area.y, layout.area.width, 1),
        );
    }
    frame.render_widget(Paragraph::new(details), layout.details);
    render_search_box(frame, layout.search, filter, active);
}

fn render_search_box(frame: &mut Frame, area: Rect, filter: &str, is_active: bool) {
    let style = if is_active {
        Style::default().bg(Color::Cyan).fg(Color::Black)
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
        .block(if area.height >= 3 {
            Block::default().borders(Borders::ALL).title("Search")
        } else {
            Block::default()
        });

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
    let menu_area = super::layout::context_menu_area(
        main_area,
        menu.x,
        menu.y,
        &labels,
        menu.header.as_deref(),
    );
    if menu_area.width < 3 || menu_area.height < 3 {
        return;
    }

    // Clear the entire menu area first to prevent text bleeding through
    frame.render_widget(ratatui::widgets::Clear, menu_area);

    // Render bordered box with background
    let block = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().bg(Color::Black).fg(Color::White));
    frame.render_widget(block, menu_area);

    // Render items - full width with consistent padding and explicit background
    let inner = Rect::new(
        menu_area.x + 1,
        menu_area.y + 1,
        menu_area.width - 2,
        menu_area.height - 2,
    );
    let inner_width = inner.width as usize;
    let mut row_y = inner.y;

    if let Some(header) = menu.header.as_ref() {
        let padded = format!(" {}", truncate(header, inner_width.saturating_sub(1)));
        let line = Line::from(Span::styled(
            padded,
            Style::default()
                .bg(Color::Black)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(
            Paragraph::new(line),
            Rect::new(inner.x, row_y, inner.width, 1),
        );
        row_y = row_y.saturating_add(1);
    }

    let capacity = inner
        .height
        .saturating_sub(u16::from(menu.header.is_some())) as usize;
    let scroll = menu
        .hover
        .unwrap_or(0)
        .saturating_sub(capacity.saturating_sub(1));
    for (i, label) in labels.iter().enumerate().skip(scroll).take(capacity) {
        let is_hovered = menu.hover == Some(i);
        let style = if is_hovered {
            Style::default().bg(Color::White).fg(Color::Black)
        } else {
            Style::default().bg(Color::Black).fg(Color::White)
        };
        let y = row_y + (i - scroll) as u16;
        if y < inner.y + inner.height {
            // Pad label to full width: " Label" + spaces to fill
            let padded = format!(" {:<width$}", label, width = inner_width.saturating_sub(1));
            let line = Line::from(Span::styled(padded, style));
            frame.render_widget(Paragraph::new(line), Rect::new(inner.x, y, inner.width, 1));
        }
    }
}

fn render_prune_confirm(frame: &mut Frame, state: &AppState, bounds: Rect) {
    let scope = match state.pending_prune {
        Some(crate::app::ContextMenuAction::PruneBuildCache) => "unused build cache",
        Some(crate::app::ContextMenuAction::PruneDanglingImages) => "all unused images",
        Some(crate::app::ContextMenuAction::PruneVolumes) => "unused anonymous volumes",
        _ => return,
    };
    let (area, yes, no) = super::layout::prune_confirmation(bounds);
    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title(" Confirm Prune "),
        area,
    );
    let inner = Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(4),
    );
    frame.render_widget(Paragraph::new(format!("Remove {scope}?\nCannot be undone.\nReferenced items are kept.\nY confirms · N/Esc cancels")).wrap(Wrap { trim: false }), inner);
    for (button, text, hovered) in [
        (
            yes,
            "[ Yes ]",
            state.pending_prune_hover == Some(crate::app::PruneConfirmChoice::Yes),
        ),
        (
            no,
            "[ No ]",
            state.pending_prune_hover == Some(crate::app::PruneConfirmChoice::No),
        ),
    ] {
        frame.render_widget(
            Paragraph::new(text).style(if hovered {
                Style::default().bg(Color::Cyan).fg(Color::Black)
            } else {
                Style::default()
            }),
            button,
        );
    }
}

fn render_delete_confirm(frame: &mut Frame, state: &AppState, main_area: Rect) {
    let Some(confirm) = state.pending_delete.as_ref() else {
        return;
    };
    let (kind_label, warning_line) = match confirm.kind {
        DeleteKind::Image => (
            "image",
            "WARNING! This will remove the image and any untagged layers.",
        ),
        DeleteKind::Container => (
            "container",
            "WARNING! This will remove the container and its writable layer.",
        ),
        DeleteKind::Volume => (
            "volume",
            "WARNING! The volume, attached containers and their writable data will be removed.",
        ),
    };

    let mut text = vec![Line::from(if confirm.kind == DeleteKind::Volume {
        "Delete volume + containers?".to_string()
    } else {
        format!("Delete {kind_label} and ALL DATA?")
    })];
    if confirm.kind == DeleteKind::Volume {
        text.push(Line::from("Running containers stop."));
    }
    text.extend([
        Line::from(confirm.name.clone()),
        Line::from(""),
        Line::from(warning_line),
        Line::from(if confirm.kind == DeleteKind::Container {
            "If running, it will be stopped and removed."
        } else if confirm.kind == DeleteKind::Volume {
            "Large volumes can take several minutes to remove."
        } else {
            ""
        }),
    ]);
    let (area, yes_area, no_area) = super::layout::delete_confirmation(main_area);
    frame.render_widget(ratatui::widgets::Clear, area);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title(" Confirm Delete ")
            .title_bottom(" Y: delete | N/Esc: cancel "),
        area,
    );
    let inner = Rect::new(
        area.x + 2,
        area.y + 1,
        area.width.saturating_sub(4),
        area.height.saturating_sub(5),
    );
    frame.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner);

    let yes_hover = state.pending_delete_hover == Some(DeleteConfirmChoice::Yes);
    let no_hover = state.pending_delete_hover == Some(DeleteConfirmChoice::No);
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
    frame.render_widget(Paragraph::new(yes_text), yes_area);
    frame.render_widget(Paragraph::new(no_text), no_area);
}

fn render_log_progress(frame: &mut Frame, state: &AppState, main_area: Rect, label: &str) {
    let prefix = if state.log_output_mode == LogOutputMode::Inspect {
        "Loading inspect for "
    } else {
        "Loading logs for "
    };
    let mut text = vec![Line::from(vec![
        Span::styled(prefix, Style::default().fg(Color::White)),
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
    let block = if state.log_output_mode == LogOutputMode::Inspect {
        Block::default().borders(Borders::ALL).title(" Inspect ")
    } else {
        Block::default().borders(Borders::ALL).title(" Logs ")
    };
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
    let base_title = if state.log_select_mode {
        format!("{} [SELECT]", output.title)
    } else {
        output.title.clone()
    };
    let title_max = width.saturating_sub(4) as usize;
    let title = if title_max > 0 {
        truncate(&base_title, title_max)
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
    frame.render_widget(Paragraph::new(lines), inner);

    let button_w = 12u16;
    let button_y = area.y + area.height.saturating_sub(3);
    let close_hover = state.log_output_hover;
    let close_style = if close_hover {
        Style::default().bg(Color::Cyan).fg(Color::Black)
    } else {
        Style::default().bg(Color::Black).fg(Color::White)
    };
    if state.log_output_mode == LogOutputMode::Logs {
        let gap = 4u16;
        let total_w = button_w.saturating_mul(2).saturating_add(gap);
        let button_x = area.x + (area.width.saturating_sub(total_w)) / 2;
        let select_hover = state.log_select_hover;
        let select_label = if state.log_select_mode {
            " [ Live ] "
        } else {
            " [ Select ] "
        };
        let select_style = if select_hover {
            Style::default().bg(Color::Cyan).fg(Color::Black)
        } else {
            Style::default().bg(Color::Black).fg(Color::White)
        };
        let select_line = Line::from(Span::styled(select_label, select_style));
        frame.render_widget(
            Paragraph::new(select_line),
            Rect::new(button_x, button_y, button_w, 1),
        );

        let close_x = button_x + button_w + gap;
        let close_line = Line::from(Span::styled(" [ Close ] ", close_style));
        frame.render_widget(
            Paragraph::new(close_line),
            Rect::new(close_x, button_y, button_w, 1),
        );
    } else {
        let close_x = area.x + (area.width.saturating_sub(button_w)) / 2;
        let close_line = Line::from(Span::styled(" [ Close ] ", close_style));
        frame.render_widget(
            Paragraph::new(close_line),
            Rect::new(close_x, button_y, button_w, 1),
        );
    }
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
    let block = Block::default().borders(Borders::ALL);
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
    frame.render_widget(
        Paragraph::new(line),
        Rect::new(button_x, button_y, button_w, 1),
    );
}

fn render_docker_list_modal(frame: &mut Frame, state: &AppState, main_area: Rect) {
    let kind = match state.docker_list_kind {
        Some(kind) => kind,
        None => return,
    };
    let title = match kind {
        DockerListKind::Images => "Docker Images",
        DockerListKind::Containers => "Docker Containers",
        DockerListKind::Volumes => "Docker Volumes",
    };

    let max_width = main_area.width.saturating_sub(2).max(4);
    let max_height = main_area.height.saturating_sub(2).max(6);
    let width = (main_area.width.saturating_mul(88) / 100)
        .max(70)
        .max(title.len() as u16 + 24)
        .min(max_width);
    let height = (main_area.height.saturating_mul(80) / 100)
        .max(14)
        .min(max_height);
    let x = main_area.x + (main_area.width.saturating_sub(width)) / 2;
    let y = main_area.y + (main_area.height.saturating_sub(height)) / 2;
    let area = Rect::new(x, y, width, height);

    frame.render_widget(ratatui::widgets::Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(
            " {} · {} ",
            title,
            state.sort_for(state.sort_target()).label()
        ))
        .title_bottom(if kind == DockerListKind::Volumes {
            " Enter details | c containers | s sort "
        } else {
            " i inspect | s sort | F10 actions "
        });
    frame.render_widget(block, area);

    let list_area = Rect::new(
        area.x + 2,
        area.y + super::layout::docker_list_top_padding(area.height),
        area.width.saturating_sub(4),
        super::layout::docker_list_content_height(area.height, kind == DockerListKind::Volumes),
    );
    let header_height = super::layout::docker_list_header_height(area.height);
    let visible_height = list_area.height.saturating_sub(header_height) as usize;
    let total = state.docker_list_items.len();
    let selected = if total > 0 {
        state.docker_list_selected.min(total - 1)
    } else {
        0
    };
    let scroll_offset = if total <= visible_height || visible_height == 0 {
        0
    } else {
        let max_offset = total.saturating_sub(visible_height);
        let ideal = selected.saturating_sub(visible_height / 2);
        ideal.min(max_offset)
    };

    let placeholder = if state.docker_list_request.is_some() {
        format!(
            "{} Loading... Esc closes this window.",
            state.spinner_char()
        )
    } else if let Some(error) = &state.docker_list_error {
        format!("Unable to load: {error}\nF5 retries. Esc closes this window.")
    } else {
        "No items found. F5 refreshes this list.".into()
    };
    let rows: Vec<Row> = if total == 0 {
        vec![Row::new(vec![
            Cell::from("No items found"),
            Cell::from(""),
            Cell::from(""),
        ])]
    } else {
        state
            .docker_list_items
            .iter()
            .skip(scroll_offset)
            .take(visible_height.max(1))
            .enumerate()
            .map(|(i, item)| {
                let actual = scroll_offset + i;
                let style = if actual == selected {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                };
                let cells = if kind == DockerListKind::Volumes {
                    vec![
                        Cell::from(item.name.clone()).style(
                            if item.attachments.as_ref().is_some_and(Vec::is_empty) {
                                Style::default()
                                    .fg(Color::Gray)
                                    .add_modifier(Modifier::ITALIC)
                            } else if item
                                .attachments
                                .as_ref()
                                .is_some_and(|entries| !entries.is_empty())
                            {
                                Style::default().add_modifier(Modifier::BOLD)
                            } else {
                                Style::default()
                            },
                        ),
                        Cell::from(item.size.clone()),
                        Cell::from(item.activity.as_deref().unwrap_or("Unknown")),
                    ]
                } else {
                    vec![
                        Cell::from(item.name.clone()),
                        Cell::from(item.id.clone()),
                        Cell::from(item.size.clone()),
                    ]
                };
                Row::new(cells).style(style)
            })
            .collect()
    };

    let (labels, fields, widths) = if kind == DockerListKind::Volumes {
        (
            ["NAME", "SIZE", "ACTIVITY"],
            [
                Some(SortField::Name),
                Some(SortField::Size),
                Some(SortField::Activity),
            ],
            [
                Constraint::Fill(3),
                Constraint::Length(9),
                Constraint::Fill(3),
            ],
        )
    } else {
        (
            ["NAME", "ID", "SIZE"],
            [
                Some(SortField::Name),
                Some(SortField::Id),
                Some(SortField::Size),
            ],
            [
                Constraint::Fill(3),
                Constraint::Fill(2),
                Constraint::Length(10),
            ],
        )
    };
    let header = if total > 0 && header_height > 0 {
        sortable_header(
            state,
            list_area,
            state.sort_target(),
            &labels,
            &fields,
            &widths,
        )
    } else {
        Row::default()
    };
    let table = Table::new(rows, widths).column_spacing(1);
    let table = if header_height > 0 {
        table.header(header)
    } else {
        table
    };
    if total == 0 {
        frame.render_widget(
            Paragraph::new(placeholder).wrap(Wrap { trim: true }),
            list_area,
        );
    } else {
        frame.render_widget(table, list_area);
    }
    if let Some(item) = state.docker_list_items.get(selected) {
        let is_volume = kind == DockerListKind::Volumes;
        let width = area.width.saturating_sub(4);
        let lines = if is_volume {
            let mut lines = vec![
                Line::from(truncate(&item.detail_left, width as usize)),
                Line::from(truncate(&item.detail_right, width as usize)),
            ];
            if area.height >= 12 {
                lines.push(Line::from(truncate(&item.detail_project, width as usize)));
            } else {
                lines[1] = Line::from(truncate(&item.detail_project, width as usize));
            }
            lines
        } else {
            vec![Line::from(format!(
                "{}  {}",
                item.detail_left, item.detail_right
            ))]
        };
        frame.render_widget(
            Paragraph::new(lines).style(Style::default().fg(Color::Gray)),
            Rect::new(
                area.x + 2,
                area.bottom()
                    .saturating_sub(if is_volume && area.height >= 12 {
                        6
                    } else if is_volume {
                        5
                    } else {
                        4
                    }),
                width,
                if is_volume && area.height >= 12 {
                    3
                } else if is_volume {
                    2
                } else {
                    1
                },
            ),
        );
    }

    let button_w = 10u16;
    let button_y = area.y + area.height.saturating_sub(3);
    let button_x = area.x + (area.width.saturating_sub(button_w)) / 2;
    let hover = state.docker_list_hover;
    let style = if hover {
        Style::default().bg(Color::Cyan).fg(Color::Black)
    } else {
        Style::default().bg(Color::Black).fg(Color::White)
    };
    let line = Line::from(Span::styled(" [ Close ] ", style));
    frame.render_widget(
        Paragraph::new(line),
        Rect::new(button_x, button_y, button_w, 1),
    );
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

    frame.render_widget(
        Paragraph::new(cpu_line),
        Rect::new(area.x, area.y, area.width, 1),
    );
    frame.render_widget(
        Paragraph::new(mem_line),
        Rect::new(area.x, area.y + 1, area.width, 1),
    );
    frame.render_widget(
        Paragraph::new(swap_line),
        Rect::new(area.x, area.y + 2, area.width, 1),
    );
    frame.render_widget(
        Paragraph::new(disk_line),
        Rect::new(area.x, area.y + 3, area.width, 1),
    );
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

pub(super) fn format_memory(bytes: u64) -> String {
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
    if Span::raw(s).width() <= max_len {
        return s.to_string();
    }
    if max_len <= 3 {
        return ".".repeat(max_len);
    }
    let mut output = String::new();
    let mut width = 0;
    for ch in s.chars() {
        let next_width = Span::raw(ch.to_string()).width();
        if width + next_width > max_len - 3 {
            break;
        }
        output.push(ch);
        width += next_width;
    }
    output.push_str("...");
    output
}

/// Render docker system df stats as a table showing disk usage for images, containers, volumes, and build cache
fn render_docker_df_stats(frame: &mut Frame, state: &AppState, area: Rect, df: &DockerSystemDf) {
    if state.docker_df_updated_at.is_none() {
        let text = state
            .docker_df_error
            .as_deref()
            .unwrap_or("Loading disk usage...");
        frame.render_widget(
            Paragraph::new(text).wrap(Wrap { trim: true }).block(
                Block::default()
                    .title(" Docker Disk Usage ")
                    .borders(Borders::ALL),
            ),
            area,
        );
        return;
    }
    let hover_row = state.docker_df_hover;

    // Build table rows - 5 rows total (header + 4 data)
    let rows = [
        (
            "Images",
            df.images_total,
            df.images_active,
            &df.images_size,
            &df.images_reclaimable,
            &df.images_reclaimable_pct,
        ),
        (
            "Containers",
            df.containers_total,
            df.containers_active,
            &df.containers_size,
            &df.containers_reclaimable,
            &df.containers_reclaimable_pct,
        ),
        (
            "Volumes",
            df.volumes_total,
            df.volumes_active,
            &df.volumes_size,
            &df.volumes_reclaimable,
            &df.volumes_reclaimable_pct,
        ),
        (
            "Build Cache",
            df.build_cache_total as u32,
            0,
            &df.build_cache_size,
            &df.build_cache_reclaimable,
            &df.build_cache_reclaimable_pct,
        ),
    ];

    let table_rows: Vec<Row> = rows
        .iter()
        .enumerate()
        .map(
            |(i, &(name, total, active, size, reclaimable, reclaimable_pct))| {
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
                    Cell::from(name).style(
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Cell::from(total.to_string()),
                    Cell::from(active.to_string()),
                    Cell::from(size.clone()),
                    Cell::from(reclaimable_display).style(Style::default().fg(Color::DarkGray)),
                ])
                .style(row_style)
            },
        )
        .collect();

    let header = Row::new(vec![
        Cell::from("Type").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Total").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Active").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Size").style(Style::default().add_modifier(Modifier::BOLD)),
        Cell::from("Reclaimable").style(Style::default().add_modifier(Modifier::BOLD)),
    ]);

    // Use fixed widths instead of percentages to ensure consistent rendering
    let widths = vec![
        Constraint::Fill(2),
        Constraint::Length(5),
        Constraint::Length(6),
        Constraint::Fill(2),
        Constraint::Fill(3),
    ];

    let table = Table::new(table_rows, widths).header(header).block(
        Block::default()
            .title(if state.docker_df_error.is_some() {
                " Docker Disk Usage · STALE · F5 retry "
            } else {
                " Docker Disk Usage "
            })
            .borders(Borders::ALL),
    );

    frame.render_widget(table, area);
}
