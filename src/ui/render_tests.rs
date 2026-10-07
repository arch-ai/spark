use super::*;
use ratatui::{backend::TestBackend, Terminal};

fn export_feature_snapshot(buffer: &ratatui::buffer::Buffer, label: &str) -> String {
    let (width, height) = (buffer.area.width, buffer.area.height);
    let text = (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    if let Ok(dir) = std::env::var("SPARK_SNAPSHOT_DIR") {
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(format!("{dir}/{label}-{width}x{height}.txt"), &text).unwrap();
        let cells=(0..height).flat_map(|y| (0..width).map(move |x| (x,y))).map(|(x,y)| { let cell=&buffer[(x,y)];serde_json::json!({"x":x,"y":y,"text":cell.symbol(),"fg":format!("{:?}",cell.fg),"bg":format!("{:?}",cell.bg),"reversed":cell.modifier.contains(Modifier::REVERSED),"bold":cell.modifier.contains(Modifier::BOLD),"italic":cell.modifier.contains(Modifier::ITALIC)}) }).collect::<Vec<_>>();
        std::fs::write(
            format!("{dir}/{label}-{width}x{height}.json"),
            serde_json::to_vec(&cells).unwrap(),
        )
        .unwrap();
    }
    text
}

#[test]
fn project_inspector_tabs_and_review_remain_usable_at_wide_and_small_sizes() {
    use crate::app::{
        history::{Metric, ObservedEvent},
        projects::{Project, Resource, ResourceRecord},
        workspace::{Cleanup, InspectorTab},
    };
    let mut state = AppState::new();
    state.view_mode = ViewMode::Projects;
    let mut r =
        ResourceRecord::simple(Resource::Container("0123456789abcdef".into()), "api".into());
    r.project = Some("path:/srv/demo".into());
    state.workspace.projects.push(Project {
        key: "path:/srv/demo".into(),
        name: "demo".into(),
        path: Some("/srv/demo".into()),
        resources: vec![r],
        cpu: 12.3,
        memory: 200 * 1024 * 1024,
        estimated: false,
        running_containers: 1,
        unmeasured: 0,
    });
    state.workspace.volumes = std::sync::Arc::new(vec![docker::DockerListItem {
        name: "data".into(),
        size: "950 GB".into(),
        activity: Some("Attached now".into()),
        detail_left: "Containers: api".into(),
        ..Default::default()
    }]);
    state.workspace.storage_loaded = true;
    state.workspace.projects[0]
        .resources
        .push(ResourceRecord::simple(
            Resource::Volume("data".into()),
            "data".into(),
        ));
    let record = state.workspace.projects[0].record();
    state.workspace.inspect(record);
    state
        .workspace
        .inspector
        .as_mut()
        .unwrap()
        .push("ERROR database request failed".into(), true);
    state.workspace.history.push(ObservedEvent {
        at: std::time::SystemTime::now(),
        resource: "container:0123456789abcdef".into(),
        project: Some("path:/srv/demo".into()),
        message: "api · die · exit 137".into(),
        warning: true,
    });
    for i in 0..30 {
        state.workspace.history.sample(
            "path:/srv/demo".into(),
            Metric {
                at: std::time::UNIX_EPOCH + std::time::Duration::from_secs(100 + i * 2),
                cpu: i as f32,
                memory: (100 + i) * 1024 * 1024,
                estimated: false,
            },
        );
    }
    for (width, height) in [(160, 36), (120, 24), (70, 20), (40, 16), (30, 10)] {
        state.term_width = width;
        state.term_height = height;
        for tab in InspectorTab::ALL {
            state.workspace.inspector.as_mut().unwrap().tab = tab;
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| {
                    render_ratatui(
                        frame,
                        &state,
                        &HashMap::new(),
                        &[],
                        &[],
                        &[],
                        &[],
                        &[],
                        &[],
                        &[],
                        &[],
                        &[],
                    )
                })
                .unwrap();
            let text = export_feature_snapshot(
                terminal.backend().buffer(),
                &format!("workspace-{}", tab.label().to_lowercase()),
            );
            assert!(
                text.contains(tab.label()),
                "active tab must remain readable at {width}x{height}: {text}"
            );
            if tab == InspectorTab::Details {
                assert!(
                    text.contains("api"),
                    "project resources must be reachable at {width}x{height}"
                );
            }
            if tab == InspectorTab::Storage {
                assert!(
                    text.contains("data"),
                    "volume must be visible at {width}x{height}"
                );
            }
            if width >= 140 {
                assert!(
                    text.contains("PROJECTS"),
                    "wide inspector must keep its table"
                );
            }
        }
    }
    state.workspace.cleanup = Cleanup::Review(crate::system::storage::CleanupPlan {
        volumes: vec!["data".into()],
        references: Default::default(),
        containers: vec![crate::system::storage::AffectedContainer {
            id: "0123456789abcdef".into(),
            name: "api".into(),
            image: "demo:v1".into(),
            status: "running".into(),
            project: "demo".into(),
            retained_volumes: vec!["backups".into()],
        }],
    });
    state.workspace.cleanup_open = true;
    let mut terminal = Terminal::new(TestBackend::new(160, 36)).unwrap();
    terminal
        .draw(|frame| {
            render_ratatui(
                frame,
                &state,
                &HashMap::new(),
                &[],
                &[],
                &[],
                &[],
                &[],
                &[],
                &[],
                &[],
                &[],
            )
        })
        .unwrap();
    let text = export_feature_snapshot(terminal.backend().buffer(), "workspace-cleanup");
    assert!(text.contains("Other volumes retained: backups"));
    assert!(text.contains("0123456789abcdef"));
}

