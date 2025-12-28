use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use sysinfo::{Pid, Process, System};

use super::NodeProcessInfo;

/// Detect all Node.js processes running on the system.
/// Filters out IDE, extension, and system processes.
pub fn detect_node_processes(system: &System) -> Vec<NodeProcessInfo> {
    let mut node_procs = Vec::new();

    for (pid, process) in system.processes() {
        if is_node_process(process) {
            let script = extract_script_path(process);

            // Filter out IDE/system processes - only keep project processes
            if !is_project_process(process, &script) {
                continue;
            }

            let node_version = detect_node_version(*pid);
            let project_name = project_name_from_process_with_script(process, &script);

            let uses_nvm = process_uses_nvm(process, &script);

            node_procs.push(NodeProcessInfo {
                pid: *pid,
                name: extract_process_name(process, &script),
                script,
                project_name,
                uses_nvm,
                node_version,
                cpu: process.cpu_usage(),
                memory_bytes: process.memory(),
                uptime_secs: Some(process.run_time()),
                pm2: None,
                worker_count: 1,
            });
        }
    }

    node_procs
}

/// Check if a Node process is a project process (not IDE/system).
fn is_project_process(process: &Process, script: &str) -> bool {
    let cmd = process.cmd();
    let cmd_str = cmd.join(" ").to_lowercase();

    // Get executable path
    let exe_path = process.exe()
        .map(|p| p.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    // ===== EXCLUDE: IDE and Editor processes =====

    // VS Code / Code Server
    if exe_path.contains(".vscode")
        || exe_path.contains(".vscode-server")
        || exe_path.contains("code-server")
        || cmd_str.contains(".vscode")
        || cmd_str.contains("vscode-server")
    {
        return false;
    }

    // Electron-based editors
    if cmd_str.contains("electron") && !cmd_str.contains("electron-") {
        // electron binary itself, not electron-xyz apps
        return false;
    }

    // JetBrains IDEs (WebStorm, etc.)
    if exe_path.contains(".jetbrains")
        || exe_path.contains("/jetbrains/")
        || cmd_str.contains("jetbrains")
    {
        return false;
    }

    // ===== EXCLUDE: Language servers and extensions =====

    // TypeScript server
    if cmd_str.contains("tsserver")
        || cmd_str.contains("typescript-language-server")
        || cmd_str.contains("typescript/lib/tsserver")
    {
        return false;
    }

    // ESLint daemon
    if cmd_str.contains("eslint_d") || cmd_str.contains("eslint-server") {
        return false;
    }

    // Prettier daemon
    if cmd_str.contains("prettierd") || cmd_str.contains("prettier-server") {
        return false;
    }

    // Generic language servers
    if cmd_str.contains("language-server")
        || cmd_str.contains("lsp-server")
        || cmd_str.contains("/lsp/")
    {
        return false;
    }

    // Node IPC (extension communication)
    if cmd_str.contains("--node-ipc") || cmd_str.contains("extensionhost") {
        return false;
    }

    // Claude Code CLI
    if cmd_str.contains("claude-code") || cmd_str.contains("@anthropic") {
        return false;
    }

    // Copilot
    if cmd_str.contains("copilot") || cmd_str.contains("github.copilot") {
        return false;
    }

    // ===== EXCLUDE: System/global installations =====

    // System paths
    if exe_path.starts_with("/usr/lib/")
        || exe_path.starts_with("/usr/share/")
        || exe_path.contains("/snap/")
    {
        // Allow if script is from user directory
        if !script.starts_with("~") && !script.starts_with("/home/") {
            return false;
        }
    }

    // ===== EXCLUDE: Package manager internal processes =====

    // npm/yarn/pnpm internal workers
    if cmd_str.contains("npm-cli.js") && cmd_str.contains("prefix") {
        return false;
    }
    if cmd_str.contains("yarn/") && cmd_str.contains("berry") {
        return false;
    }

    // ===== INCLUDE: Likely project processes =====

    // Has a meaningful script path (not just "-")
    if script != "-" {
        // Script is in a typical project location
        if script.starts_with("~")
            || script.starts_with("/home/")
            || script.starts_with("./")
            || script.contains("/src/")
            || script.contains("/dist/")
            || script.contains("/build/")
            || script.contains("/app/")
            || script.contains("/server/")
            || script.contains("/api/")
        {
            return true;
        }

        // Script has typical project file names
        if script.contains("index.")
            || script.contains("server.")
            || script.contains("app.")
            || script.contains("main.")
            || script.contains("start.")
        {
            return true;
        }
    }

    // Check working directory - if it's a project directory
    let cwd = format!("/proc/{}/cwd", process.pid().as_u32());
    if let Ok(cwd_path) = fs::read_link(&cwd) {
        let cwd_str = cwd_path.to_string_lossy().to_lowercase();

        // Working from a project directory
        if cwd_str.contains("/projects/")
            || cwd_str.contains("/repos/")
            || cwd_str.contains("/workspace/")
            || cwd_str.contains("/work/")
            || cwd_str.contains("/dev/")
            || cwd_str.contains("/src/")
        {
            return true;
        }

        // Check for package.json in cwd (indicates a Node project)
        let pkg_json = cwd_path.join("package.json");
        if pkg_json.exists() {
            return true;
        }
    }

    // If script path is empty/unknown, be conservative - exclude
    if script == "-" {
        return false;
    }

    // Default: include if we have some script info
    true
}

/// Check if a process is a Node.js process.
fn is_node_process(process: &Process) -> bool {
    let name = process.name().to_lowercase();

    // Direct node/bun/deno executables
    if name == "node" || name == "nodejs" || name == "bun" || name == "deno" {
        return true;
    }

    // ts-node, tsx, etc.
    if name == "ts-node" || name == "tsx" || name == "ts-node-esm" {
        return true;
    }

    // Check executable path for node binary
    if let Some(exe) = process.exe() {
        let exe_str = exe.to_string_lossy().to_lowercase();
        if exe_str.contains("/node") || exe_str.contains("/nodejs") {
            return true;
        }
        // Check for nvm/fnm/volta managed node
        if exe_str.contains(".nvm/") || exe_str.contains(".fnm/") || exe_str.contains(".volta/") {
            if exe_str.ends_with("/node") || exe_str.contains("/node/") {
                return true;
            }
        }
    }

    false
}

fn process_uses_nvm(process: &Process, script: &str) -> bool {
    let name = process.name().to_lowercase();
    if name == "nvm" {
        return true;
    }
    let script_lower = script.to_lowercase();
    if script_lower.contains("/.nvm/") || script_lower.contains("/nvm/") {
        return true;
    }
    for arg in process.cmd() {
        let lower = arg.to_lowercase();
        if lower == "nvm" {
            return true;
        }
        if lower.starts_with("nvm ")
            || lower.ends_with(" nvm")
            || lower.contains(" nvm ")
            || lower.ends_with("/nvm")
            || lower.ends_with("\\nvm")
        {
            return true;
        }
    }
    false
}

/// Extract the script path from the process command line.
fn extract_script_path(process: &Process) -> String {
    let cmd = process.cmd();

    if cmd.is_empty() {
        return "-".to_string();
    }

    // Find the first argument that looks like a JS/TS file or script path
    for (i, arg) in cmd.iter().enumerate() {
        // Skip the first argument (node executable) and flags
        if i == 0 {
            continue;
        }

        let arg_str = arg.as_str();

        // Skip node flags
        if arg_str.starts_with('-') {
            continue;
        }

        // Skip common node options that take values
        if i > 0 {
            let prev = cmd[i - 1].as_str();
            if prev == "-e" || prev == "--eval" || prev == "-p" || prev == "--print" {
                continue;
            }
            if prev == "-r" || prev == "--require" || prev == "--import" {
                continue;
            }
        }

        // Check if it looks like a script file or path
        if arg_str.ends_with(".js")
            || arg_str.ends_with(".mjs")
            || arg_str.ends_with(".cjs")
            || arg_str.ends_with(".ts")
            || arg_str.ends_with(".mts")
            || arg_str.ends_with(".tsx")
            || arg_str.ends_with(".jsx")
            || arg_str.contains('/')
            || arg_str.contains('\\')
        {
            // Return a cleaned-up path
            let path = arg_str.to_string();
            // Shorten home directory
            if let Ok(home) = std::env::var("HOME") {
                if path.starts_with(&home) {
                    return path.replacen(&home, "~", 1);
                }
            }
            return path;
        }

        // Could be a package binary (npx, etc.)
        if !arg_str.is_empty() && !arg_str.starts_with('-') {
            return arg_str.to_string();
        }
    }

    // Fallback: try to get from /proc/[pid]/cmdline
    let cmdline_path = format!("/proc/{}/cmdline", process.pid().as_u32());
    if let Ok(cmdline) = fs::read_to_string(&cmdline_path) {
        let parts: Vec<&str> = cmdline.split('\0').collect();
        for part in parts.iter().skip(1) {
            if part.ends_with(".js") || part.ends_with(".ts") || part.contains('/') {
                return part.to_string();
            }
        }
    }

    "-".to_string()
}

/// Extract a meaningful process name.
fn extract_process_name(process: &Process, script: &str) -> String {
    // If we have a script path, use the filename
    if script != "-" {
        let path = Path::new(script);
        if let Some(filename) = path.file_name() {
            let name = filename.to_string_lossy();
            // Remove extension for cleaner display
            if let Some(stem) = Path::new(&*name).file_stem() {
                return stem.to_string_lossy().to_string();
            }
            return name.to_string();
        }
    }

    // Fallback to process name
    process.name().to_string()
}

pub(crate) fn project_name_from_process(process: &Process) -> Option<String> {
    let script = extract_script_path(process);
    project_name_from_process_with_script(process, &script)
}

fn project_name_from_process_with_script(process: &Process, script: &str) -> Option<String> {
    let mut candidates = Vec::new();
    if let Some(script_dir) = script_dir_from_path(script, Some(process)) {
        candidates.push(script_dir);
    }
    if let Some(cwd) = read_process_cwd(process) {
        candidates.push(cwd);
    }
    project_name_from_candidates(&candidates)
}

pub(crate) fn project_name_from_script(script: &str) -> Option<String> {
    let candidates = script_dir_from_path(script, None).into_iter().collect::<Vec<_>>();
    project_name_from_candidates(&candidates)
}

fn project_name_from_candidates(candidates: &[PathBuf]) -> Option<String> {
    for dir in candidates {
        if let Some(name) = project_name_for_dir(dir) {
            return Some(name);
        }
    }
    None
}

fn project_name_for_dir(dir: &Path) -> Option<String> {
    let key = dir.to_string_lossy().into_owned();
    let cache = project_name_cache();
    if let Ok(guard) = cache.lock() {
        if let Some(cached) = guard.get(&key) {
            return cached.clone();
        }
    }

    let name = find_package_name(dir);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(key, name.clone());
    }
    name
}

