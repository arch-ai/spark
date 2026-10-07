use super::{
    launcher::Mode,
    projects::{Resource, ResourceRecord},
    sorting::SortTarget,
    workspace::{Cleanup, Editor, EditorKind, InspectorTab},
    workspace_config::{self, SavedFilter},
    AppState, Focus, InputMode, NodeTab, ViewMode,
};
use crate::system::{docker::ContainerInfo, node::Pm2Process, ports::PortInfo};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

fn selected_project(state: &AppState) -> Option<String> {
    state
        .workspace
        .visible_projects(state.sort_for(SortTarget::Projects))
        .get(state.workspace.selected)
        .map(|p| p.key.clone())
}
fn inspector_project(state: &AppState) -> Option<String> {
    state
        .workspace
        .inspector
        .as_ref()
        .and_then(|i| match &i.record.resource {
            Resource::Project(key) => Some(key.clone()),
            _ => i.record.project.clone(),
        })
}
fn record(state: &AppState, resource: Resource, name: String) -> ResourceRecord {
    state
        .workspace
        .projects
        .iter()
        .flat_map(|p| p.resources.iter())
        .find(|r| r.resource == resource)
        .cloned()
        .unwrap_or_else(|| ResourceRecord::simple(resource, name))
}

pub fn inspect_selected(
    state: &mut AppState,
    containers: &[ContainerInfo],
    ports: &[PortInfo],
    pm2: &[Pm2Process],
    pm2_rows: &[usize],
) {
    let selected = match state.view_mode {
        ViewMode::Projects => selected_project(state)
            .and_then(|key| state.workspace.project(&key).map(|p| p.record())),
        ViewMode::Docker => state
            .docker_rows
            .get(state.docker_selected_row)
            .and_then(|row| match row {
                crate::system::docker::DockerRow::Item { index, .. } => {
                    state.visible_containers.get(*index).map(|id| (*index, id))
                }
                _ => None,
            })
            .filter(|(_, id)| !id.is_empty())
            .map(|(index, id)| {
                let mut record = record(
                    state,
                    Resource::Container(id.clone()),
                    state
                        .visible_container_names
                        .get(index)
                        .cloned()
                        .unwrap_or_else(|| id.clone()),
                );
                if let Some(container) = containers.iter().find(|c| &c.id == id) {
                    record.path = container.group_path.clone();
                    record.details = vec![
                        format!("ID: {}", container.id),
                        format!("Image: {}", container.image),
                        format!("Status: {}", container.status),
                        format!("Published ports: {}", container.port_public),
                    ];
                    record.command = Some(format!("docker logs --follow {}", container.id));
                }
                record
            }),
        ViewMode::Node if state.node_tab == NodeTab::Pm2 => pm2_rows
            .get(state.pm2_selected)
            .and_then(|index| pm2.get(*index))
            .map(|p| {
                let mut record = record(
                    state,
                    Resource::Pm2 {
                        id: p.pm_id,
                        pid: p.pid,
                    },
                    p.name.clone(),
                );
                record.path = p.cwd.clone();
                record.command = p.script.clone();
                record.cpu = p.cpu;
                record.memory = p.memory_bytes;
                record.estimated = true;
                record.details = vec![
                    format!(
                        "PM2 ID: {} · PID: {}",
                        p.pm_id,
                        p.pid.map_or("-".into(), |pid| pid.to_string())
                    ),
                    format!("Status: {} · Mode: {}", p.status, p.mode),
                ];
                record
            }),
        ViewMode::Process | ViewMode::Node => state
            .visible_pids
            .get(state.selected)
            .filter(|pid| pid.as_u32() != 0)
            .and_then(|pid| {
                state
                    .process_identities
                    .get(&pid.as_u32())
                    .copied()
                    .or_else(|| crate::system::process::process_identity(pid.as_u32()).ok())
                    .map(|identity| {
                        let mut r = record(
                            state,
                            Resource::Process {
                                pid: pid.as_u32(),
                                identity,
                            },
                            format!("PID {pid}"),
                        );
                        if r.details.is_empty() {
                            r.details =
                                vec![format!("PID: {pid}"), format!("Start identity: {identity}")];
                        }
                        r
                    })
            }),
        ViewMode::Ports => state
            .visible_port_indices
            .get(state.selected)
            .and_then(|index| *index)
            .and_then(|index| ports.get(index))
            .map(|p| {
                let mut r = record(
                    state,
                    Resource::Port {
                        protocol: p.proto.clone(),
                        port: p.port,
                        pid: p.pid.as_u32(),
                        container: p.container_id.clone(),
                    },
                    format!("{} {} · {}", p.proto, p.binding_display(), p.name),
                );
                r.details = vec![
                    format!("Owner: {} · PID: {}", p.name, p.pid),
                    format!("Container: {}", p.container_id.as_deref().unwrap_or("-")),
                ];
                r.command = Some(p.exe_path.clone());
                r
            }),
        _ => None,
    };
    if let Some(record) = selected {
        state.workspace.inspect(record);
    } else {
        state.set_message("Select a resource row to inspect it.");
    }
}
fn jump(state: &mut AppState, resource: Resource) {
    let resource = match resource {
        Resource::Port {
            container: Some(id),
            ..
        } => Resource::Container(id),
        Resource::Port {
            protocol,
            port,
            pid,
            ..
        } if pid != 0 => {
            if state.workspace.owner_request.is_some() {
                state.set_message("A listener owner lookup is already running.");
                return;
            }
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(
                    crate::system::ports::current_native_owner(&protocol, port)
                        .map(|(pid, identity)| Resource::Process { pid, identity }),
                );
            });
            state.workspace.owner_request = Some(rx);
            state.set_message("Resolving the current listener owner… Esc cancels navigation.");
            return;
        }
        Resource::Port { .. } => {
            state.set_message("Port ownership is unavailable. Refresh Ports before navigating.");
            return;
        }
        resource => resource,
    };
    match &resource {
        Resource::Container(_) => {
            state.set_view(ViewMode::Docker);
            state.docker_filter.clear();
            state.docker_volume_scope = None;
            state.docker_refresh_requested = true;
        }
        Resource::Process { .. } => {
            state.set_view(ViewMode::Process);
            state.process_filter.clear();
            // A listener can belong to a child hidden in the collapsed tree.
            state.zoom = true;
            state.refresh_requested = true;
        }
        Resource::Pm2 { .. } => {
            state.set_view(ViewMode::Node);
            state.set_node_tab(NodeTab::Pm2);
            state.node_filter.clear();
            state.refresh_requested = true;
        }
        _ => {
            state.set_message("Inspect this resource in the project details.");
            return;
        }
    }
    state.workspace.inspector = None;
    state.workspace.navigation = Some(resource);
    state.workspace.navigation_started = Some(std::time::Instant::now());
    state.focus = Focus::Main;
}
pub fn complete_navigation(state: &mut AppState, pm2: &[Pm2Process], pm2_rows: &[usize]) {
    if let Some(result) = state
        .workspace
        .owner_request
        .as_ref()
        .and_then(|rx| rx.try_recv().ok())
    {
        state.workspace.owner_request = None;
        match result {
            Ok(resource) => jump(state, resource),
            Err(error) => state.set_message(format!("Owner navigation failed: {error}")),
        }
    }
    let Some(resource) = &state.workspace.navigation else {
        return;
    };
    let index = match resource {
        Resource::Container(id) => {
            let rows: Vec<_> = state
                .visible_containers
                .iter()
                .enumerate()
                .filter(|(_, full)| *full == id || (id.len() >= 12 && full.starts_with(id)))
                .map(|(index, _)| index)
                .collect();
            if rows.len() == 1 {
                Some(rows[0])
            } else {
                None
            }
        }
        Resource::Process { pid, identity } => state.visible_pids.iter().position(|p| {
            p.as_u32() == *pid
                && (*identity == 0 || state.process_identities.get(pid) == Some(identity))
        }),
        Resource::Pm2 { id, .. } => pm2_rows
            .iter()
            .position(|index| pm2.get(*index).is_some_and(|p| p.pm_id == *id)),
        _ => None,
    };
    if let Some(index) = index {
        if matches!(resource, Resource::Pm2 { .. }) {
            state.pm2_selected = index;
        } else {
            state.selected = index;
        }
        if state.view_mode == ViewMode::Docker {
            if let Some(row) = state.docker_rows.iter().position(
                |row| matches!(row,crate::system::docker::DockerRow::Item{index:i,..} if *i==index),
            ) {
                state.docker_selected_row = row;
            }
        }
        state.workspace.navigation = None;
        state.workspace.navigation_started = None;
        state.set_message("Selected resource owner. F2 opens its inspector.");
    } else if state
        .workspace
        .navigation_started
        .is_some_and(|started| started.elapsed() >= std::time::Duration::from_secs(15))
    {
        state.workspace.navigation = None;
        state.workspace.navigation_started = None;
        state.set_message("Owner is absent from the current snapshot. It may have exited; F5 refreshes this view.");
    }
}
fn select_tab(state: &mut AppState, tab: InspectorTab) {
    if let Some(i) = &mut state.workspace.inspector {
        i.tab = tab;
        i.scroll = 0;
    }
    if tab == InspectorTab::Logs {
        state.workspace.open_logs();
    }
    if tab == InspectorTab::Storage && !state.workspace.storage_loaded {
        state.workspace.load_storage();
    }
    if tab == InspectorTab::Events {
        state.workspace.start_events();
    }
}
fn save(state: &mut AppState) -> Result<(), String> {
    if let Some(error) = &state.workspace.config_error {
        return Err(format!(
            "Existing configuration was not loaded: {error}. Fix the file before saving."
        ));
    }
    let path = state
        .workspace
        .config_path
        .clone()
        .unwrap_or_else(workspace_config::config_path);
    workspace_config::save(&path, &workspace_config::encode(state)).map_err(|e| e.to_string())
}
fn editor_key(key: KeyEvent, state: &mut AppState) {
    let Some(mut editor) = state.workspace.editor.take() else {
        return;
    };
    if key.code == KeyCode::Esc {
        return;
    }
    match editor.kind {
        EditorKind::Project => {
            if key.code == KeyCode::Tab {
                editor.field = (editor.field + 1) % 6;
            } else if key.code == KeyCode::BackTab {
                editor.field = (editor.field + 5) % 6;
            } else if key.code == KeyCode::Char('s')
                && key.modifiers.contains(KeyModifiers::CONTROL)
            {
                if editor.config.name.trim().is_empty()
                    || !std::path::Path::new(&editor.config.path).is_absolute()
                {
                    editor.error = Some("Enter a name and an absolute project directory".into());
                } else {
                    let previous = state.workspace.preferences.projects.clone();
                    if let Some(existing) = state
                        .workspace
                        .preferences
                        .projects
                        .iter_mut()
                        .find(|p| p.path == editor.text || p.path == editor.config.path)
                    {
                        *existing = editor.config.clone();
                    } else {
                        state
                            .workspace
                            .preferences
                            .projects
                            .push(editor.config.clone());
                    }
                    match save(state) {
                        Ok(()) => {
                            state.workspace.catalog_version += 1;
                            state.set_message(
                                "Project configuration saved. D starts dev; P starts prod.",
                            );
                            return;
                        }
                        Err(error) => {
                            state.workspace.preferences.projects = previous;
                            editor.error = Some(error);
                        }
                    }
                }
            } else if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
                match key.code {
                    KeyCode::Char(ch) => {
                        let field = workspace_config::field_mut(&mut editor.config, editor.field);
                        if field.len() < 4096 {
                            field.push(ch);
                        }
                    }
                    KeyCode::Backspace => {
                        workspace_config::field_mut(&mut editor.config, editor.field).pop();
                    }
                    _ => {}
                }
            }
        }
        EditorKind::SaveFilter => match key.code {
            KeyCode::Enter => {
                if editor.text.trim().is_empty() {
                    editor.error = Some("Enter a filter name".into());
                } else {
                    let filter = SavedFilter {
                        name: editor.text.trim().into(),
                        view: state.view_mode,
                        pm2: state.view_mode == ViewMode::Node && state.node_tab == NodeTab::Pm2,
                        filter: state.active_filter().into(),
                    };
                    let previous = state.workspace.preferences.filters.clone();
                    state
                        .workspace
                        .preferences
                        .filters
                        .retain(|f| f.name != filter.name);
                    state.workspace.preferences.filters.push(filter);
                    match save(state) {
                        Ok(()) => {
                            state.set_message("Named filter saved. W opens saved filters.");
                            return;
                        }
                        Err(error) => {
                            state.workspace.preferences.filters = previous;
                            editor.error = Some(error);
                        }
                    }
                }
            }
            KeyCode::Backspace => {
                editor.text.pop();
            }
            KeyCode::Char(ch)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                if editor.text.len() < 128 {
                    editor.text.push(ch);
                }
            }
            _ => {}
        },
        EditorKind::Filters => match key.code {
            KeyCode::Up => editor.field = editor.field.saturating_sub(1),
            KeyCode::Down => {
                editor.field = (editor.field + 1)
                    .min(state.workspace.preferences.filters.len().saturating_sub(1))
            }
            KeyCode::Char('n') => {
                editor.kind = EditorKind::SaveFilter;
                editor.text.clear();
            }
            KeyCode::Enter => {
                if let Some(filter) = state
                    .workspace
                    .preferences
                    .filters
                    .get(editor.field)
                    .cloned()
                {
                    state.set_view(filter.view);
                    if filter.pm2 {
                        state.set_node_tab(NodeTab::Pm2);
                    }
                    *state.active_filter_mut() = filter.filter;
                    state.workspace.inspector = None;
                    state.focus = Focus::Main;
                    return;
                }
            }
            KeyCode::Delete => {
                if editor.field < state.workspace.preferences.filters.len() {
                    let previous = state.workspace.preferences.filters.clone();
                    state.workspace.preferences.filters.remove(editor.field);
                    if let Err(error) = save(state) {
                        state.workspace.preferences.filters = previous;
                        editor.error = Some(error);
                    }
                    editor.field = editor
                        .field
                        .min(state.workspace.preferences.filters.len().saturating_sub(1));
                }
            }
            _ => {}
        },
    }
    state.workspace.editor = Some(editor);
}
pub fn key(
    key: KeyEvent,
    state: &mut AppState,
    containers: &[ContainerInfo],
    ports: &[PortInfo],
    pm2: &[Pm2Process],
    pm2_rows: &[usize],
) -> Option<bool> {
    if key.code == KeyCode::Esc && state.workspace.owner_request.is_some() {
        state.workspace.owner_request = None;
        state.set_message("Owner navigation canceled.");
        return Some(false);
    }
    if state.workspace.editor.is_some() {
        editor_key(key, state);
        return Some(false);
    }
    if state.workspace.cleanup_open && !matches!(state.workspace.cleanup, Cleanup::Idle) {
        match key.code {
            KeyCode::Char('y') if matches!(state.workspace.cleanup, Cleanup::Review(_)) => {
                state.workspace.execute_cleanup()
            }
            KeyCode::Esc | KeyCode::Char('n') => {
                if matches!(state.workspace.cleanup, Cleanup::Loading(_)) {
                    state.workspace.review_cancelled = true;
                }
                if !state.workspace.cleanup_busy() {
                    state.workspace.cleanup = Cleanup::Idle;
                }
                state.workspace.cleanup_open = false;
            }
            KeyCode::Up => {
                state.workspace.review_scroll = state.workspace.review_scroll.saturating_sub(1)
            }
            KeyCode::Down => {
                state.workspace.review_scroll = state.workspace.review_scroll.saturating_add(1)
            }
            KeyCode::PageDown => {
                state.workspace.review_scroll = state.workspace.review_scroll.saturating_add(10)
            }
            KeyCode::PageUp => {
                state.workspace.review_scroll = state.workspace.review_scroll.saturating_sub(10)
            }
            _ => {}
        }
        return Some(false);
    }
    if key.code == KeyCode::F(6) && !matches!(state.workspace.cleanup, Cleanup::Idle) {
        state.workspace.cleanup_open = true;
        return Some(false);
    }
    if state.input_mode != InputMode::Normal {
        return None;
    }
    let viewport = crate::ui::workspace::panes(
        crate::ui::layout::main_area(Rect::new(0, 0, state.term_width, state.term_height)),
        true,
    )
    .1
    .map(crate::ui::workspace::inspector_body)
    .map_or(10, |r| r.height as usize);
    if let Some(inspector) = &mut state.workspace.inspector {
        if inspector.searching {
            match key.code {
                KeyCode::Esc | KeyCode::Enter => inspector.searching = false,
                KeyCode::Backspace => {
                    inspector.query.pop();
                }
                KeyCode::Char(ch)
                    if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
                {
                    if inspector.query.len() < 256 {
                        inspector.query.push(ch);
                    }
                }
                _ => {}
            }
            return Some(false);
        }
        let tab = inspector.tab;
        match key.code {
            KeyCode::Esc | KeyCode::F(2) => state.workspace.inspector = None,
            KeyCode::Tab | KeyCode::BackTab => {
                let index = InspectorTab::ALL.iter().position(|t| *t == tab).unwrap();
                select_tab(
                    state,
                    InspectorTab::ALL[(index + if key.code == KeyCode::Tab { 1 } else { 5 }) % 6],
                );
            }
            KeyCode::Char('l') => select_tab(state, InspectorTab::Logs),
            KeyCode::Char('e') => select_tab(state, InspectorTab::Events),
            KeyCode::Char('t') => select_tab(state, InspectorTab::Trends),
            KeyCode::Char('v') => select_tab(state, InspectorTab::Storage),
            KeyCode::Char('/') if tab == InspectorTab::Logs => inspector.searching = true,
            KeyCode::Char('p') if tab == InspectorTab::Logs => {
                if inspector.frozen.is_some() {
                    inspector.frozen = None;
                    inspector.follow = true;
                } else {
                    inspector.frozen = Some(inspector.logs.iter().cloned().collect());
                    inspector.scroll = inspector.visible_logs().len().saturating_sub(viewport);
                    inspector.follow = false;
                }
            }
            KeyCode::End => {
                inspector.follow = true;
                inspector.frozen = None;
                inspector.scroll = usize::MAX;
            }
            KeyCode::Home => {
                inspector.follow = false;
                inspector.scroll = 0;
                inspector.resource_selected = 0;
                state.workspace.storage_selected = 0;
            }
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown => {
                let down = matches!(key.code, KeyCode::Down | KeyCode::PageDown);
                let amount = if matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
                    10
                } else {
                    1
                };
                if tab == InspectorTab::Details
                    && matches!(inspector.record.resource, Resource::Project(_))
                {
                    let count = inspector
                        .record
                        .project
                        .as_ref()
                        .and_then(|key| state.workspace.projects.iter().find(|p| &p.key == key))
                        .map_or(0, |p| p.resources.len());
                    inspector.resource_selected = if down {
                        inspector
                            .resource_selected
                            .saturating_add(amount)
                            .min(count.saturating_sub(1))
                    } else {
                        inspector.resource_selected.saturating_sub(amount)
                    };
                } else if tab == InspectorTab::Storage {
                    let selected = state.workspace.storage_selected;
                    let _ = inspector;
                    let count = state.workspace.storage_items().len();
                    state.workspace.storage_selected = if down {
                        selected.saturating_add(amount).min(count.saturating_sub(1))
                    } else {
                        selected.saturating_sub(amount)
                    };
                } else {
                    if inspector.follow && matches!(tab, InspectorTab::Logs | InspectorTab::Run) {
                        let count = if tab == InspectorTab::Run {
                            let key = match &inspector.record.resource {
                                Resource::Project(key) => Some(key),
                                _ => inspector.record.project.as_ref(),
                            };
                            key.and_then(|key| {
                                state.workspace.runs.iter().find(|run| &run.key == key)
                            })
                            .map_or(0, |run| run.lines.len() + 4)
                        } else {
                            inspector.visible_logs().len()
                        };
                        inspector.scroll = count.saturating_sub(viewport);
                    }
                    inspector.follow = false;
                    inspector.scroll = if down {
                        inspector.scroll.saturating_add(amount)
                    } else {
                        inspector.scroll.saturating_sub(amount)
                    };
                }
            }
            KeyCode::Char(' ') if tab == InspectorTab::Storage => {
                let name = state
                    .workspace
                    .storage_items()
                    .get(state.workspace.storage_selected)
                    .map(|item| item.name.clone());
                if let Some(name) = name {
                    if !state.workspace.selected_volumes.remove(&name) {
                        state.workspace.selected_volumes.insert(name);
                    }
                }
            }
            KeyCode::Delete if tab == InspectorTab::Storage => {
                if state.delete_in_progress.is_some()
                    || state.prune_in_progress.is_some()
                    || !state.pending_operations.is_empty()
                {
                    state.set_message(
                        "Wait for the existing resource action before reviewing cleanup.",
                    );
                } else {
                    state.workspace.review_cleanup();
                }
            }
            KeyCode::Enter if tab == InspectorTab::Details => {
                let previous = inspector.record.clone();
                let selected = inspector.resource_selected;
                let next = match &previous.resource {
                    Resource::Project(key) => state
                        .workspace
                        .project(key)
                        .and_then(|p| p.resources.get(selected))
                        .cloned(),
                    Resource::Port { .. } => {
                        jump(state, previous.resource);
                        return Some(false);
                    }
                    _ => None,
                };
                if let Some(next) = next {
                    state.workspace.inspect(next);
                    state.workspace.inspector.as_mut().unwrap().back = Some(previous);
                }
            }
            KeyCode::Enter if tab == InspectorTab::Storage => {
                if let Some(item) = state
                    .workspace
                    .storage_items()
                    .get(state.workspace.storage_selected)
                {
                    let mut next = ResourceRecord::simple(
                        Resource::Volume(item.name.clone()),
                        item.name.clone(),
                    );
                    next.details = vec![
                        format!("Size: {}", item.size),
                        format!(
                            "Activity: {}",
                            item.activity.as_deref().unwrap_or("Unknown")
                        ),
                        format!("Containers: {}", item.detail_left),
                        format!("Images: {}", item.detail_right),
                    ];
                    let back = state.workspace.inspector.as_ref().map(|i| i.record.clone());
                    state.workspace.inspect(next);
                    state.workspace.inspector.as_mut().unwrap().back = back;
                }
            }
            KeyCode::Backspace => {
                if let Some(back) = inspector.back.clone() {
                    state.workspace.inspect(back);
                }
            }
            KeyCode::Char('o') => {
                let target = if let Resource::Project(key) = &inspector.record.resource {
                    state
                        .workspace
                        .projects
                        .iter()
                        .find(|p| &p.key == key)
                        .and_then(|p| p.resources.get(inspector.resource_selected))
                        .map(|r| r.resource.clone())
                } else {
                    Some(inspector.record.resource.clone())
                };
                if let Some(target) = target {
                    jump(state, target);
                }
            }
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let copy = if key.code == KeyCode::Char('Y') {
                    inspector.record.command.clone()
                } else if tab == InspectorTab::Logs {
                    let logs = inspector.visible_logs();
                    let start = if inspector.follow && inspector.frozen.is_none() {
                        logs.len().saturating_sub(viewport)
                    } else {
                        inspector.scroll.min(logs.len().saturating_sub(viewport))
                    };
                    Some(
                        logs.iter()
                            .skip(start)
                            .take(viewport)
                            .map(|line| line.text.as_str())
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                } else {
                    inspector.record.path.clone()
                };
                if copy.as_ref().is_some_and(|value| !value.is_empty()) {
                    state.workspace.clipboard = copy;
                    state
                        .set_message("Copy requested through terminal clipboard support (OSC 52).");
                } else {
                    state.set_message(
                        "No path, command, or visible log lines are available to copy.",
                    );
                }
            }
            KeyCode::Char('C') => {
                let key = inspector_project(state);
                state.workspace.configure(key.as_deref());
            }
            KeyCode::Char('D') | KeyCode::Char('P') => {
                if let Some(project) = inspector_project(state) {
                    let result = state.workspace.launch(
                        &project,
                        if key.code == KeyCode::Char('D') {
                            Mode::Dev
                        } else {
                            Mode::Prod
                        },
                    );
                    if let Err(error) = result {
                        state.set_message(error);
                    }
                    select_tab(state, InspectorTab::Run);
                }
            }
            KeyCode::Char('X') if tab == InspectorTab::Run => {
                if let Some(key) = inspector_project(state) {
                    if let Some(job) = state
                        .workspace
                        .runs
                        .iter()
                        .find(|r| r.key == key)
                        .and_then(|r| r.job.as_ref())
                    {
                        job.cancel();
                    }
                }
            }
            KeyCode::F(5) => {
                if tab == InspectorTab::Events {
                    state.workspace.event_stream = None;
                    state.workspace.events_started = false;
                    state.workspace.start_events();
                } else if tab == InspectorTab::Logs {
                    inspector.stream = None;
                    state.workspace.open_logs();
                } else {
                    state.workspace.load_storage();
                    state.refresh_requested = true;
                    state.docker_refresh_requested = true;
                }
            }
            KeyCode::Char('1')
            | KeyCode::Char('2')
            | KeyCode::Char('3')
            | KeyCode::Char('4')
            | KeyCode::Char('5') => {
                state.workspace.inspector = None;
                return None;
            }
            _ => {}
        }
        return Some(false);
    }
    if key.code == KeyCode::Char('l') && !key.modifiers.contains(KeyModifiers::CONTROL) {
        inspect_selected(state, containers, ports, pm2, pm2_rows);
        state.workspace.open_logs();
        return Some(false);
    }
    if key.code == KeyCode::F(2) {
        inspect_selected(state, containers, ports, pm2, pm2_rows);
        return Some(false);
    }
    if key.code == KeyCode::Char('W') {
        state.workspace.editor = Some(Editor {
            kind: EditorKind::Filters,
            config: Default::default(),
            field: 0,
            text: String::new(),
            error: None,
        });
        return Some(false);
    }
    if state.view_mode == ViewMode::Ports && key.code == KeyCode::Enter {
        inspect_selected(state, containers, ports, pm2, pm2_rows);
        if let Some(record) = state.workspace.inspector.take().map(|i| i.record) {
            jump(state, record.resource);
        }
        return Some(false);
    }
    if key.code == KeyCode::Char('5') {
        state.set_view(ViewMode::Projects);
        state.focus = Focus::Main;
        return Some(false);
    }
    if state.view_mode != ViewMode::Projects || state.focus == Focus::Sidebar {
        return None;
    }
    let count = state
        .workspace
        .visible_projects(state.sort_for(SortTarget::Projects))
        .len();
    match key.code {
        KeyCode::Enter => {
            inspect_selected(state, containers, ports, pm2, pm2_rows);
        }
        KeyCode::Up => state.workspace.selected = state.workspace.selected.saturating_sub(1),
        KeyCode::Down => {
            state.workspace.selected = (state.workspace.selected + 1).min(count.saturating_sub(1))
        }
        KeyCode::Home => state.workspace.selected = 0,
        KeyCode::End => state.workspace.selected = count.saturating_sub(1),
        KeyCode::PageDown => {
            state.workspace.selected = (state.workspace.selected + 10).min(count.saturating_sub(1))
        }
        KeyCode::PageUp => state.workspace.selected = state.workspace.selected.saturating_sub(10),
        KeyCode::Char('f') => {
            if let Some(key) = selected_project(state) {
                if !state.workspace.preferences.favorites.remove(&key) {
                    state.workspace.preferences.favorites.insert(key.clone());
                }
                if let Some(index) = state
                    .workspace
                    .visible_projects(state.sort_for(SortTarget::Projects))
                    .iter()
                    .position(|p| p.key == key)
                {
                    state.workspace.selected = index;
                }
                if let Err(error) = save(state) {
                    state.set_message(error);
                }
            }
        }
        KeyCode::Char('C') => {
            let key = selected_project(state);
            state.workspace.configure(key.as_deref());
        }
        KeyCode::Char('D') | KeyCode::Char('P') => {
            if let Some(project) = selected_project(state) {
                let record = state.workspace.project(&project).unwrap().record();
                state.workspace.inspect(record);
                if let Err(error) = state.workspace.launch(
                    &project,
                    if key.code == KeyCode::Char('D') {
                        Mode::Dev
                    } else {
                        Mode::Prod
                    },
                ) {
                    state.set_message(error);
                }
                select_tab(state, InspectorTab::Run);
            }
        }
        KeyCode::F(5) => {
            state.refresh_requested = true;
            state.docker_refresh_requested = true;
            if state.workspace.storage_loaded {
                state.workspace.load_storage();
            }
            state.workspace.catalog_version += 1;
        }
        _ => return None,
    }
    Some(false)
}

