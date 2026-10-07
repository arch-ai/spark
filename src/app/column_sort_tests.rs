use super::*;
use crate::app::sorting::{SortField, SortTarget, TableSort};
use crate::app::SortOrder;
use crate::system::{docker, node, ports, process};
use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};
use std::collections::HashMap;

fn draw(state: &AppState) -> Buffer {
    let pid = sysinfo::Pid::from_u32(100);
    let processes = HashMap::from([(
        pid,
        process::ProcInfo {
            name: "app".into(),
            name_lower: "app".into(),
            cpu: 1.0,
            memory_bytes: 10,
            memory_estimated: false,
            tree_memory_bytes: 20,
            tree_memory_estimated: false,
            tree_swap_bytes: Some(0),
            user: "long-owner".into(),
            parent: None,
        },
    )]);
    let process_rows = vec![process::TreeRow {
        pid,
        prefix: String::new(),
    }];
    let (containers, container_rows) = docker::group_containers_sorted(
        vec![docker::ContainerInfo {
            id: "container-id".into(),
            name: "api".into(),
            image: "demo:latest".into(),
            port_public: "8080".into(),
            port_internal: "80".into(),
            status: "Up 2 hours".into(),
            group_name: "demo".into(),
            group_path: Some("/srv/demo".into()),
            running: true,
            memory: None,
            activity_secs: 0,
        }],
        Some(state.sort_for(SortTarget::Docker)),
    );
    let ports = vec![ports::PortInfo {
        proto: "tcp".into(),
        port: 8080,
        internal_port: Some(80),
        pid,
        name: "api".into(),
        exe_path: "/app/api".into(),
        container_id: None,
        group_name: None,
        project_name: None,
    }];
    let port_rows = ports::group_ports(&ports);
    let native = vec![node::NodeProcessInfo {
        pid,
        name: "dev-server".into(),
        script: "/app/dev.js".into(),
        project_name: None,
        uses_nvm: false,
        cpu: 1.0,
        memory_bytes: 20,
        uptime_secs: Some(120),
        pm2: None,
        worker_count: 1,
    }];
    let native_rows = vec![node::NodeRow::Item { index: 0 }];
    let pm2 = vec![node::Pm2Process {
        pm_id: 7,
        name: "worker".into(),
        mode: "fork".into(),
        status: "online".into(),
        pid: Some(111),
        cpu: Some(1.0),
        memory_bytes: Some(20),
        uptime_ms: Some(60000),
        script: None,
        cwd: None,
    }];
    let mut terminal =
        Terminal::new(TestBackend::new(state.term_width, state.term_height)).unwrap();
    terminal
        .draw(|frame| {
            crate::ui::render_ratatui(
                frame,
                state,
                &processes,
                &process_rows,
                &containers,
                &container_rows,
                &ports,
                &port_rows,
                &native,
                &native_rows,
                &pm2,
                &[0],
            )
        })
        .unwrap();
    terminal.backend().buffer().clone()
}

fn column_label(target: SortTarget, field: SortField) -> &'static str {
    match field {
        SortField::Name if target == SortTarget::Docker => "NAME / PROJECT",
        SortField::Name => "NAME",
        SortField::Memory if target == SortTarget::Process => "TREE",
        SortField::Memory if target == SortTarget::Docker => "RAM",
        SortField::Memory => "RSS",
        SortField::SelfMemory => "RAM",
        SortField::Swap => "SWAPtree",
        SortField::User => "USER",
        SortField::Pid => "PID",
        SortField::Cpu => "CPU",
        SortField::Id => "ID",
        SortField::Status => "STATUS",
        SortField::Image => "IMAGE",
        SortField::Port if target == SortTarget::Docker => "HOST PORTS",
        SortField::Port => "EXT:INT",
        SortField::Protocol => "PROTO",
        SortField::Command => "COMMAND",
        SortField::Script => "SCRIPT",
        SortField::Mode => "MODE",
        SortField::Uptime => "UPTIME",
        SortField::Size => "SIZE",
        SortField::Activity => "ACTIVITY",
    }
}

fn click(mouse_x: u16, mouse_y: u16, state: &mut AppState) {
    let mouse = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: mouse_x,
        row: mouse_y,
        modifiers: KeyModifiers::NONE,
    };
    let (width, height) = (state.term_width, state.term_height);
    assert!(handle_mouse_event(
        mouse,
        state,
        &[],
        &[],
        &[],
        &[],
        width,
        height
    ));
}

