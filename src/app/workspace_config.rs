//! Versioned workspace preferences. Script paths are data, never inferred commands.
use super::sorting::{SortTarget, TableSort};
use super::{AppState, NodeTab, ViewMode};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectConfig {
    pub name: String,
    pub path: String,
    pub start_dev: String,
    pub stop_dev: String,
    pub start_prod: String,
    pub stop_prod: String,
}
#[derive(Clone, Debug)]
pub struct SavedFilter {
    pub name: String,
    pub view: ViewMode,
    pub pm2: bool,
    pub filter: String,
}
#[derive(Default)]
pub struct Preferences {
    pub projects: Vec<ProjectConfig>,
    pub favorites: BTreeSet<String>,
    pub filters: Vec<SavedFilter>,
}

pub fn config_path() -> PathBuf {
    if let Some(path) = std::env::var_os("SPARK_CONFIG_DIR") {
        return PathBuf::from(path).join("workspace.json");
    }
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    root.join("spark/workspace.json")
}

pub fn view_key(view: ViewMode) -> &'static str {
    match view {
        ViewMode::Process => "process",
        ViewMode::Docker | ViewMode::DockerEnv => "docker",
        ViewMode::Ports => "ports",
        ViewMode::Node => "node",
        ViewMode::Projects => "projects",
    }
}
pub fn parse_view(key: &str) -> ViewMode {
    match key {
        "docker" => ViewMode::Docker,
        "ports" => ViewMode::Ports,
        "node" => ViewMode::Node,
        "projects" => ViewMode::Projects,
        _ => ViewMode::Process,
    }
}
const TARGETS: [SortTarget; 9] = [
    SortTarget::Process,
    SortTarget::Docker,
    SortTarget::Ports,
    SortTarget::Node,
    SortTarget::Pm2,
    SortTarget::Images,
    SortTarget::Containers,
    SortTarget::Volumes,
    SortTarget::Projects,
];

pub fn encode(state: &AppState) -> Value {
    let prefs = &state.workspace.preferences;
    json!({"version": 1, "view": view_key(state.view_mode), "pm2": state.node_tab == NodeTab::Pm2,
        "sorts": TARGETS.iter().map(|target| { let sort = state.sort_for(*target);
            json!({"target": format!("{target:?}"), "field": format!("{:?}", sort.field), "desc": sort.order == super::SortOrder::Desc}) }).collect::<Vec<_>>(),
        "filters": {"process":state.process_filter,"docker":state.docker_filter,"ports":state.ports_filter,"node":state.node_filter,"projects":state.workspace.filter},
        "favorites": prefs.favorites,
        "saved_filters": prefs.filters.iter().map(|f|json!({"name":f.name,"view":view_key(f.view),"pm2":f.pm2,"filter":f.filter})).collect::<Vec<_>>(),
        "projects": prefs.projects.iter().map(|p|json!({"name":p.name,"path":p.path,"start_dev":p.start_dev,"stop_dev":p.stop_dev,"start_prod":p.start_prod,"stop_prod":p.stop_prod})).collect::<Vec<_>>() })
}

fn string(value: &Value, key: &str) -> String {
    value[key]
        .as_str()
        .unwrap_or_default()
        .chars()
        .take(4096)
        .collect()
}

pub fn load(state: &mut AppState, path: &Path) -> io::Result<()> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    if bytes.len() > 1024 * 1024 {
        return Err(io::Error::other("Workspace exceeds 1 MiB"));
    }
    let value: Value = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
    if value["version"].as_u64() != Some(1) {
        return Err(io::Error::other(
            "Unsupported workspace version; existing file preserved",
        ));
    }
    state.process_filter = string(&value["filters"], "process");
    state.docker_filter = string(&value["filters"], "docker");
    state.ports_filter = string(&value["filters"], "ports");
    state.node_filter = string(&value["filters"], "node");
    state.workspace.filter = string(&value["filters"], "projects");
    if let Some(sorts) = value["sorts"].as_array() {
        for entry in sorts.iter().take(9) {
            if let Some(target) = TARGETS
                .iter()
                .find(|t| format!("{t:?}") == string(entry, "target"))
            {
                if let Some(field) = target
                    .fields()
                    .iter()
                    .find(|f| format!("{f:?}") == string(entry, "field"))
                {
                    state.apply_sort(
                        *target,
                        TableSort::new(
                            *field,
                            if entry["desc"].as_bool() == Some(true) {
                                super::SortOrder::Desc
                            } else {
                                super::SortOrder::Asc
                            },
                        ),
                    );
                }
            }
        }
    }
    let mut preferences = Preferences::default();
    if let Some(favorites) = value["favorites"].as_array() {
        preferences.favorites = favorites
            .iter()
            .take(512)
            .filter_map(|f| f.as_str().map(str::to_string))
            .collect();
    }
    if let Some(filters) = value["saved_filters"].as_array() {
        preferences.filters = filters
            .iter()
            .take(128)
            .map(|f| SavedFilter {
                name: string(f, "name"),
                view: parse_view(&string(f, "view")),
                pm2: f["pm2"].as_bool().unwrap_or(false),
                filter: string(f, "filter"),
            })
            .collect();
    }
    if let Some(projects) = value["projects"].as_array() {
        preferences.projects = projects
            .iter()
            .take(512)
            .map(|p| ProjectConfig {
                name: string(p, "name"),
                path: string(p, "path"),
                start_dev: string(p, "start_dev"),
                stop_dev: string(p, "stop_dev"),
                start_prod: string(p, "start_prod"),
                stop_prod: string(p, "stop_prod"),
            })
            .filter(|p| Path::new(&p.path).is_absolute())
            .collect();
    }
    state.workspace.preferences = preferences;
    state.set_view(parse_view(&string(&value, "view")));
    if state.view_mode == ViewMode::Node && value["pm2"].as_bool() == Some(true) {
        state.set_node_tab(NodeTab::Pm2);
    }
    Ok(())
}

pub fn save(path: &Path, value: &Value) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("Workspace has no parent directory"))?;
    std::fs::create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".workspace-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(
            serde_json::to_string_pretty(value)
                .map_err(io::Error::other)?
                .as_bytes(),
        )?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        std::fs::rename(&temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp);
    }
    result
}

pub fn script_path(config: &ProjectConfig, script: &str) -> io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    if script.is_empty() || script.contains('\0') {
        return Err(io::Error::other(
            "Configure both the selected start script and the opposite stop script",
        ));
    }
    let path = Path::new(&config.path).join(script);
    let metadata = std::fs::metadata(&path)?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
        return Err(io::Error::other(format!(
            "{} must be an executable file",
            path.display()
        )));
    }
    Ok(path)
}

pub fn fields(config: &ProjectConfig) -> [&str; 6] {
    [
        &config.name,
        &config.path,
        &config.start_dev,
        &config.stop_dev,
        &config.start_prod,
        &config.stop_prod,
    ]
}
pub fn field_mut(config: &mut ProjectConfig, index: usize) -> &mut String {
    match index {
        0 => &mut config.name,
        1 => &mut config.path,
        2 => &mut config.start_dev,
        3 => &mut config.stop_dev,
        4 => &mut config.start_prod,
        _ => &mut config.stop_prod,
    }
}
