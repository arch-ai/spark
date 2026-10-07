use crate::app::{SortBy, SortOrder};
use crate::system::{docker, node, ports};
use ratatui::layout::Rect;
use std::cmp::Ordering;

#[derive(Clone, Copy, Debug)]
pub struct SortHeaderHit {
    pub area: Rect,
    pub target: SortTarget,
    pub field: SortField,
}

/// Geometry of the last rendered frame; input never guesses responsive columns.
#[derive(Default)]
pub struct RenderedHeaders {
    pub bounds: Rect,
    pub view: Option<super::ViewMode>,
    pub resource: Option<super::DockerListKind>,
    pub node_tab: Option<super::NodeTab>,
    pub hits: Vec<SortHeaderHit>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortField {
    Name,
    Cpu,
    Memory,
    Pid,
    Port,
    Protocol,
    Size,
    Activity,
    Status,
    Image,
    Uptime,
    Id,
    SelfMemory,
    Swap,
    User,
    Command,
    Script,
    Mode,
}
impl SortField {
    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Cpu => "CPU",
            Self::Memory => "RAM",
            Self::Pid => "PID",
            Self::Port => "Port",
            Self::Protocol => "Protocol",
            Self::Size => "Size",
            Self::Activity => "Activity",
            Self::Status => "Status",
            Self::Image => "Image",
            Self::Uptime => "Uptime",
            Self::Id => "ID",
            Self::SelfMemory => "RAM",
            Self::Swap => "Tree swap",
            Self::User => "User",
            Self::Command => "Command",
            Self::Script => "Script",
            Self::Mode => "Mode",
        }
    }
    pub fn default_order(self) -> SortOrder {
        if matches!(
            self,
            Self::Cpu | Self::Memory | Self::SelfMemory | Self::Swap | Self::Size | Self::Uptime
        ) {
            SortOrder::Desc
        } else {
            SortOrder::Asc
        }
    }
    pub fn process(self) -> Option<SortBy> {
        match self {
            Self::Cpu => Some(SortBy::Cpu),
            Self::Memory => Some(SortBy::Memory),
            Self::Name => Some(SortBy::Name),
            Self::Pid => Some(SortBy::Pid),
            Self::SelfMemory => Some(SortBy::SelfMemory),
            Self::Swap => Some(SortBy::Swap),
            Self::User => Some(SortBy::User),
            _ => None,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableSort {
    pub field: SortField,
    pub order: SortOrder,
}
impl TableSort {
    pub const fn new(field: SortField, order: SortOrder) -> Self {
        Self { field, order }
    }
    pub fn label(self) -> String {
        format!(
            "{} {}",
            self.field.label(),
            if self.order == SortOrder::Asc {
                "asc"
            } else {
                "desc"
            }
        )
    }
    fn ordered(self, ordering: Ordering) -> Ordering {
        if self.order == SortOrder::Desc {
            ordering.reverse()
        } else {
            ordering
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum SortTarget {
    Process,
    Docker,
    Ports,
    Node,
    Pm2,
    Images,
    Containers,
    Volumes,
    Projects,
}
impl SortTarget {
    pub fn field_label(self, field: SortField) -> &'static str {
        if field == SortField::Memory {
            return match self {
                Self::Process => "Tree RAM",
                Self::Projects => "Native RAM",
                Self::Docker => "RAM",
                _ => "RSS",
            };
        }
        field.label()
    }
    pub fn sort_label(self, sort: TableSort) -> String {
        format!(
            "{} {}",
            self.field_label(sort.field),
            if sort.order == SortOrder::Asc {
                "asc"
            } else {
                "desc"
            }
        )
    }
    pub fn fields(self) -> &'static [SortField] {
        use SortField::*;
        match self {
            Self::Projects => &[Name, Cpu, Memory],
            Self::Process => &[Cpu, Memory, Name, SelfMemory, Swap, User, Pid],
            Self::Docker => &[Name, Memory, Image, Status, Activity, Port, Id],
            Self::Ports => &[Port, Name, Protocol, Pid, Command],
            Self::Node => &[Name, Cpu, Memory, Pid, Script, Uptime],
            Self::Pm2 => &[Name, Cpu, Memory, Id, Pid, Status, Mode, Uptime],
            Self::Volumes => &[Size, Name, Activity],
            Self::Images | Self::Containers => &[Size, Name, Id],
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct SortMenu {
    pub target: SortTarget,
    pub selected: usize,
}

fn text_cmp(a: &str, b: &str) -> Ordering {
    a.to_lowercase()
        .cmp(&b.to_lowercase())
        .then_with(|| a.cmp(b))
}
fn optional_cmp<T>(
    a: Option<T>,
    b: Option<T>,
    sort: TableSort,
    compare: impl FnOnce(T, T) -> Ordering,
) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => sort.ordered(compare(a, b)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        _ => Ordering::Equal,
    }
}

pub fn compare_containers(
    a: &docker::ContainerInfo,
    b: &docker::ContainerInfo,
    sort: TableSort,
) -> Ordering {
    if sort.field == SortField::Memory {
        let measured = |c: &docker::ContainerInfo| {
            c.memory
                .filter(|m| c.running && !m.stale)
                .map(|m| m.used_bytes)
        };
        return optional_cmp(measured(a), measured(b), sort, |a, b| a.cmp(&b))
            .then_with(|| text_cmp(&a.name, &b.name))
            .then_with(|| a.id.cmp(&b.id));
    }
    if sort.field == SortField::Port {
        return optional_cmp(
            host_port(&a.port_public),
            host_port(&b.port_public),
            sort,
            |a, b| a.cmp(&b),
        )
        .then_with(|| text_cmp(&a.name, &b.name))
        .then_with(|| a.id.cmp(&b.id));
    }
    if sort.field == SortField::Activity {
        return optional_cmp(
            (a.activity_secs != u64::MAX).then_some(a.activity_secs),
            (b.activity_secs != u64::MAX).then_some(b.activity_secs),
            sort,
            |a, b| a.cmp(&b),
        )
        .then_with(|| text_cmp(&a.name, &b.name))
        .then_with(|| a.id.cmp(&b.id));
    }
    let order = match sort.field {
        SortField::Image => text_cmp(&a.image, &b.image),
        SortField::Status => text_cmp(&a.status, &b.status),
        SortField::Id => text_cmp(&a.id, &b.id),
        SortField::Activity => a.activity_secs.cmp(&b.activity_secs),
        _ => text_cmp(&a.name, &b.name),
    };
    sort.ordered(order)
        .then_with(|| text_cmp(&a.name, &b.name))
        .then_with(|| a.id.cmp(&b.id))
}

fn host_port(text: &str) -> Option<u16> {
    text.split(',')
        .filter_map(|port| port.trim().split(['-', '/']).next()?.parse().ok())
        .min()
}
pub fn sort_ports(rows: &mut [ports::PortInfo], sort: TableSort) {
    rows.sort_by(|a, b| {
        let order = match sort.field {
            SortField::Port => sort.ordered(a.port.cmp(&b.port)),
            SortField::Protocol => sort.ordered(text_cmp(&a.proto, &b.proto)),
            SortField::Command => sort.ordered(text_cmp(&a.exe_path, &b.exe_path)),
            SortField::Pid => optional_cmp(
                (a.pid.as_u32() != 0).then_some(a.pid),
                (b.pid.as_u32() != 0).then_some(b.pid),
                sort,
                |a, b| a.cmp(&b),
            ),
            _ => sort.ordered(text_cmp(&a.name, &b.name)),
        };
        order
            .then_with(|| a.port.cmp(&b.port))
            .then_with(|| a.proto.cmp(&b.proto))
            .then_with(|| a.pid.cmp(&b.pid))
            .then_with(|| a.container_id.cmp(&b.container_id))
    });
}
pub fn sort_node(rows: &mut [node::NodeProcessInfo], sort: TableSort) {
    rows.sort_by(|a, b| {
        let order = match sort.field {
            SortField::Cpu => sort.ordered(a.cpu.total_cmp(&b.cpu)),
            SortField::Memory => sort.ordered(a.memory_bytes.cmp(&b.memory_bytes)),
            SortField::Pid => sort.ordered(a.pid.cmp(&b.pid)),
            SortField::Script => sort.ordered(text_cmp(&a.script, &b.script)),
            SortField::Uptime => optional_cmp(a.uptime_secs, b.uptime_secs, sort, |a, b| a.cmp(&b)),
            _ => sort.ordered(text_cmp(&a.name, &b.name)),
        };
        order
            .then_with(|| a.pid.cmp(&b.pid))
            .then_with(|| a.script.cmp(&b.script))
    });
}
pub fn sort_pm2(rows: &mut [node::Pm2Process], sort: TableSort) {
    rows.sort_by(|a, b| {
        let order = match sort.field {
            SortField::Cpu => optional_cmp(a.cpu, b.cpu, sort, |a, b| a.total_cmp(&b)),
            SortField::Memory => {
                optional_cmp(a.memory_bytes, b.memory_bytes, sort, |a, b| a.cmp(&b))
            }
            SortField::Pid => optional_cmp(a.pid, b.pid, sort, |a, b| a.cmp(&b)),
            SortField::Uptime => optional_cmp(a.uptime_ms, b.uptime_ms, sort, |a, b| a.cmp(&b)),
            SortField::Id => sort.ordered(a.pm_id.cmp(&b.pm_id)),
            SortField::Status => sort.ordered(text_cmp(&a.status, &b.status)),
            SortField::Mode => sort.ordered(text_cmp(&a.mode, &b.mode)),
            _ => sort.ordered(text_cmp(&a.name, &b.name)),
        };
        order.then_with(|| a.pm_id.cmp(&b.pm_id))
    });
}
pub fn size_bytes(raw: &str) -> Option<u64> {
    let raw = raw.split('(').next()?.trim();
    let split = raw
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(raw.len());
    let value: f64 = raw[..split].parse().ok()?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    let multiplier = match raw[split..].trim().to_ascii_lowercase().as_str() {
        "b" | "" => 1.0,
        "k" | "kb" => 1e3,
        "m" | "mb" => 1e6,
        "g" | "gb" => 1e9,
        "t" | "tb" => 1e12,
        "kib" => 1024.0,
        "mib" => 1048576.0,
        "gib" => 1073741824.0,
        "tib" => 1099511627776.0,
        _ => return None,
    };
    Some((value * multiplier) as u64)
}
pub fn sort_resources(rows: &mut [docker::DockerListItem], sort: TableSort) {
    rows.sort_by(|a, b| {
        let order = match sort.field {
            SortField::Size => {
                optional_cmp(size_bytes(&a.size), size_bytes(&b.size), sort, |a, b| {
                    a.cmp(&b)
                })
            }
            SortField::Id => sort.ordered(text_cmp(&a.id, &b.id)),
            SortField::Activity => {
                optional_cmp(a.activity_age_secs, b.activity_age_secs, sort, |a, b| {
                    a.cmp(&b)
                })
            }
            _ => sort.ordered(text_cmp(&a.name, &b.name)),
        };
        order
            .then_with(|| text_cmp(&a.name, &b.name))
            .then_with(|| a.id.cmp(&b.id))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_ports_sort_numerically_including_udp_ranges_and_missing_bindings() {
        assert_eq!(host_port("9000,80/udp,100-110"), Some(80));
        assert_eq!(host_port("100-110/udp"), Some(100));
        assert_eq!(host_port("-"), None);
        let container = |name: &str, ports: &str| docker::ContainerInfo {
            id: name.into(),
            name: name.into(),
            image: "demo".into(),
            port_public: ports.to_owned().into(),
            port_internal: "-".into(),
            status: "Up".into(),
            group_name: "Other".into(),
            group_path: None,
            running: true,
            memory: None,
            activity_secs: 0,
        };
        let source = vec![
            container("large", "100/udp"),
            container("small", "9"),
            container("unbound", "-"),
        ];
        for order in [SortOrder::Asc, SortOrder::Desc] {
            let (rows, _) = docker::group_containers_sorted(
                source.clone(),
                Some(TableSort::new(SortField::Port, order)),
            );
            assert_eq!(
                rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>(),
                if order == SortOrder::Asc {
                    vec!["small", "large", "unbound"]
                } else {
                    vec!["large", "small", "unbound"]
                }
            );
        }
    }
    fn item(name: &str, size: &str, age: Option<u64>) -> docker::DockerListItem {
        docker::DockerListItem {
            name: name.into(),
            id: name.into(),
            size: size.into(),
            activity_age_secs: age,
            ..Default::default()
        }
    }
    #[test]
    fn sizes_and_activity_use_numeric_values_and_unknowns_stay_last_in_both_directions() {
        let source = vec![
            item("large", "950 GB", Some(20 * 86400)),
            item("small", "100 MB", Some(3 * 86400)),
            item("medium", "9 GB", Some(0)),
            item("unknown", "Unknown", None),
        ];
        for order in [SortOrder::Asc, SortOrder::Desc] {
            let mut rows = source.clone();
            sort_resources(&mut rows, TableSort::new(SortField::Size, order));
            let names = rows
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>();
            assert_eq!(
                names,
                if order == SortOrder::Asc {
                    vec!["small", "medium", "large", "unknown"]
                } else {
                    vec!["large", "medium", "small", "unknown"]
                }
            );
            sort_resources(&mut rows, TableSort::new(SortField::Activity, order));
            let names = rows
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<_>>();
            assert_eq!(
                names,
                if order == SortOrder::Asc {
                    vec!["medium", "small", "large", "unknown"]
                } else {
                    vec!["large", "small", "medium", "unknown"]
                }
            );
        }
        assert_eq!(size_bytes("1 MiB (virtual 900 GB)"), Some(1048576));
        for invalid in ["Unknown", "-1 GB", "NaN GB", "1 invalid"] {
            assert!(size_bytes(invalid).is_none());
        }
    }
    fn pm2(id: u32, name: &str, memory: Option<u64>) -> node::Pm2Process {
        node::Pm2Process {
            pm_id: id,
            name: name.into(),
            pid: Some(id + 100),
            status: "online".into(),
            mode: "fork".into(),
            cpu: Some(id as f32),
            memory_bytes: memory,
            uptime_ms: Some(id as u64 * 1000),
            script: None,
            cwd: None,
        }
    }
    #[test]
    fn pm2_memory_pid_and_name_orders_preserve_identity_and_do_not_promote_missing_measurements() {
        let mut rows = vec![
            pm2(7, "z-worker", Some(9)),
            pm2(8, "A-worker", Some(100)),
            pm2(9, "unknown", None),
        ];
        sort_pm2(
            &mut rows,
            TableSort::new(SortField::Memory, SortOrder::Desc),
        );
        assert_eq!(
            rows.iter().map(|row| row.pm_id).collect::<Vec<_>>(),
            [8, 7, 9]
        );
        sort_pm2(&mut rows, TableSort::new(SortField::Name, SortOrder::Asc));
        assert_eq!(
            rows.iter().map(|row| row.pm_id).collect::<Vec<_>>(),
            [8, 9, 7]
        );
        sort_pm2(&mut rows, TableSort::new(SortField::Pid, SortOrder::Desc));
        assert_eq!(
            rows.iter().map(|row| row.pm_id).collect::<Vec<_>>(),
            [9, 8, 7]
        );
    }
    #[test]
    fn ports_remain_grouped_with_correct_indices_after_sorting() {
        let port = |name: &str, number, pid| ports::PortInfo {
            name: name.into(),
            port: number,
            proto: "tcp".into(),
            pid: sysinfo::Pid::from_u32(pid),
            internal_port: None,
            exe_path: "-".into(),
            container_id: None,
            group_name: Some("project".into()),
            project_name: None,
        };
        let mut rows = vec![port("z", 80, 10), port("a", 8080, 11)];
        sort_ports(&mut rows, TableSort::new(SortField::Name, SortOrder::Asc));
        let grouped = ports::group_ports(&rows);
        assert!(matches!(grouped[1], ports::PortRow::Item { index: 0 }));
        assert_eq!(rows[0].name, "a");
        sort_ports(&mut rows, TableSort::new(SortField::Port, SortOrder::Desc));
        assert_eq!(rows[0].port, 8080);
    }
    #[test]
    fn native_cpu_and_rss_sort_numerically() {
        let record = |pid, cpu, memory| node::NodeProcessInfo {
            pid: sysinfo::Pid::from_u32(pid),
            name: format!("node-{pid}"),
            script: format!("/app/{pid}.js"),
            project_name: None,
            uses_nvm: false,
            cpu,
            memory_bytes: memory,
            uptime_secs: Some(1),
            pm2: None,
            worker_count: 1,
        };
        let mut rows = vec![record(1, 90.0, 9), record(2, 9.0, 100)];
        sort_node(&mut rows, TableSort::new(SortField::Cpu, SortOrder::Desc));
        assert_eq!(rows[0].pid.as_u32(), 1);
        sort_node(
            &mut rows,
            TableSort::new(SortField::Memory, SortOrder::Desc),
        );
        assert_eq!(rows[0].pid.as_u32(), 2);
    }
    #[test]
    fn docker_sort_orders_project_members_without_changing_container_targets() {
        let container = |id: &str, name: &str, activity| docker::ContainerInfo {
            id: id.into(),
            name: name.into(),
            image: "demo".into(),
            port_public: "-".into(),
            port_internal: "-".into(),
            status: "Up".into(),
            group_name: "project".into(),
            group_path: Some("/app/project".into()),
            running: true,
            memory: None,
            activity_secs: activity,
        };
        let source = vec![
            container("z-id", "z-app", 1),
            container("a-id", "a-app", 20),
            container("unknown-id", "missing", u64::MAX),
        ];
        let (rows, groups) = docker::group_containers_sorted(
            source.clone(),
            Some(TableSort::new(SortField::Name, SortOrder::Asc)),
        );
        assert_eq!(rows[0].id, "a-id");
        assert!(matches!(
            groups[1],
            docker::DockerRow::Item { index: 0, .. }
        ));
        let (rows, _) = docker::group_containers_sorted(
            source,
            Some(TableSort::new(SortField::Activity, SortOrder::Desc)),
        );
        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            ["a-id", "z-id", "unknown-id"]
        );

        let mut measured = container("measured", "measured", 0);
        measured.memory = Some(docker::memory::ContainerMemory {
            used_bytes: 512 * 1024 * 1024,
            limit_bytes: 2 << 30,
            percent: 25.0,
            measured_at: std::time::Instant::now(),
            stale: false,
        });
        let mut larger = measured.clone();
        larger.id = "larger".into();
        larger.memory.as_mut().unwrap().used_bytes = 3 << 29;
        let mut zero = measured.clone();
        zero.id = "zero".into();
        zero.memory.as_mut().unwrap().used_bytes = 0;
        let mut stale = larger.clone();
        stale.id = "stale".into();
        stale.memory.as_mut().unwrap().stale = true;
        let mut stopped = larger.clone();
        stopped.id = "stopped".into();
        stopped.running = false;
        let unknown = container("unknown", "missing", 0);
        for (order, expected) in [
            (SortOrder::Desc, ["larger", "measured", "zero"]),
            (SortOrder::Asc, ["zero", "measured", "larger"]),
        ] {
            let (rows, _) = docker::group_containers_sorted(
                vec![
                    unknown.clone(),
                    stale.clone(),
                    stopped.clone(),
                    measured.clone(),
                    larger.clone(),
                    zero.clone(),
                ],
                Some(TableSort::new(SortField::Memory, order)),
            );
            assert_eq!(
                rows[..3]
                    .iter()
                    .map(|row| row.id.as_str())
                    .collect::<Vec<_>>(),
                expected
            );
            assert!(rows[3..]
                .iter()
                .all(|row| ["stale", "stopped", "unknown"].contains(&row.id.as_str())));
        }
    }
}