#[test]
fn rendered_column_headers_sort_and_reverse_without_changing_selected_resource_identity() {
    let mut checked = 0;
    for (width, height) in [(140, 36), (100, 24), (60, 20), (40, 16), (30, 10)] {
        for (view, resource, pm2) in [
            (ViewMode::Process, None, false),
            (ViewMode::Docker, None, false),
            (ViewMode::Ports, None, false),
            (ViewMode::Node, None, false),
            (ViewMode::Node, None, true),
            (ViewMode::Docker, Some(DockerListKind::Images), false),
            (ViewMode::Docker, Some(DockerListKind::Containers), false),
            (ViewMode::Docker, Some(DockerListKind::Volumes), false),
        ] {
            let mut state = AppState::new();
            state.view_mode = view;
            state.term_width = width;
            state.term_height = height;
            state.pm2_available = pm2;
            state.node_tab = if pm2 {
                NodeTab::Pm2
            } else {
                NodeTab::Processes
            };
            state.docker_list_open = resource.is_some();
            state.docker_list_kind = resource;
            state.docker_list_items = vec![
                docker::DockerListItem {
                    name: "zeta".into(),
                    id: "selected-id".into(),
                    size: "1 GB".into(),
                    activity_age_secs: Some(0),
                    ..Default::default()
                },
                docker::DockerListItem {
                    name: "alpha".into(),
                    id: "other-id".into(),
                    size: "9 GB".into(),
                    activity_age_secs: Some(10),
                    ..Default::default()
                },
            ];
            let buffer = draw(&state);
            let hits = state.rendered_headers.borrow().hits.clone();
            assert!(
                !hits.is_empty(),
                "missing headers: {view:?} {resource:?} {width}x{height}"
            );
            if let Ok(dir) = std::env::var("SPARK_SNAPSHOT_DIR") {
                std::fs::create_dir_all(&dir).unwrap();
                let cells=(0..height).flat_map(|y| (0..width).map(move |x| (x,y))).map(|(x,y)| { let cell=&buffer[(x,y)];serde_json::json!({"x":x,"y":y,"text":cell.symbol(),"fg":format!("{:?}",cell.fg),"bg":format!("{:?}",cell.bg),"reversed":cell.modifier.contains(ratatui::style::Modifier::REVERSED),"bold":cell.modifier.contains(ratatui::style::Modifier::BOLD),"italic":cell.modifier.contains(ratatui::style::Modifier::ITALIC)}) }).collect::<Vec<_>>();
                std::fs::write(
                    format!(
                        "{dir}/column-headers-{view:?}-{resource:?}-{pm2}-{width}x{height}.json"
                    ),
                    serde_json::to_vec(&cells).unwrap(),
                )
                .unwrap();
            }
            // Check each recorded rectangle against the real Table widget's output.
            for hit in &hits {
                let sort = state.sort_for(hit.target);
                let label = column_label(hit.target, hit.field);
                let expected = if sort.field == hit.field {
                    format!(
                        "{label} {}",
                        if sort.order == SortOrder::Asc {
                            "▲"
                        } else {
                            "▼"
                        }
                    )
                } else {
                    label.to_owned()
                };
                let actual = (hit.area.x..hit.area.right())
                    .map(|x| buffer[(x, hit.area.y)].symbol())
                    .collect::<String>();
                let expected = expected
                    .chars()
                    .take(hit.area.width as usize)
                    .collect::<String>();
                assert_eq!(
                    actual.trim_end(),
                    expected.trim_end(),
                    "header mismatch {view:?} {width}x{height}: {hit:?}"
                );
            }
            for hit in hits {
                let prior = state.sort_for(hit.target);
                let order = if prior.field == hit.field {
                    prior.order.toggle()
                } else {
                    hit.field.default_order()
                };
                click(hit.area.x + hit.area.width / 2, hit.area.y, &mut state);
                assert_eq!(state.sort_for(hit.target), TableSort::new(hit.field, order));
                if matches!(hit.target, SortTarget::Node | SortTarget::Pm2) {
                    assert_eq!(
                        state.node_tab == NodeTab::Pm2,
                        hit.target == SortTarget::Pm2
                    );
                }
                click(hit.area.x, hit.area.y, &mut state);
                assert_eq!(
                    state.sort_for(hit.target),
                    TableSort::new(hit.field, order.toggle())
                );
                if resource.is_some() {
                    assert_eq!(
                        state.docker_list_items[state.docker_list_selected].id,
                        "selected-id"
                    );
                }
                checked += 1;
            }
        }
    }
    assert!(checked > 100);
}

