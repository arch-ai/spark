use super::*;
use ratatui::{backend::TestBackend, Terminal};

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
    }];
    for (width, height) in [(140, 36), (100, 24), (60, 20), (40, 16)] {
        let text = render(&state, &[], &[], width, height);
        assert!(text.contains("2.5 GB"), "size clipped at {width}x{height}");
        assert!(
            text.contains("Stop 2d ago"),
            "activity clipped at {width}x{height}"
        );
        assert!(text.contains("Enter/i details"));
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
