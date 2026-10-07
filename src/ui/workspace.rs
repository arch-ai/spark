//! Shared, responsive inspector and project work surface.
use crate::app::{
    history,
    projects::Resource,
    sorting::{SortField, SortTarget},
    workspace::{Cleanup, EditorKind, InspectorTab},
    workspace_config, AppState, InputMode, ViewMode,
};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Clear, Paragraph, Row, Sparkline, Table, Wrap},
    Frame,
};

pub fn panes(area: Rect, open: bool) -> (Rect, Option<Rect>) {
    if !open {
        return (area, None);
    }
    if area.width >= 114 && area.height >= 16 {
        let left = (area.width * 55 / 100).max(60);
        (
            Rect::new(area.x, area.y, left, area.height),
            Some(Rect::new(
                area.x + left,
                area.y,
                area.width - left,
                area.height,
            )),
        )
    } else {
        (Rect::new(area.x, area.y, 0, 0), Some(area))
    }
}
pub fn project_layout(area: Rect) -> [Rect; 3] {
    let header = super::layout::collection_header(area, ViewMode::Projects);
    let chunks = Layout::vertical([
        Constraint::Length(header.area.height),
        Constraint::Min(3),
        Constraint::Length(if area.height >= 16 { 2 } else { 1 }),
    ])
    .split(area);
    [chunks[0], chunks[1], chunks[2]]
}
pub fn inspector_body(area: Rect) -> Rect {
    Rect::new(
        area.x + 1,
        area.y + 3,
        area.width.saturating_sub(2),
        area.height.saturating_sub(6),
    )
}
pub fn project_resources(body: Rect) -> Rect {
    let offset = if body.height < 8 { 1 } else { 3 };
    Rect::new(
        body.x,
        body.y + offset,
        body.width,
        body.height.saturating_sub(offset),
    )
}
pub fn storage_table(body: Rect) -> Rect {
    let compact = body.height < 7;
    Rect::new(
        body.x,
        body.y + if compact { 1 } else { 2 },
        body.width,
        body.height.saturating_sub(if compact { 1 } else { 4 }),
    )
}
pub fn tab_hits(area: Rect) -> Vec<(Rect, InspectorTab)> {
    if area.width < 50 {
        return Vec::new();
    }
    let mut x = area.x + 1;
    InspectorTab::ALL
        .iter()
        .map(|tab| {
            let rect = Rect::new(x, area.y + 1, (tab.label().len() + 2) as u16, 1);
            x += rect.width;
            (rect, *tab)
        })
        .collect()
}
fn normal() -> Style {
    Style::default().fg(Color::Gray)
}
fn selected() -> Style {
    Style::default().fg(Color::Black).bg(Color::Cyan)
}
fn bytes(value: u64) -> String {
    super::render::format_memory(value)
}
fn memory(value: u64, estimate: bool) -> String {
    format!("{}{}", if estimate { "~" } else { "" }, bytes(value))
}
fn text(
    frame: &mut Frame,
    area: Rect,
    content: impl Into<ratatui::text::Text<'static>>,
    style: Style,
) {
    frame.render_widget(Paragraph::new(content).style(style), area);
}