#[test]
fn sort_menu_volume_directories_and_logo_motion_preserve_usable_rows_at_small_sizes() {
    let mut state = AppState::new();
    state.view_mode = ViewMode::Docker;
    state.docker_list_open = true;
    state.docker_list_kind = Some(DockerListKind::Volumes);
    state.docker_list_items = vec![docker::DockerListItem {
        name: "db".into(),
        id: "db".into(),
        size: "950 GB".into(),
        activity: Some("Attached now".into()),
        detail_left: "Containers: api".into(),
        detail_right: "Images: demo:latest".into(),
        detail_project: "Projects: demo: /srv/demo".into(),
        attachments: Some(vec![docker::VolumeAttachment {
            container: "api".into(),
            ..Default::default()
        }]),
        ..Default::default()
    }];
    for (width, height) in [(140, 36), (100, 24), (60, 20), (40, 16), (30, 10)] {
        state.term_width = width;
        state.term_height = height;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                render_ratatui(
                    frame,
                    &state,
                    &HashMap::new(),
                    &[],
                    &[],
                    &[],
                    &[],
                    &[],
                    &[],
                    &[],
                    &[],
                    &[],
                )
            })
            .unwrap();
        let text = export_feature_snapshot(terminal.backend().buffer(), "volume-projects");
        assert!(
            text.contains("950 GB"),
            "volume size must remain visible at {width}x{height}"
        );
        assert!(text.contains("db"));
        if width >= 40 {
            assert!(text.contains("/srv/demo"));
            assert!(text.contains("api"));
        }
        state.open_sort_menu();
        terminal
            .draw(|frame| {
                render_ratatui(
                    frame,
                    &state,
                    &HashMap::new(),
                    &[],
                    &[],
                    &[],
                    &[],
                    &[],
                    &[],
                    &[],
                    &[],
                    &[],
                )
            })
            .unwrap();
        let text = export_feature_snapshot(terminal.backend().buffer(), "sort-menu");
        assert!(text.contains("Activity"));
        assert!(text.contains("Name"));
        assert!(text.contains("Size"));
        state.sort_menu = None;
    }
    state.docker_list_open = false;
    state.view_mode = ViewMode::Process;
    state.term_width = 100;
    state.term_height = 24;
    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
    terminal
        .draw(|frame| {
            render_ratatui(
                frame,
                &state,
                &HashMap::new(),
                &[],
                &[],
                &[],
                &[],
                &[],
                &[],
                &[],
                &[],
                &[],
            )
        })
        .unwrap();
    let first = terminal.backend().buffer().clone();
    export_feature_snapshot(&first, "logo-rest");
    state.logo_frame = 1;
    terminal
        .draw(|frame| {
            render_ratatui(
                frame,
                &state,
                &HashMap::new(),
                &[],
                &[],
                &[],
                &[],
                &[],
                &[],
                &[],
                &[],
                &[],
            )
        })
        .unwrap();
    let second = terminal.backend().buffer();
    export_feature_snapshot(second, "logo-spark");
    assert_ne!(first[(9, 4)].symbol(), second[(9, 4)].symbol());
    for y in 9..14 {
        for x in 0..20 {
            assert_eq!(
                first[(x, y)],
                second[(x, y)],
                "logo animation must not move navigation"
            );
        }
    }
}

