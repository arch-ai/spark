use super::projects::{Project, Resource, ResourceRecord};
use super::sorting::{SortField, SortTarget, TableSort};
use super::workspace::{Cleanup, InspectorTab};
use super::*;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn path() -> std::path::PathBuf {
    std::env::temp_dir()
        .join(format!(
            "spark-workspace-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
        .join("workspace.json")
}
fn press(state: &mut AppState, code: KeyCode) {
    assert_eq!(
        super::workspace_input::key(
            KeyEvent::new(code, KeyModifiers::NONE),
            state,
            &[],
            &[],
            &[],
            &[]
        ),
        Some(false)
    );
}
fn project(name: &str, path: &str) -> Project {
    Project {
        key: projects::project_key(path),
        name: name.into(),
        path: Some(path.into()),
        resources: vec![],
        cpu: 2.0,
        memory: 50,
        estimated: true,
        running_containers: 0,
        unmeasured: 0,
    }
}

#[test]
fn persisted_workspace_round_trips_sorts_filters_favorites_and_explicit_scripts() {
    let path = path();
    let mut state = AppState::new();
    state.set_view(ViewMode::Projects);
    state.workspace.filter = "api".into();
    state.docker_filter = "postgres".into();
    state
        .workspace
        .preferences
        .favorites
        .insert("path:/srv/api".into());
    state
        .workspace
        .preferences
        .projects
        .push(workspace_config::ProjectConfig {
            name: "api".into(),
            path: "/srv/api".into(),
            start_dev: "./start-dev".into(),
            stop_prod: "./stop-prod".into(),
            ..Default::default()
        });
    state
        .workspace
        .preferences
        .filters
        .push(workspace_config::SavedFilter {
            name: "db".into(),
            view: ViewMode::Docker,
            pm2: false,
            filter: "postgres".into(),
        });
    state.apply_sort(
        SortTarget::Projects,
        TableSort::new(SortField::Memory, SortOrder::Desc),
    );
    workspace_config::save(&path, &workspace_config::encode(&state)).unwrap();
    let mut restored = AppState::new();
    workspace_config::load(&mut restored, &path).unwrap();
    assert_eq!(restored.view_mode, ViewMode::Projects);
    assert_eq!(restored.workspace.filter, "api");
    assert_eq!(restored.docker_filter, "postgres");
    assert_eq!(
        restored.sort_for(SortTarget::Projects).field,
        SortField::Memory
    );
    assert_eq!(
        restored.workspace.preferences.projects[0].stop_prod,
        "./stop-prod"
    );
    assert!(restored
        .workspace
        .preferences
        .favorites
        .contains("path:/srv/api"));
    assert_eq!(restored.workspace.preferences.filters[0].name, "db");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}
#[test]
fn invalid_configuration_is_not_silently_overwritten() {
    let path = path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"{broken").unwrap();
    let mut state = AppState::new();
    assert!(workspace_config::load(&mut state, &path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"{broken");
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}
#[test]
fn inspector_pause_search_and_close_preserve_the_underlying_table() {
    let mut state = AppState::new();
    state.selected = 4;
    state.process_scroll = 2;
    state.workspace.inspect(ResourceRecord::simple(
        Resource::Volume("data".into()),
        "data".into(),
    ));
    let i = state.workspace.inspector.as_mut().unwrap();
    i.tab = InspectorTab::Logs;
    i.push("normal".into(), false);
    i.push("ERROR failed request".into(), false);
    press(&mut state, KeyCode::Char('p'));
    state
        .workspace
        .inspector
        .as_mut()
        .unwrap()
        .push("new output".into(), false);
    assert_eq!(
        state
            .workspace
            .inspector
            .as_ref()
            .unwrap()
            .visible_logs()
            .len(),
        2
    );
    press(&mut state, KeyCode::Char('/'));
    for ch in "error".chars() {
        press(&mut state, KeyCode::Char(ch));
    }
    press(&mut state, KeyCode::Enter);
    assert_eq!(
        state
            .workspace
            .inspector
            .as_ref()
            .unwrap()
            .visible_logs()
            .len(),
        1
    );
    assert!(state.workspace.inspector.as_ref().unwrap().visible_logs()[0].error);
    press(&mut state, KeyCode::Esc);
    assert!(state.workspace.inspector.is_none());
    assert_eq!(state.selected, 4);
    assert_eq!(state.process_scroll, 2);
}
#[test]
fn favorites_keep_identity_when_they_move_to_the_top() {
    let mut state = AppState::new();
    state.view_mode = ViewMode::Projects;
    state.workspace.config_path = Some(path());
    state.workspace.projects = vec![project("a", "/srv/a"), project("b", "/srv/b")];
    state.workspace.selected = 1;
    press(&mut state, KeyCode::Char('f'));
    assert_eq!(
        state
            .workspace
            .visible_projects(state.sort_for(SortTarget::Projects))[state.workspace.selected]
            .name,
        "b"
    );
    std::fs::remove_dir_all(
        state
            .workspace
            .config_path
            .as_ref()
            .unwrap()
            .parent()
            .unwrap(),
    )
    .unwrap();
}
#[test]
fn storage_selection_is_scoped_to_the_inspected_project_and_unknown_sizes_stay_unknown() {
    let mut state = AppState::new();
    let mut p = project("api", "/srv/api");
    let mut r = ResourceRecord::simple(Resource::Volume("data".into()), "data".into());
    r.project = Some(p.key.clone());
    p.resources.push(r);
    let record = p.record();
    state.workspace.projects.push(p);
    state.workspace.inspect(record);
    state.workspace.inspector.as_mut().unwrap().tab = InspectorTab::Storage;
    state.workspace.volumes = std::sync::Arc::new(vec![
        crate::system::docker::DockerListItem {
            name: "data".into(),
            size: "Unknown".into(),
            ..Default::default()
        },
        crate::system::docker::DockerListItem {
            name: "other-project".into(),
            size: "950 GB".into(),
            ..Default::default()
        },
    ]);
    assert_eq!(state.workspace.storage_items().len(), 1);
    press(&mut state, KeyCode::Char(' '));
    assert!(state.workspace.selected_volumes.contains("data"));
    assert!(!state.workspace.selected_volumes.contains("other-project"));
    assert_eq!(state.workspace.storage_items()[0].size, "Unknown");
    assert!(matches!(state.workspace.cleanup, Cleanup::Idle));
}
#[test]
fn docker_inspector_uses_the_selected_row_after_live_sorting_reorders_containers() {
    use crate::system::docker::DockerRow;
    let mut state = AppState::new();
    state.set_view(ViewMode::Docker);
    state.visible_containers = vec!["alpha".into(), "beta".into()];
    state.visible_container_names = vec!["Alpha".into(), "Beta".into()];
    state.docker_rows = vec![
        DockerRow::Group {
            name: "demo".into(),
            path: None,
            count: 2,
            running_count: 2,
        },
        DockerRow::Item {
            index: 0,
            prefix: String::new(),
        },
        DockerRow::Item {
            index: 1,
            prefix: String::new(),
        },
    ];
    state.docker_selected_row = 2;
    state.selected = 0; // This index belongs to other tables and can be stale.
    press(&mut state, KeyCode::F(2));
    let inspector = state.workspace.inspector.as_ref().unwrap();
    assert_eq!(
        inspector.record.resource,
        Resource::Container("beta".into())
    );
    assert_eq!(inspector.record.name, "Beta");
    press(&mut state, KeyCode::Esc);
    state.docker_selected_row = 0;
    press(&mut state, KeyCode::F(2));
    assert!(
        state.workspace.inspector.is_none(),
        "A project header is not a container"
    );
}

#[test]
fn owner_navigation_keeps_exact_identity_and_does_not_select_a_reused_pid() {
    let mut state = AppState::new();
    state.visible_pids = vec![sysinfo::Pid::from_u32(100)];
    state.process_identities.insert(100, 9);
    state.workspace.navigation = Some(Resource::Process {
        pid: 100,
        identity: 1,
    });
    super::workspace_input::complete_navigation(&mut state, &[], &[]);
    assert!(state.workspace.navigation.is_some());
    state.process_identities.insert(100, 1);
    super::workspace_input::complete_navigation(&mut state, &[], &[]);
    assert!(state.workspace.navigation.is_none());
    assert_eq!(state.selected, 0);
    assert_eq!(
        workspace::clipboard_escape("hello"),
        "\x1b]52;c;aGVsbG8=\x07"
    );
}

#[test]
fn resolved_native_port_owner_expands_the_tree_before_selecting_a_child() {
    let mut state = AppState::new();
    state.set_view(ViewMode::Ports);
    state.zoom = false;
    let (tx, rx) = std::sync::mpsc::channel();
    state.workspace.owner_request = Some(rx);
    tx.send(Ok(Resource::Process {
        pid: 100,
        identity: 7,
    }))
    .unwrap();
    super::workspace_input::complete_navigation(&mut state, &[], &[]);
    assert_eq!(state.view_mode, ViewMode::Process);
    assert!(state.zoom, "The owner may be hidden below its parent");
    assert!(state.workspace.navigation.is_some());

    // The expanded process snapshot arrives after the view changes.
    state.visible_pids = vec![sysinfo::Pid::from_u32(42), sysinfo::Pid::from_u32(100)];
    state.process_identities.insert(100, 7);
    super::workspace_input::complete_navigation(&mut state, &[], &[]);
    assert_eq!(state.selected, 1);
    assert!(state.workspace.navigation.is_none());
}

#[test]
fn project_inspector_follows_the_selected_resource_when_members_are_inserted() {
    let mut state = AppState::new();
    let mut p = project("demo", "/srv/demo");
    let port = ResourceRecord::simple(
        Resource::Port {
            protocol: "tcp".into(),
            port: 80,
            pid: 100,
            container: None,
        },
        "http".into(),
    );
    p.resources.push(port.clone());
    let record = p.record();
    state.workspace.projects.push(p.clone());
    state.workspace.inspect(record);
    p.resources.insert(
        0,
        ResourceRecord::simple(
            Resource::Pm2 {
                id: 7,
                pid: Some(101),
            },
            "worker".into(),
        ),
    );
    state.workspace.replace_projects(vec![p]);
    let inspector = state.workspace.inspector.as_ref().unwrap();
    assert_eq!(inspector.resource_selected, 1);
    assert_eq!(
        state.workspace.projects[0].resources[inspector.resource_selected].resource,
        port.resource
    );
}