pub fn render_projects(frame: &mut Frame, state: &AppState, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let chunks = project_layout(area);
    let workspace = &state.workspace;
    let rows = workspace.visible_projects(state.sort_for(SortTarget::Projects));
    let summary = if let Some(error) = &workspace.catalog_error {
        format!("Cached resources · {error}")
    } else if workspace.catalog_loading && workspace.projects.is_empty() {
        "Discovering projects…".into()
    } else {
        format!(
            "{} shown / {} projects · Enter inspect · C configure",
            rows.len(),
            workspace.projects.len()
        )
    };
    super::render::render_collection_header(
        frame,
        super::layout::collection_header(area, ViewMode::Projects),
        "PROJECTS",
        vec![Line::from(summary)],
        &workspace.filter,
        state.input_mode == InputMode::Filter,
    );
    if rows.is_empty() {
        frame.render_widget(Paragraph::new(if workspace.catalog_loading{"Discovering resources in the background…"}else{"No projects match. Clear the filter with x, or press C to add a stopped project and its scripts."}).wrap(Wrap{trim:true}),chunks[1]);
    }
    let wide = area.width >= 68;
    let mut widths = vec![
        Constraint::Min(15),
        Constraint::Length(7),
        Constraint::Length(9),
        Constraint::Length(7),
    ];
    let mut headers = vec!["PROJECT", "CPU", "RAM", "ITEMS"];
    let mut fields = vec![
        Some(SortField::Name),
        Some(SortField::Cpu),
        Some(SortField::Memory),
        None,
    ];
    if wide {
        widths.push(Constraint::Min(16));
        headers.push("DIRECTORY");
        fields.push(None);
    }
    let block = Block::default().borders(Borders::ALL).title(" Projects ");
    let inner = block.inner(chunks[1]);
    let header = super::render::sortable_header(
        state,
        inner,
        SortTarget::Projects,
        &headers,
        &fields,
        &widths,
    );
    let table_rows = rows
        .iter()
        .enumerate()
        .skip(workspace.scroll)
        .take(inner.height.saturating_sub(1) as usize)
        .map(|(index, project)| {
            let favorite = workspace.preferences.favorites.contains(&project.key);
            let mut cells = vec![
                format!("{}{}", if favorite { "* " } else { "" }, project.name),
                if project.unmeasured > 0 {
                    format!("{:.0}%+?", project.cpu)
                } else {
                    format!("{:.1}%", project.cpu)
                },
                format!(
                    "{}{}",
                    memory(project.memory, project.estimated),
                    if project.unmeasured > 0 { "+?" } else { "" }
                ),
                project.resources.len().to_string(),
            ];
            if wide {
                cells.push(
                    project
                        .path
                        .clone()
                        .unwrap_or_else(|| "Docker context".into()),
                );
            }
            Row::new(cells).style(if index == workspace.selected {
                selected()
            } else if project.resources.is_empty() {
                normal().add_modifier(Modifier::ITALIC)
            } else {
                normal()
            })
        });
    frame.render_widget(
        Table::new(table_rows, widths)
            .header(header)
            .column_spacing(1)
            .block(block),
        chunks[1],
    );
    text(frame,chunks[2],"Enter inspect · F2 inspector · f favorite · W saved filters\nC configure · D dev · P prod · / filter · s sort · F5 refresh",normal());
}