fn project_name_cache() -> &'static Mutex<HashMap<String, Option<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn package_name_cache() -> &'static Mutex<HashMap<String, Option<String>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn find_package_name(start: &Path) -> Option<String> {
    let mut current = Some(start);
    let mut depth = 0usize;
    while let Some(path) = current {
        let pkg = path.join("package.json");
        if pkg.is_file() {
            if let Some(name) = package_name_from_path(&pkg) {
                return Some(name);
            }
        }
        current = path.parent();
        depth += 1;
        if depth > 15 {
            break;
        }
    }
    None
}

fn package_name_from_path(path: &Path) -> Option<String> {
    let key = path.to_string_lossy().into_owned();
    let cache = package_name_cache();
    if let Ok(guard) = cache.lock() {
        if let Some(cached) = guard.get(&key) {
            return cached.clone();
        }
    }

    let contents = fs::read_to_string(path).ok()?;
    let name = extract_package_name(&contents);
    if let Ok(mut guard) = cache.lock() {
        guard.insert(key, name.clone());
    }
    name
}

fn extract_package_name(contents: &str) -> Option<String> {
    let mut iter = contents.chars().peekable();
    let mut depth = 0usize;

    while let Some(ch) = iter.next() {
        match ch {
            '{' => depth += 1,
            '}' => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            '"' => {
                if depth == 1 {
                    let key = read_json_string(&mut iter);
                    skip_whitespace(&mut iter);
                    if iter.next() != Some(':') {
                        continue;
                    }
                    skip_whitespace(&mut iter);
                    if let Some('"') = iter.peek().copied() {
                        iter.next();
                        let value = read_json_string(&mut iter);
                        if key == "name" {
                            return Some(value);
                        }
                    }
                } else {
                    let _ = read_json_string(&mut iter);
                }
            }
            _ => {}
        }
    }

    None
}