#[test]
fn process_memory_columns_show_self_tree_swap_and_rss_fallback() {
    let mib = 1024 * 1024;
    let entries = vec![
        process::ProcessEntry {
            pid: Pid::from_u32(100),
            name: "chrome".into(),
            cpu: 2.0,
            memory_bytes: 900 * mib,
            start_time: 1,
            memory_sample: Some(process::MemorySample {
                pss_bytes: 200 * mib,
                swap_pss_bytes: Some(10 * mib),
            }),
            user_id: None,
            parent: None,
            is_thread: false,
        },
        process::ProcessEntry {
            pid: Pid::from_u32(101),
            name: "renderer".into(),
            cpu: 3.0,
            memory_bytes: 400 * mib,
            start_time: 1,
            memory_sample: None,
            user_id: None,
            parent: Some(Pid::from_u32(100)),
            is_thread: false,
        },
    ];
    let processes = process::collect_processes_from_entries(&entries, "", &HashMap::new());
    let rows = process::build_tree_rows(&processes, SortBy::Memory, SortOrder::Desc, true);
    let mut state = AppState::new();
    state.zoom = true;
    for (width, height) in [(120, 30), (80, 24), (60, 20), (40, 16)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| render_process_view(frame, &state, frame.area(), &processes, &rows))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text = (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("chrome"), "name clipped at {width}x{height}");
        assert!(
            text.contains("~600M"),
            "tree total missing at {width}x{height}"
        );
        assert!(text.contains("TREE"));
        if width >= 52 {
            assert!(text.contains("200M"));
        }
        if width >= 76 {
            assert!(text.contains("SWAPtree"));
            assert!(text.contains('?'));
        }
        if let Ok(dir) = std::env::var("SPARK_SNAPSHOT_DIR") {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(format!("{dir}/process-memory-{width}x{height}.txt"), text).unwrap();
        }
    }
}

#[test]
fn long_volume_deletion_remains_visible_at_small_terminal_sizes() {
    let mut state = AppState::new();
    state.view_mode = ViewMode::Docker;
    for (width, height) in [(100, 24), (60, 20), (40, 16), (30, 10)] {
        state.pending_delete = Some(crate::app::DeleteConfirm {
            kind: DeleteKind::Volume,
            name: "abcdef0123456789".repeat(4),
            id: String::new(),
        });
        let text = render(&state, &[], &[], width, height);
        assert!(text.contains("[ Yes ]") && text.contains("[ No ]"));
        assert!(text.contains("Y: delete"));
        assert!(text.contains("abcdef0123456789"));
        if let Ok(dir) = std::env::var("SPARK_SNAPSHOT_DIR") {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(format!("{dir}/delete-{width}x{height}.txt"), text).unwrap();
        }
        state.pending_delete = None;
        state.delete_in_progress = Some(crate::app::DeleteProgress {
            label: "volume large-data".into(),
            started_at: std::time::Instant::now() - std::time::Duration::from_secs(185),
        });
        assert!(render(&state, &[], &[], width, height).contains("Deleting 3:05"));
        state.delete_in_progress = None;
    }
}

fn container(id: &str, name: &str, status: &str) -> docker::ContainerInfo {
    docker::ContainerInfo {
        id: id.into(),
        name: name.into(),
        image: "postgres:17-alpine".into(),
        port_public: "5432".into(),
        port_internal: "5432".into(),
        status: status.to_string().into(),
        group_name: "demo-project".into(),
        group_path: Some("/tmp/demo-project".into()),
        running: status.starts_with("Up"),
        memory: None,
        activity_secs: 60,
    }
}