pub fn render_inspector(frame: &mut Frame, state: &AppState, area: Rect) {
    let workspace = &state.workspace;
    let Some(inspector) = &workspace.inspector else {
        return;
    };
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(
                " {} · {} ",
                inspector.record.resource.kind(),
                inspector.record.name
            ))
            .border_style(Style::default().fg(Color::Cyan)),
        area,
    );
    let hits = tab_hits(area);
    if hits.is_empty() {
        text(
            frame,
            Rect::new(area.x + 1, area.y + 1, area.width.saturating_sub(2), 1),
            format!("{} · Tab next tab", inspector.tab.label()),
            selected(),
        );
    }
    for (rect, tab) in hits {
        text(
            frame,
            rect,
            format!(" {} ", tab.label()),
            if tab == inspector.tab {
                selected()
            } else {
                normal()
            },
        );
    }
    let body = inspector_body(area);
    let footer = Rect::new(
        area.x + 1,
        area.bottom().saturating_sub(3),
        area.width.saturating_sub(2),
        2,
    );
    match inspector.tab {
        InspectorTab::Details => {
            if let Resource::Project(key) = &inspector.record.resource {
                if let Some(project) = workspace.project(key) {
                    let partial = if project.unmeasured > 0 { " + ?" } else { "" };
                    text(frame,Rect::new(body.x,body.y,body.width,project_resources(body).y-body.y),format!("{}\nNative CPU {:.1}%{} · RAM {}{}\nEnter inspect resource · o jump to owner",project.path.as_deref().unwrap_or("Directory unavailable"),project.cpu,partial,memory(project.memory,project.estimated),partial),normal());
                    let rows_area = project_resources(body);
                    let visible = rows_area.height.saturating_sub(1) as usize;
                    let start = inspector
                        .resource_selected
                        .saturating_sub(visible.saturating_sub(1));
                    let rows = project
                        .resources
                        .iter()
                        .enumerate()
                        .skip(start)
                        .take(visible)
                        .map(|(index, r)| {
                            Row::new([r.resource.kind().to_string(), r.name.clone()]).style(
                                if index == inspector.resource_selected {
                                    selected()
                                } else {
                                    normal()
                                },
                            )
                        });
                    frame.render_widget(
                        Table::new(rows, [Constraint::Length(7), Constraint::Min(10)])
                            .header(
                                Row::new(["TYPE", "RESOURCE"])
                                    .style(Style::default().fg(Color::Cyan)),
                            )
                            .column_spacing(1),
                        rows_area,
                    );
                } else {
                    text(
                        frame,
                        body,
                        "Project is no longer in the latest snapshot. Esc returns to its table.",
                        normal(),
                    );
                }
            } else {
                let mut lines = inspector.record.details.clone();
                if let Resource::Container(id) = &inspector.record.resource {
                    lines.extend(state.docker_memory.details(id));
                }
                if let Some(path) = &inspector.record.path {
                    lines.push(format!("Directory: {path}"));
                }
                if let Some(command) = &inspector.record.command {
                    lines.push(format!("Command: {command}"));
                }
                let latest = workspace
                    .history
                    .samples
                    .get(&inspector.record.resource.key())
                    .and_then(|samples| samples.back());
                if let Some(cpu) = latest.map(|m| m.cpu).or(inspector.record.cpu) {
                    lines.push(format!("CPU: {cpu:.1}%"));
                }
                if let Some(memory_bytes) = latest.map(|m| m.memory).or(inspector.record.memory) {
                    lines.push(format!(
                        "RAM: {}",
                        memory(
                            memory_bytes,
                            latest.map_or(inspector.record.estimated, |m| m.estimated)
                        )
                    ));
                }
                if inspector.detail_request.is_some() {
                    lines.push("Loading process command and directory…".into());
                }
                frame.render_widget(
                    Paragraph::new(lines.join("\n"))
                        .style(normal())
                        .wrap(Wrap { trim: false })
                        .scroll((inspector.scroll.min(u16::MAX as usize) as u16, 0)),
                    body,
                );
            }
            text(frame,footer,"y copy path · Y copy command · o owner\nTab switch tab · Backspace back · Esc close",normal());
        }
        InspectorTab::Logs => {
            text(
                frame,
                Rect::new(area.x + 1, area.y + 2, area.width.saturating_sub(2), 1),
                if inspector.searching {
                    format!("Search: {}_", inspector.query)
                } else {
                    format!(
                        "{} · {}",
                        if inspector.frozen.is_some() {
                            "Paused"
                        } else if inspector.follow {
                            "Follow"
                        } else {
                            "Scrolled"
                        },
                        inspector.stream_status
                    )
                },
                if inspector.searching {
                    selected()
                } else {
                    normal()
                },
            );
            let logs = inspector.visible_logs();
            let start = if inspector.follow && inspector.frozen.is_none() {
                logs.len().saturating_sub(body.height as usize)
            } else {
                inspector
                    .scroll
                    .min(logs.len().saturating_sub(body.height as usize))
            };
            let lines: Vec<_> = logs
                .iter()
                .skip(start)
                .take(body.height as usize)
                .map(|line| {
                    Line::styled(
                        line.text.clone(),
                        Style::default().fg(if line.error {
                            Color::LightRed
                        } else {
                            Color::Gray
                        }),
                    )
                })
                .collect();
            if lines.is_empty() {
                text(
                    frame,
                    body,
                    if inspector.query.is_empty() {
                        "Waiting for live output. Native process logs use the journal; permission errors appear here."
                    } else {
                        "No lines match this search."
                    },
                    normal(),
                );
            }
            frame.render_widget(Paragraph::new(lines), body);
            text(frame,footer,"/ search · p pause · End follow · l retry\ny copy visible lines · ↑↓ scroll · Esc close",normal());
        }
        InspectorTab::Events => {
            let events = workspace.history.for_resource(&inspector.record.resource);
            let lines: Vec<_> = events
                .iter()
                .rev()
                .skip(inspector.scroll)
                .take(body.height as usize)
                .map(|event| {
                    Line::styled(
                        format!("{} UTC  {}", history::clock(event.at), event.message),
                        Style::default().fg(if event.warning {
                            Color::LightRed
                        } else {
                            Color::Gray
                        }),
                    )
                })
                .collect();
            text(
                frame,
                Rect::new(area.x + 1, area.y + 2, area.width.saturating_sub(2), 1),
                workspace
                    .events_error
                    .clone()
                    .unwrap_or_else(|| "Observed this session · newest first".into()),
                normal(),
            );
            if lines.is_empty() {
                text(frame,body,"No observed events for this resource. Native exits have no exit-code evidence; Docker events include exit and health information.",normal());
            }
            frame.render_widget(Paragraph::new(lines), body);
            text(frame,footer,"F5 reconnect Docker events · ↑↓ scroll\nHistory is bounded to this session · Esc close",normal());
        }
        InspectorTab::Trends => {
            let samples = workspace
                .history
                .samples
                .get(&inspector.record.resource.key());
            if let Some(samples) = samples.filter(|samples| !samples.is_empty()) {
                let cpu = trend_points(samples, body.width as usize, true);
                let ram = trend_points(samples, body.width as usize, false);
                let chunks = Layout::vertical([
                    Constraint::Length(2),
                    Constraint::Min(1),
                    Constraint::Length(1),
                    Constraint::Min(1),
                ])
                .split(body);
                let last = samples.back().unwrap();
                text(
                    frame,
                    chunks[0],
                    format!(
                        "{} samples · latest {} UTC\nCPU {:.1}% · RAM {}",
                        samples.len(),
                        history::clock(last.at),
                        last.cpu,
                        memory(last.memory, last.estimated)
                    ),
                    normal(),
                );
                frame.render_widget(
                    Sparkline::default()
                        .data(&cpu)
                        .max(cpu.iter().copied().max().unwrap_or(1).max(1000))
                        .style(Style::default().fg(Color::Cyan)),
                    chunks[1],
                );
                text(frame, chunks[2], "RAM (sample-relative scale)", normal());
                frame.render_widget(
                    Sparkline::default()
                        .data(&ram)
                        .style(Style::default().fg(Color::Gray)),
                    chunks[3],
                );
            } else {
                text(frame,body,"No resource history collected yet. Trends use native CPU and RAM snapshots. Live Docker memory is shown in Details.",normal());
            }
            text(frame,footer,"Up to 300 samples per resource · ~ estimated\nSampling gaps remain gaps · Tab switch · Esc close",normal());
        }
        InspectorTab::Storage => {
            let rows = workspace.storage_items();
            let known: Vec<_> = rows
                .iter()
                .filter_map(|v| crate::app::sorting::size_bytes(&v.size))
                .collect();
            let unknown = rows.len() - known.len();
            text(
                frame,
                Rect::new(
                    body.x,
                    body.y,
                    body.width,
                    if body.height < 7 { 1 } else { 2 },
                ),
                format!(
                    "{} volumes · {} known{} · {} selected\n{}",
                    rows.len(),
                    storage_bytes(
                        known
                            .iter()
                            .fold(0u64, |sum, size| sum.saturating_add(*size))
                    ),
                    if unknown > 0 {
                        format!(" + {unknown} unknown")
                    } else {
                        String::new()
                    },
                    workspace.selected_volumes.len(),
                    if let Some(error) = &workspace.storage_error {
                        format!("Cached data · {error}")
                    } else if workspace.storage_request.is_some() {
                        "Loading sizes and owners…".into()
                    } else {
                        "Space select · Delete review cleanup · F5 refresh".into()
                    }
                ),
                normal(),
            );
            let table_area = storage_table(body);
            let count = table_area.height.saturating_sub(1) as usize;
            let start = workspace
                .storage_selected
                .saturating_sub(count.saturating_sub(1));
            let table_rows =
                rows.iter()
                    .enumerate()
                    .skip(start)
                    .take(count)
                    .map(|(index, item)| {
                        Row::new([
                            if workspace.selected_volumes.contains(&item.name) {
                                "[x]".into()
                            } else {
                                "[ ]".into()
                            },
                            item.name.clone(),
                            item.size.clone(),
                        ])
                        .style(if index == workspace.storage_selected {
                            selected()
                        } else {
                            normal()
                        })
                    });
            frame.render_widget(
                Table::new(
                    table_rows,
                    [
                        Constraint::Length(3),
                        Constraint::Min(10),
                        Constraint::Length(10),
                    ],
                )
                .header(Row::new(["", "VOLUME", "SIZE"]).style(Style::default().fg(Color::Cyan)))
                .column_spacing(1),
                table_area,
            );
            if let Some(item) = rows
                .get(workspace.storage_selected)
                .filter(|_| body.height >= 7)
            {
                text(
                    frame,
                    Rect::new(body.x, body.bottom().saturating_sub(2), body.width, 2),
                    format!(
                        "{}\n{}",
                        item.activity.as_deref().unwrap_or("Activity unknown"),
                        item.detail_left
                    ),
                    normal(),
                );
            }
            if rows.is_empty() && workspace.storage_request.is_none() {
                text(
                    frame,
                    table_area,
                    if workspace.storage_loaded {
                        "No volumes attached to this project. Unassigned volumes appear as their own project."
                    } else {
                        "Press F5 to load volume sizes and owners."
                    },
                    normal(),
                );
            }
            text(frame,footer,"Space select · Delete review · Enter details\nCleanup removes reviewed containers, then volumes",normal());
        }
        InspectorTab::Run => {
            let key = match &inspector.record.resource {
                Resource::Project(key) => Some(key),
                _ => inspector.record.project.as_ref(),
            };
            let config = key
                .and_then(|key| workspace.project(key))
                .and_then(|p| p.path.as_ref())
                .and_then(|path| {
                    workspace
                        .preferences
                        .projects
                        .iter()
                        .find(|p| &p.path == path)
                });
            let run = key.and_then(|key| workspace.runs.iter().find(|r| &r.key == key));
            let mut lines = Vec::new();
            if let Some(config) = config {
                lines.push(Line::from(format!(
                    "dev: {} → {}",
                    config.stop_prod, config.start_dev
                )));
                lines.push(Line::from(format!(
                    "prod: {} → {}",
                    config.stop_dev, config.start_prod
                )));
            } else {
                lines.push(Line::from(
                    "Configure a project directory and executable scripts with C.",
                ));
            }
            if let Some(run) = run {
                let failed = run.result.as_ref().is_some_and(|r| r.is_err());
                let status = match &run.result {
                    Some(Err(error)) if error.contains("start script was not run") => {
                        "Start blocked: stop script failed".to_string()
                    }
                    Some(Err(_)) => "Script failed · check output".into(),
                    Some(Ok(_)) => format!("{} scripts completed successfully", run.mode.label()),
                    None => format!("{} scripts running… · X cancel", run.mode.label()),
                };
                text(
                    frame,
                    Rect::new(area.x + 1, area.y + 2, area.width.saturating_sub(2), 1),
                    status,
                    Style::default().fg(if failed { Color::LightRed } else { Color::Cyan }),
                );
                lines.extend(run.lines.iter().map(|line| {
                    Line::styled(
                        line.text.clone(),
                        Style::default().fg(if line.error {
                            Color::LightRed
                        } else {
                            Color::Gray
                        }),
                    )
                }));
                if let Some(result) = &run.result {
                    lines.push(Line::styled(
                        result.clone().unwrap_or_else(|e| e.clone()),
                        Style::default().fg(if failed { Color::LightRed } else { Color::Cyan }),
                    ));
                    if result
                        .as_ref()
                        .is_err_and(|error| error.contains("start script was not run"))
                    {
                        lines.push(Line::styled(
                            "start script was not run",
                            Style::default().fg(Color::LightRed),
                        ));
                    }
                }
            }
            let start = if inspector.follow {
                lines.len().saturating_sub(body.height as usize)
            } else {
                inspector.scroll
            };
            frame.render_widget(
                Paragraph::new(
                    lines
                        .into_iter()
                        .skip(start)
                        .take(body.height as usize)
                        .collect::<Vec<_>>(),
                ),
                body,
            );
            text(frame,footer,"C configure · D dev · P prod · X cancel\nStop must succeed before start · Esc close",normal());
        }
    }
}