fn read_json_string(iter: &mut std::iter::Peekable<std::str::Chars<'_>>) -> String {
    let mut out = String::new();
    let mut escape = false;
    while let Some(ch) = iter.next() {
        if escape {
            out.push(ch);
            escape = false;
            continue;
        }
        if ch == '\\' {
            escape = true;
            continue;
        }
        if ch == '"' {
            break;
        }
        out.push(ch);
    }
    out
}

fn skip_whitespace(iter: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(ch) = iter.peek() {
        if ch.is_whitespace() {
            iter.next();
        } else {
            break;
        }
    }
}

fn script_dir_from_path(script: &str, process: Option<&Process>) -> Option<PathBuf> {
    if !is_script_path(script) {
        return None;
    }
    let resolved = resolve_script_path(script, process)?;
    let path = if resolved.is_file() {
        resolved.parent().map(|p| p.to_path_buf())?
    } else {
        resolved
    };
    Some(path)
}

fn resolve_script_path(script: &str, process: Option<&Process>) -> Option<PathBuf> {
    let mut path = if script.starts_with('~') {
        let home = std::env::var("HOME").ok()?;
        PathBuf::from(script.replacen('~', &home, 1))
    } else {
        PathBuf::from(script)
    };

    if path.is_absolute() {
        return Some(path);
    }

    if let Some(proc) = process {
        if let Some(cwd) = read_process_cwd(proc) {
            path = cwd.join(path);
            return Some(path);
        }
        return None;
    }

    None
}