fn render(
    state: &AppState,
    containers: &[docker::ContainerInfo],
    rows: &[docker::DockerRow],
    width: u16,
    height: u16,
) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal
        .draw(|frame| {
            render_ratatui(
                frame,
                state,
                &HashMap::new(),
                &[],
                containers,
                rows,
                &[],
                &[],
                &[],
                &[],
                &[],
                &[],
            )
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn docker_memory_stays_visible_and_sortable_at_compact_sizes() {
    let mut state = AppState::new();
    state.view_mode = ViewMode::Docker;
    state.docker_total = 1;
    state.docker_updated_at = Some(std::time::Instant::now());
    state.table_sorts[SortTarget::Docker as usize] =
        crate::app::sorting::TableSort::new(SortField::Memory, SortOrder::Desc);
    let mut api = container("a", "api", "Up 2 hours (unhealthy)");
    api.memory = Some(docker::memory::ContainerMemory {
        used_bytes: 3 << 29,
        limit_bytes: 2 << 30,
        percent: 75.0,
        measured_at: std::time::Instant::now(),
        stale: false,
    });
    for (width, height) in [(160, 36), (100, 24), (60, 20), (40, 16), (30, 10)] {
        state.term_width = width;
        state.term_height = height;
        let (containers, rows) = docker::group_containers(vec![api.clone()]);
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| {
                render_ratatui(
                    frame,
                    &state,
                    &HashMap::new(),
                    &[],
                    &containers,
                    &rows,
                    &[],
                    &[],
                    &[],
                    &[],
                    &[],
                    &[],
                )
            })
            .unwrap();
        let text = export_feature_snapshot(terminal.backend().buffer(), "docker-memory");
        assert!(
            text.contains("RAM ▼"),
            "RAM sort header missing at {width}x{height}: {text}"
        );
        assert!(
            text.contains("1.5G"),
            "RAM clipped at {width}x{height}: {text}"
        );
        assert!(state
            .rendered_headers
            .borrow()
            .hits
            .iter()
            .any(|hit| hit.target == SortTarget::Docker && hit.field == SortField::Memory));
    }
    api.memory.as_mut().unwrap().stale = true;
    let (containers, rows) = docker::group_containers(vec![api.clone()]);
    assert!(render(&state, &containers, &rows, 100, 24).contains("1.5G*"));
    api.running = false;
    let (containers, rows) = docker::group_containers(vec![api]);
    let text = render(&state, &containers, &rows, 100, 24);
    assert!(
        !text.contains("1.5G"),
        "Stopped containers must not show old measurements"
    );
}

#[test]
fn docker_view_snapshots() {
    let (containers, rows) = docker::group_containers(vec![
        container("0123456789abcdef", "database", "Up 2 hours (healthy)"),
        container("123456789abcdef0", "api", "Up 4 minutes (unhealthy)"),
        container("23456789abcdef01", "worker", "Exited (1) 3 minutes ago"),
    ]);
    let mut state = AppState::new();
    state.view_mode = ViewMode::Docker;
    state.docker_total = containers.len();
    state.docker_selected_row = 1;
    state.docker_updated_at = Some(std::time::Instant::now());
    state.docker_df_updated_at = state.docker_updated_at;
    for (width, height) in [(140, 36), (100, 24), (60, 20), (40, 16)] {
        let text = render(&state, &containers, &rows, width, height);
        assert!(text.contains("DOCKER"));
        assert!(
            text.contains("api"),
            "container name clipped at {width}x{height}"
        );
        assert!(
            text.contains("unhealthy"),
            "health clipped at {width}x{height}"
        );
        if let Ok(dir) = std::env::var("SPARK_SNAPSHOT_DIR") {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(format!("{dir}/docker-{width}x{height}.txt"), text).unwrap();
        }
    }
}

#[test]
fn unicode_truncation_never_splits_utf8_or_exceeds_display_width() {
    for value in [
        "éééééééé",
        "数据库日志文件",
        "👩‍💻 a long name",
        "a\u{301}b\u{301}c\u{301}",
    ] {
        for width in 0..16 {
            assert!(Span::raw(truncate(value, width)).width() <= width);
        }
    }
}

#[test]
fn docker_empty_loading_filter_and_error_states_are_distinct() {
    let mut state = AppState::new();
    state.view_mode = ViewMode::Docker;
    assert!(render(&state, &[], &[], 100, 24).contains("Loading containers"));
    state.docker_updated_at = Some(std::time::Instant::now());
    assert!(render(&state, &[], &[], 100, 24).contains("No containers in this Docker context"));
    state.docker_filter = "missing".into();
    assert!(render(&state, &[], &[], 100, 24).contains("No containers match"));
    state.docker_error = Some("permission denied".into());
    assert!(render(&state, &[], &[], 100, 24).contains("permission denied"));
}

#[test]
fn every_view_handles_tiny_terminals_and_resize() {
    let mut state = AppState::new();
    for view in [
        ViewMode::Process,
        ViewMode::Docker,
        ViewMode::Ports,
        ViewMode::Node,
        ViewMode::DockerEnv,
    ] {
        state.view_mode = view;
        for (width, height) in [(0, 0), (1, 1), (20, 5), (40, 12), (80, 24)] {
            render(&state, &[], &[], width, height);
        }
    }
}

#[test]
fn volume_list_exposes_size_and_container_activity_at_small_widths() {
    let mut state = AppState::new();
    state.view_mode = ViewMode::Docker;
    state.docker_list_open = true;
    state.docker_list_kind = Some(DockerListKind::Volumes);
    state.docker_list_items = vec![docker::DockerListItem {
        name: "db-data".into(),
        id: "db-data".into(),
        size: "2.5 GB".into(),
        activity: Some("Stop 2d ago".into()),
        detail_left: "Containers: db".into(),
        detail_right: "Images: postgres:17".into(),
        ..Default::default()
    }];
    for (width, height) in [(140, 36), (100, 24), (60, 20), (40, 16)] {
        let text = render(&state, &[], &[], width, height);
        assert!(text.contains("2.5 GB"), "size clipped at {width}x{height}");
        assert!(
            text.contains("Stop 2d ago"),
            "activity clipped at {width}x{height}"
        );
        assert!(text.contains("Enter details"));
        assert!(
            text.contains("Containers: db"),
            "container hidden at {width}x{height}"
        );
        assert!(
            text.contains("Images: postgres:17"),
            "image hidden at {width}x{height}"
        );
        if let Ok(dir) = std::env::var("SPARK_SNAPSHOT_DIR") {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(format!("{dir}/volumes-{width}x{height}.txt"), text).unwrap();
        }
    }
}

#[test]
fn audited_resource_views_keep_identity_status_and_values_visible() {
    let ports = vec![ports::PortInfo {
        proto: "tcp".into(),
        port: 8080,
        internal_port: Some(80),
        pid: Pid::from_u32(0),
        name: "docker:api".into(),
        exe_path: "image:node:24".into(),
        container_id: Some("api-id".into()),
        group_name: Some("demo".into()),
        project_name: None,
    }];
    let port_rows = ports::group_ports(&ports);
    let native = vec![node::NodeProcessInfo {
        pid: Pid::from_u32(222),
        name: "dev-server".into(),
        script: "/app/dev.js".into(),
        project_name: Some("demo".into()),
        uses_nvm: false,
        cpu: 2.5,
        memory_bytes: 123 * 1024 * 1024,
        uptime_secs: Some(120),
        pm2: None,
        worker_count: 1,
    }];
    let native_rows = vec![
        node::NodeRow::Group {
            name: "demo-project".into(),
            count: 1,
        },
        node::NodeRow::Item { index: 0 },
    ];
    let pm2 = vec![node::Pm2Process {
        pm_id: 7,
        name: "api-worker".into(),
        mode: "fork".into(),
        status: "online".into(),
        pid: Some(111),
        cpu: Some(1.5),
        memory_bytes: Some(45 * 1024 * 1024),
        uptime_ms: Some(60000),
        script: Some("/app/api.js".into()),
        cwd: Some("/app".into()),
    }];
    let mut state = AppState::new();
    state.pm2_available = true;
    state.process_loaded = true;
    state.ports_loaded = true;
    state.node_loaded = true;
    state.pm2_selected = 0;
    for (width, height) in [(140, 36), (100, 24), (60, 20), (40, 16), (30, 10)] {
        state.term_width = width;
        state.term_height = height;
        for (view, tab) in [
            (ViewMode::Ports, crate::app::NodeTab::Processes),
            (ViewMode::Node, crate::app::NodeTab::Processes),
            (ViewMode::Node, crate::app::NodeTab::Pm2),
        ] {
            state.view_mode = view;
            state.node_tab = tab;
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| {
                    render_ratatui(
                        frame,
                        &state,
                        &HashMap::new(),
                        &[],
                        &[],
                        &[],
                        &ports,
                        &port_rows,
                        &native,
                        &native_rows,
                        &pm2,
                        &[0],
                    )
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let text = (0..height)
                .map(|y| {
                    (0..width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n");
            if view == ViewMode::Ports {
                assert!(
                    text.contains("docker:api"),
                    "port owner clipped at {width}x{height}"
                );
                assert!(text.contains("8080:80"));
                assert!(text.contains("PROTO"));
            } else {
                assert!(text.contains("Node.js Processes"));
                assert!(text.contains("PM2"));
                let tabs = super::super::layout::node_layout(super::super::layout::main_area(
                    ratatui::layout::Rect::new(0, 0, width, height),
                ))[1];
                let active_x = tabs.x
                    + if tab == crate::app::NodeTab::Pm2 {
                        23
                    } else {
                        1
                    };
                assert_eq!(buffer[(active_x, tabs.y)].bg, Color::Cyan);
                if tab == crate::app::NodeTab::Pm2 {
                    assert!(
                        text.contains("api-worker"),
                        "PM2 identity clipped at {width}x{height}"
                    );
                    assert!(text.contains("online"));
                    assert!(text.contains("45M"));
                    assert!(!text.contains("dev-server"));
                } else {
                    assert!(
                        text.contains("dev-server"),
                        "Node identity clipped at {width}x{height}"
                    );
                    assert!(text.contains("123M"));
                    assert!(!text.contains("api-worker"));
                }
            }
            if let Ok(dir) = std::env::var("SPARK_SNAPSHOT_DIR") {
                std::fs::create_dir_all(&dir).unwrap();
                let label = if view == ViewMode::Ports {
                    "ports"
                } else if tab == crate::app::NodeTab::Pm2 {
                    "node-pm2"
                } else {
                    "node-processes"
                };
                std::fs::write(format!("{dir}/{label}-{width}x{height}.txt"), &text).unwrap();
                let cells: Vec<_> = (0..height).flat_map(|y| (0..width).map(move |x| (x,y))).map(|(x,y)| {
                    let cell = &buffer[(x,y)];
                    serde_json::json!({"x":x,"y":y,"text":cell.symbol(),"fg":format!("{:?}",cell.fg),"bg":format!("{:?}",cell.bg),"reversed":cell.modifier.contains(Modifier::REVERSED),"bold":cell.modifier.contains(Modifier::BOLD)})
                }).collect();
                std::fs::write(
                    format!("{dir}/{label}-{width}x{height}.json"),
                    serde_json::to_vec(&cells).unwrap(),
                )
                .unwrap();
            }
        }
    }
}

#[test]
fn pm2_tab_keeps_loading_unavailable_empty_and_error_states_visible_at_small_sizes() {
    for (width, height) in [(100, 24), (30, 10)] {
        for (label, loaded, loading, available, error, expected) in [
            ("pm2-loading", false, true, false, None, "Loading..."),
            (
                "pm2-unavailable",
                true,
                false,
                false,
                None,
                "PM2 unavailable.",
            ),
            ("pm2-empty", true, false, true, None, "No PM2 processes."),
            (
                "pm2-error",
                true,
                false,
                false,
                Some("PM2 query failed"),
                "PM2 query failed",
            ),
        ] {
            let mut state = AppState::new();
            state.view_mode = ViewMode::Node;
            state.node_tab = NodeTab::Pm2;
            state.node_loaded = loaded;
            state.pm2_loading = loading;
            state.pm2_available = available;
            state.pm2_error = error.map(str::to_owned);
            if error.is_some() {
                state.node_filter = "active filter".into();
            }
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| render_node_view(frame, &state, frame.area(), &[], &[], &[], &[]))
                .unwrap();
            let text = export_feature_snapshot(terminal.backend().buffer(), label);
            assert!(text.contains("Node.js Processes"));
            assert!(
                text.contains(expected),
                "{label} at {width}x{height}: {text}"
            );
            if error.is_some() || (loaded && !available) {
                assert!(text.contains("F5"));
            }
            let hits = &state.rendered_headers.borrow().hits;
            assert!(hits.iter().all(|hit| hit.target == SortTarget::Pm2));
        }
    }
}