pub fn dialog_area(bounds: Rect) -> Rect {
    let width = bounds.width.min(110);
    Rect::new(
        bounds.x + (bounds.width - width) / 2,
        bounds.y + 1,
        width,
        bounds.height.saturating_sub(2),
    )
}
pub fn render_dialogs(frame: &mut Frame, state: &AppState, bounds: Rect) {
    let workspace = &state.workspace;
    let area = dialog_area(bounds);
    if workspace.cleanup_open && !matches!(workspace.cleanup, Cleanup::Idle) {
        frame.render_widget(Clear, area);
        frame.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .title(" Volume cleanup ")
                .border_style(Style::default().fg(Color::Cyan)),
            area,
        );
        let body = Rect::new(
            area.x + 1,
            area.y + 1,
            area.width.saturating_sub(2),
            area.height.saturating_sub(4),
        );
        let footer = Rect::new(body.x, area.bottom().saturating_sub(3), body.width, 2);
        let (content,error,help)=match &workspace.cleanup {
            Cleanup::Loading(_)=>("Inspecting exact container references and retained volumes…".into(),false,"Esc cancel review · no resources will be deleted"),
            Cleanup::Running(_)=>("Removing the reviewed containers, then volumes in the background. Large volume removal may take several minutes.".into(),false,"Esc continue in background · F6 shows progress"),
            Cleanup::Review(plan)=>{
                let mut lines=vec![format!("Delete {} volumes and {} containers?",plan.volumes.len(),plan.containers.len()),"Running containers will be removed. Volume data deletion is permanent.".into(),String::new(),"VOLUMES TO DELETE".into()];
                lines.extend(plan.volumes.iter().map(|name|format!("  {name}")));lines.push(String::new());lines.push("CONTAINERS TO REMOVE".into());
                for container in &plan.containers{lines.push(format!("  {} · {} · {}",container.name,container.status,container.image));lines.push(format!("    ID {} · Project {}",container.id,container.project));if !container.retained_volumes.is_empty(){lines.push(format!("    Other volumes retained: {}",container.retained_volumes.join(", ")));}}
                (lines.join("\n"),false,"↑↓ review · y delete this exact batch · n / Esc cancel")
            },Cleanup::Result(text,error)=>(text.clone(),*error,"Esc close · F5 refresh storage before retrying"),Cleanup::Idle=>return,
        };
        frame.render_widget(
            Paragraph::new(content)
                .wrap(Wrap { trim: false })
                .scroll((workspace.review_scroll.min(u16::MAX as usize) as u16, 0))
                .style(Style::default().fg(if error { Color::LightRed } else { Color::Gray })),
            body,
        );
        text(frame, footer, help, normal());
    }
    if let Some(editor) = &workspace.editor {
        frame.render_widget(Clear, area);
        let title = match editor.kind {
            EditorKind::Project => "Project scripts",
            EditorKind::SaveFilter => "Save current filter",
            EditorKind::Filters => "Saved filters",
        };
        frame.render_widget(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {title} "))
                .border_style(Style::default().fg(Color::Cyan)),
            area,
        );
        let body = Rect::new(
            area.x + 1,
            area.y + 1,
            area.width.saturating_sub(2),
            area.height.saturating_sub(4),
        );
        match editor.kind {
            EditorKind::Project => {
                let labels = [
                    "Project name",
                    "Absolute project directory",
                    "Start dev script",
                    "Stop dev script",
                    "Start prod script",
                    "Stop prod script",
                ];
                let values = workspace_config::fields(&editor.config);
                let visible = (body.height / 2).max(1) as usize;
                let start = editor.field.saturating_sub(visible.saturating_sub(1));
                for index in start..(start + visible).min(6) {
                    let y = body.y + ((index - start) * 2) as u16;
                    text(
                        frame,
                        Rect::new(body.x, y, body.width, 1),
                        labels[index],
                        normal(),
                    );
                    text(
                        frame,
                        Rect::new(body.x, y + 1, body.width, 1),
                        format!(
                            "{}{}",
                            values[index],
                            if index == editor.field { "_" } else { "" }
                        ),
                        if index == editor.field {
                            selected()
                        } else {
                            normal()
                        },
                    );
                }
            }
            EditorKind::SaveFilter => text(
                frame,
                body,
                format!("Name: {}_\nFilter: {}", editor.text, state.active_filter()),
                selected(),
            ),
            EditorKind::Filters => {
                if workspace.preferences.filters.is_empty() {
                    text(
                        frame,
                        body,
                        "No saved filters. Press n to name and save the current filter.",
                        normal(),
                    );
                }
                let start = editor
                    .field
                    .saturating_sub(body.height.saturating_sub(1) as usize);
                let lines = workspace
                    .preferences
                    .filters
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(body.height as usize)
                    .map(|(index, f)| {
                        Line::styled(
                            format!(
                                "{} · {}{} · {}",
                                f.name,
                                workspace_config::view_key(f.view),
                                if f.pm2 { " / PM2" } else { "" },
                                f.filter
                            ),
                            if index == editor.field {
                                selected()
                            } else {
                                normal()
                            },
                        )
                    })
                    .collect::<Vec<_>>();
                frame.render_widget(Paragraph::new(lines), body);
            }
        }
        text(
            frame,
            Rect::new(body.x, area.bottom().saturating_sub(3), body.width, 2),
            format!(
                "{}\n{}",
                editor.error.clone().unwrap_or_default(),
                match editor.kind {
                    EditorKind::Project => "Tab next field · Ctrl+S save · Esc cancel",
                    EditorKind::SaveFilter => "Enter save named filter · Esc cancel",
                    EditorKind::Filters =>
                        "Enter apply · n save current · Delete remove · Esc close",
                }
            ),
            if editor.error.is_some() {
                Style::default().fg(Color::LightRed)
            } else {
                normal()
            },
        );
    }
}