fn read_process_cwd(process: &Process) -> Option<PathBuf> {
    let cwd = format!("/proc/{}/cwd", process.pid().as_u32());
    fs::read_link(&cwd).ok()
}

fn is_script_path(script: &str) -> bool {
    let lower = script.to_lowercase();
    if script == "-" {
        return false;
    }
    if script.contains('/') || script.contains('\\') {
        return true;
    }
    lower.ends_with(".js")
        || lower.ends_with(".mjs")
        || lower.ends_with(".cjs")
        || lower.ends_with(".ts")
        || lower.ends_with(".mts")
        || lower.ends_with(".tsx")
        || lower.ends_with(".jsx")
}

/// Try to detect the Node.js version for a process.
fn detect_node_version(pid: Pid) -> Option<String> {
    // Try to read from /proc/[pid]/exe symlink to find the node binary
    let exe_path = format!("/proc/{}/exe", pid.as_u32());

    if let Ok(exe) = fs::read_link(&exe_path) {
        let exe_str = exe.to_string_lossy();

        // Try to extract version from path (common with nvm/fnm/volta)
        // e.g., /home/user/.nvm/versions/node/v20.10.0/bin/node
        if let Some(version) = extract_version_from_path(&exe_str) {
            return Some(version);
        }

        // Try to run node --version (cached per binary path)
        if let Some(version) = get_node_version_cached(&exe_str) {
            return Some(version);
        }
    }

    None
}

/// Extract version from node binary path (nvm/fnm/volta style).
fn extract_version_from_path(path: &str) -> Option<String> {
    // Look for version patterns like v20.10.0 or 20.10.0
    let parts: Vec<&str> = path.split('/').collect();

    for part in parts {
        if part.starts_with('v') && part.len() > 1 {
            let rest = &part[1..];
            if rest.chars().next().map_or(false, |c| c.is_ascii_digit()) {
                // Looks like a version
                return Some(part.to_string());
            }
        }
    }

    None
}

/// Get node version by running node --version (with simple caching).
fn get_node_version_cached(node_path: &str) -> Option<String> {
    use std::sync::Mutex;
    use std::collections::HashMap;
    use std::sync::OnceLock;

    static VERSION_CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();

    let cache = VERSION_CACHE.get_or_init(|| Mutex::new(HashMap::new()));

    // Check cache
    if let Ok(guard) = cache.lock() {
        if let Some(cached) = guard.get(node_path) {
            return cached.clone();
        }
    }

    // Run node --version
    let version = Command::new(node_path)
        .arg("--version")
        .output()
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
            } else {
                None
            }
        });

    // Update cache
    if let Ok(mut guard) = cache.lock() {
        guard.insert(node_path.to_string(), version.clone());
    }

    version
}