/// The inspector owns its pane; table input uses the matching split geometry.
pub fn mouse(mouse: MouseEvent, state: &mut AppState, full: Rect) -> Option<bool> {
    if state.workspace.editor.is_some()
        || state.workspace.cleanup_open && !matches!(state.workspace.cleanup, Cleanup::Idle)
    {
        return Some(true);
    }
    let (table, pane) = crate::ui::workspace::panes(full, state.workspace.inspector.is_some());
    if let Some(pane) = pane.filter(|pane| pane.contains((mouse.column, mouse.row).into())) {
        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
            if let Some((_, tab)) = crate::ui::workspace::tab_hits(pane)
                .into_iter()
                .find(|(area, _)| area.contains((mouse.column, mouse.row).into()))
            {
                select_tab(state, tab);
                return Some(true);
            }
            let body = crate::ui::workspace::inspector_body(pane);
            let Some(inspector) = &mut state.workspace.inspector else {
                return Some(true);
            };
            if inspector.tab == InspectorTab::Details
                && matches!(inspector.record.resource, Resource::Project(_))
            {
                let rows = crate::ui::workspace::project_resources(body);
                if mouse.row > rows.y && mouse.row < rows.bottom() {
                    let visible = rows.height.saturating_sub(1) as usize;
                    let start = inspector
                        .resource_selected
                        .saturating_sub(visible.saturating_sub(1));
                    inspector.resource_selected = start + (mouse.row - rows.y - 1) as usize;
                }
            } else if inspector.tab == InspectorTab::Storage {
                let rows = crate::ui::workspace::storage_table(body);
                if mouse.row > rows.y && mouse.row < rows.bottom() {
                    let visible = rows.height.saturating_sub(1) as usize;
                    let start = state
                        .workspace
                        .storage_selected
                        .saturating_sub(visible.saturating_sub(1));
                    state.workspace.storage_selected = start + (mouse.row - rows.y - 1) as usize;
                }
            }
        } else if matches!(
            mouse.kind,
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
        ) {
            let code = if mouse.kind == MouseEventKind::ScrollUp {
                KeyCode::Up
            } else {
                KeyCode::Down
            };
            let _ = key(
                KeyEvent::new(code, KeyModifiers::NONE),
                state,
                &[],
                &[],
                &[],
                &[],
            );
        }
        return Some(true);
    }
    if state.view_mode == ViewMode::Projects && table.contains((mouse.column, mouse.row).into()) {
        let header = crate::ui::layout::collection_header(table, ViewMode::Projects);
        if mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && header.search.contains((mouse.column, mouse.row).into())
        {
            state.input_mode = InputMode::Filter;
            return Some(true);
        }
        let rows = crate::ui::workspace::project_layout(table)[1];
        if mouse.row < rows.y + 2 || mouse.row >= rows.bottom().saturating_sub(1) {
            return None;
        }
        let count = state
            .workspace
            .visible_projects(state.sort_for(SortTarget::Projects))
            .len();
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                state.workspace.selected = (state.workspace.scroll
                    + (mouse.row - rows.y - 2) as usize)
                    .min(count.saturating_sub(1));
                state.focus = Focus::Main;
            }
            MouseEventKind::ScrollUp => {
                state.workspace.selected = state.workspace.selected.saturating_sub(1)
            }
            MouseEventKind::ScrollDown => {
                state.workspace.selected =
                    (state.workspace.selected + 1).min(count.saturating_sub(1))
            }
            _ => return Some(false),
        }
        return Some(true);
    }
    None
}