fn trend_points(
    samples: &std::collections::VecDeque<history::Metric>,
    width: usize,
    cpu: bool,
) -> Vec<u64> {
    let Some(first) = samples.front() else {
        return vec![];
    };
    let last = samples.back().unwrap();
    let span = last
        .at
        .duration_since(first.at)
        .unwrap_or_default()
        .as_secs_f64();
    let mut data = vec![0; width];
    for sample in samples {
        let index = if span > 0.0 {
            ((sample
                .at
                .duration_since(first.at)
                .unwrap_or_default()
                .as_secs_f64()
                / span)
                * (width.saturating_sub(1) as f64))
                .round() as usize
        } else {
            width.saturating_sub(1)
        };
        if let Some(value) = data.get_mut(index) {
            *value = (*value).max(if cpu {
                (sample.cpu.max(0.0) * 10.0) as u64
            } else {
                sample.memory
            });
        }
    }
    data
}

fn storage_bytes(value: u64) -> String {
    if value >= 1_000_000_000_000 {
        format!("{:.1} TB", value as f64 / 1e12)
    } else if value >= 1_000_000_000 {
        format!("{:.1} GB", value as f64 / 1e9)
    } else if value >= 1_000_000 {
        format!("{:.1} MB", value as f64 / 1e6)
    } else if value >= 1000 {
        format!("{:.1} KB", value as f64 / 1e3)
    } else {
        format!("{value} B")
    }
}