#[test]
fn hidden_stale_or_covered_headers_do_not_sort_the_background_table() {
    let mut state = AppState::new();
    state.term_width = 100;
    state.term_height = 24;
    draw(&state);
    let hit = state.rendered_headers.borrow().hits[0];
    let before = state.sort_for(hit.target);
    let mouse = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: hit.area.x,
        row: hit.area.y,
        modifiers: KeyModifiers::NONE,
    };
    assert!(!handle_sort_header_click(mouse, &mut state, 60, 20));
    state.view_mode = ViewMode::Ports;
    assert!(!handle_sort_header_click(mouse, &mut state, 100, 24));
    state.view_mode = ViewMode::Process;
    state.docker_list_open = true;
    state.docker_list_kind = Some(DockerListKind::Volumes);
    assert!(!handle_sort_header_click(mouse, &mut state, 100, 24));
    state.docker_list_open = false;
    state.env_modal_open = true;
    handle_mouse_event(mouse, &mut state, &[], &[], &[], &[], 100, 24);
    assert_eq!(state.sort_for(hit.target), before);
    state.env_modal_open = false;
    state.term_width = 20;
    state.term_height = 8;
    draw(&state);
    assert!(state.rendered_headers.borrow().hits.is_empty());
}

#[test]
fn switching_node_tabs_rejects_headers_from_the_previous_tab_until_rendered() {
    let mut state = AppState::new();
    state.view_mode = ViewMode::Node;
    state.term_width = 100;
    state.term_height = 24;
    draw(&state);
    let hit = state.rendered_headers.borrow().hits[0];
    let before = state.sort_for(SortTarget::Node);
    state.set_node_tab(NodeTab::Pm2);
    let mouse = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: hit.area.x,
        row: hit.area.y,
        modifiers: KeyModifiers::NONE,
    };
    assert!(!handle_sort_header_click(mouse, &mut state, 100, 24));
    assert_eq!(state.node_tab, NodeTab::Pm2);
    assert_eq!(state.sort_for(SortTarget::Node), before);
    draw(&state);
    assert!(state
        .rendered_headers
        .borrow()
        .hits
        .iter()
        .all(|hit| hit.target == SortTarget::Pm2));
}

#[test]
fn search_clicks_follow_the_rendered_column_and_never_activate_from_view_details() {
    for (width, height) in [(140, 36), (100, 24), (90, 20), (89, 20), (40, 16), (30, 10)] {
        for (view, tab) in [
            (ViewMode::Process, NodeTab::Processes),
            (ViewMode::Ports, NodeTab::Processes),
            (ViewMode::Docker, NodeTab::Processes),
            (ViewMode::Node, NodeTab::Processes),
            (ViewMode::Node, NodeTab::Pm2),
        ] {
            let mut state = AppState::new();
            state.view_mode = view;
            state.node_tab = tab;
            state.term_width = width;
            state.term_height = height;
            let buffer = draw(&state);
            let (search_x, search_y) = (0..height)
                .find_map(|y| {
                    let text = (0..width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>();
                    text.find("/ to filter...")
                        .map(|x| (ratatui::text::Span::raw(&text[..x]).width() as u16, y))
                })
                .expect("rendered search input");
            let main = crate::ui::layout::main_area(buffer.area);
            if search_x > main.x + 10 {
                click(search_x - 3, search_y, &mut state);
                assert_eq!(
                    state.input_mode,
                    InputMode::Normal,
                    "details entered filter mode: {view:?} {width}x{height}"
                );
            }
            click(search_x + 2, search_y, &mut state);
            assert_eq!(
                state.input_mode,
                InputMode::Filter,
                "search click missed: {view:?} {width}x{height}"
            );
            let mut system = System::new();
            handle_key_event(
                KeyEvent::new(KeyCode::Char('Z'), KeyModifiers::NONE),
                &mut state,
                &mut system,
                &[],
                &[],
                &[],
                &[],
            );
            assert_eq!(state.active_filter(), "Z");
            assert_eq!(state.node_tab, tab);
        }
    }
}
