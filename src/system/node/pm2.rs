use std::process::Command;

/// PM2 process information.
#[derive(Clone, Debug, PartialEq)]
pub struct Pm2Process {
    pub pm_id: u32,
    pub name: String,
    pub mode: String,
    pub status: String,
    pub pid: Option<u32>,
    pub cpu: Option<f32>,
    pub memory_bytes: Option<u64>,
    pub uptime_ms: Option<u64>,
    pub script: Option<String>,
    pub cwd: Option<String>,
}

/// Check if PM2 daemon is running.
pub fn is_pm2_running() -> bool {
    // Use bash -lc to ensure PM2 is in PATH (handles nvm, npm global installs, etc.)
    // Use 'pm2 jlist' instead of 'pm2 ping' for more reliable detection
    Command::new("bash")
        .args(["-lc", "pm2 jlist"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Load PM2 process list using `pm2 jlist`.
/// Returns an empty Vec on error (graceful degradation).
pub fn load_pm2_processes() -> Result<Vec<Pm2Process>, Pm2Error> {
    // Try to run pm2 jlist via bash -lc to ensure PM2 is in PATH
    let output = Command::new("bash")
        .args(["-lc", "pm2 jlist"])
        .output()
        .map_err(|e| Pm2Error::CommandFailed(e.to_string()))?;

    if !output.status.success() {
        // PM2 might not be installed or daemon not running
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not found") || stderr.contains("command not found") {
            return Err(Pm2Error::NotInstalled);
        }
        if stderr.contains("PM2 is not running") || stderr.contains("spawn pm2") {
            return Err(Pm2Error::DaemonNotRunning);
        }
        return Err(Pm2Error::CommandFailed(stderr.to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse JSON output
    parse_pm2_json(&stdout)
}

pub fn pm2_start(pm_id: u32) -> std::io::Result<()> {
    let cmd = format!("pm2 start {}", pm_id);
    let output = Command::new("bash")
        .args(["-lc", &cmd])
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("pm2 start failed: {}", stderr.trim()),
        ))
    }
}

pub fn pm2_stop(pm_id: u32) -> std::io::Result<()> {
    let cmd = format!("pm2 stop {}", pm_id);
    let output = Command::new("bash")
        .args(["-lc", &cmd])
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("pm2 stop failed: {}", stderr.trim()),
        ))
    }
}

pub fn pm2_restart(pm_id: u32) -> std::io::Result<()> {
    let cmd = format!("pm2 restart {}", pm_id);
    let output = Command::new("bash")
        .args(["-lc", &cmd])
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("pm2 restart failed: {}", stderr.trim()),
        ))
    }
}

pub fn load_pm2_logs(pm_id: u32) -> std::io::Result<String> {
    let cmd = format!("pm2 logs {} --lines 200 --nostream", pm_id);
    let output = Command::new("bash")
        .args(["-lc", &cmd])
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.success() {
        if stderr.trim().is_empty() {
            Ok(stdout.to_string())
        } else if stdout.trim().is_empty() {
            Ok(stderr.to_string())
        } else {
            Ok(format!("{}\n{}", stdout, stderr))
        }
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("pm2 logs failed: {}", stderr.trim()),
        ))
    }
}

pub fn load_pm2_env(pm_id: u32) -> Result<Vec<String>, Pm2Error> {
    let output = Command::new("bash")
        .args(["-lc", "pm2 jlist"])
        .output()
        .map_err(|e| Pm2Error::CommandFailed(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("not found") || stderr.contains("command not found") {
            return Err(Pm2Error::NotInstalled);
        }
        if stderr.contains("PM2 is not running") || stderr.contains("spawn pm2") {
            return Err(Pm2Error::DaemonNotRunning);
        }
        return Err(Pm2Error::CommandFailed(stderr.to_string()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    let inner = trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .ok_or_else(|| Pm2Error::ParseError("Invalid JSON array".to_string()))?;

    let objects = split_json_objects(inner);
    for obj in objects {
        if extract_u32(&obj, "pm_id") == Some(pm_id) {
            if let Some(env_obj) = extract_nested_object(&obj, "pm2_env", "env") {
                let envs = parse_env_object(&env_obj);
                return Ok(envs);
            }
            return Ok(Vec::new());
        }
    }

    Err(Pm2Error::ParseError("PM2 process not found".to_string()))
}

/// Errors that can occur when interacting with PM2.
#[derive(Debug)]
pub enum Pm2Error {
    NotInstalled,
    DaemonNotRunning,
    CommandFailed(String),
    ParseError(String),
}

impl std::fmt::Display for Pm2Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Pm2Error::NotInstalled => write!(f, "PM2 is not installed"),
            Pm2Error::DaemonNotRunning => write!(f, "PM2 daemon is not running"),
            Pm2Error::CommandFailed(msg) => write!(f, "PM2 command failed: {}", msg),
            Pm2Error::ParseError(msg) => write!(f, "Failed to parse PM2 output: {}", msg),
        }
    }
}

/// Parse PM2 JSON output manually (avoiding external JSON crate dependency).
fn parse_pm2_json(json_str: &str) -> Result<Vec<Pm2Process>, Pm2Error> {
    let trimmed = json_str.trim();

    // Handle empty array
    if trimmed == "[]" {
        return Ok(Vec::new());
    }

    // Very basic JSON array parsing
    // PM2 jlist output format:
    // [{"pm_id":0,"name":"app","pid":1234,"monit":{"memory":123456,"cpu":5.5},...},...]

    let mut processes = Vec::new();

    // Remove outer brackets
    let inner = trimmed
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .ok_or_else(|| Pm2Error::ParseError("Invalid JSON array".to_string()))?;

    if inner.trim().is_empty() {
        return Ok(processes);
    }

    // Split by },{ pattern (accounting for nested objects)
    let objects = split_json_objects(inner);

    for obj_str in objects {
        if let Some(proc) = parse_pm2_object(&obj_str) {
            processes.push(proc);
        }
    }

    Ok(processes)
}

/// Split JSON array into individual object strings.
fn split_json_objects(json: &str) -> Vec<String> {
    let mut objects = Vec::new();
    let mut current = String::new();
    let mut brace_depth = 0;
    let mut in_string = false;
    let mut escape_next = false;

    for ch in json.chars() {
        if escape_next {
            current.push(ch);
            escape_next = false;
            continue;
        }

        if ch == '\\' && in_string {
            current.push(ch);
            escape_next = true;
            continue;
        }

        if ch == '"' {
            in_string = !in_string;
        }

        if !in_string {
            match ch {
                '{' => brace_depth += 1,
                '}' => brace_depth -= 1,
                ',' if brace_depth == 0 => {
                    if !current.trim().is_empty() {
                        objects.push(current.trim().to_string());
                    }
                    current = String::new();
                    continue;
                }
                _ => {}
            }
        }

        current.push(ch);
    }

    if !current.trim().is_empty() {
        objects.push(current.trim().to_string());
    }

    objects
}

/// Parse a single PM2 process object.
fn parse_pm2_object(json: &str) -> Option<Pm2Process> {
    // Extract fields using simple string matching

    let pm_id = extract_u32(json, "pm_id")?;
    let name = extract_string(json, "name").unwrap_or_else(|| "unknown".to_string());
    let pid = extract_u32(json, "pid");

    // Status is in pm2_env.status
    let status = extract_nested_string(json, "pm2_env", "status")
        .or_else(|| extract_string(json, "status"))
        .unwrap_or_else(|| "unknown".to_string());

    // Mode (fork/cluster) is in pm2_env.exec_mode
    let mode = extract_nested_string(json, "pm2_env", "exec_mode")
        .map(|m| {
            if m.contains("cluster") {
                "cluster".to_string()
            } else {
                "fork".to_string()
            }
        })
        .unwrap_or_else(|| "fork".to_string());

    // Uptime in pm2_env.pm_uptime (timestamp when started)
    let uptime_ms = extract_nested_u64(json, "pm2_env", "pm_uptime").and_then(|start_time| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_millis() as u64;
        if now > start_time {
            Some(now - start_time)
        } else {
            None
        }
    });

    // Memory and CPU from monit object
    let memory_bytes = extract_nested_u64(json, "monit", "memory");
    let cpu = extract_nested_f32(json, "monit", "cpu");

    // Script path and cwd
    let script = extract_nested_string(json, "pm2_env", "pm_exec_path");
    let cwd = extract_nested_string(json, "pm2_env", "pm_cwd");

    Some(Pm2Process {
        pm_id,
        name,
        mode,
        status,
        pid,
        cpu,
        memory_bytes,
        uptime_ms,
        script,
        cwd,
    })
}

/// Extract a string value from JSON.
fn extract_string(json: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{}\":\"", key);
    let start = json.find(&pattern)? + pattern.len();
    let rest = &json[start..];

    // Find the closing quote (handling escapes)
    let mut end = 0;
    let mut escape_next = false;
    for (i, ch) in rest.chars().enumerate() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if ch == '\\' {
            escape_next = true;
            continue;
        }
        if ch == '"' {
            end = i;
            break;
        }
    }

    Some(rest[..end].to_string())
}

/// Extract a u32 value from JSON.
fn extract_u32(json: &str, key: &str) -> Option<u32> {
    let pattern = format!("\"{}\":", key);
    let start = json.find(&pattern)? + pattern.len();
    let rest = json[start..].trim_start();

    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());

    if end == 0 {
        return None;
    }

    rest[..end].parse().ok()
}

/// Extract a u64 value from JSON.
fn extract_u64(json: &str, key: &str) -> Option<u64> {
    let pattern = format!("\"{}\":", key);
    let start = json.find(&pattern)? + pattern.len();
    let rest = json[start..].trim_start();

    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());

    if end == 0 {
        return None;
    }

    rest[..end].parse().ok()
}

/// Extract a f32 value from JSON.
fn extract_f32(json: &str, key: &str) -> Option<f32> {
    let pattern = format!("\"{}\":", key);
    let start = json.find(&pattern)? + pattern.len();
    let rest = json[start..].trim_start();

    let end = rest
        .find(|c: char| !c.is_ascii_digit() && c != '.' && c != '-')
        .unwrap_or(rest.len());

    if end == 0 {
        return None;
    }

    rest[..end].parse().ok()
}

/// Extract a nested string value from JSON (e.g., "pm2_env": {"status": "online"}).
fn extract_nested_string(json: &str, parent: &str, key: &str) -> Option<String> {
    let parent_pattern = format!("\"{}\":{{", parent);

    // Also try with space: "parent": {
    let alt_pattern = format!("\"{}\" : {{", parent);

    let parent_start = json
        .find(&parent_pattern)
        .or_else(|| json.find(&alt_pattern))?;

    let parent_content = &json[parent_start..];

    // Find matching brace
    let mut brace_depth = 0;
    let mut started = false;
    let mut end = parent_content.len();

    for (i, ch) in parent_content.chars().enumerate() {
        if ch == '{' {
            brace_depth += 1;
            started = true;
        } else if ch == '}' {
            brace_depth -= 1;
            if started && brace_depth == 0 {
                end = i + 1;
                break;
            }
        }
    }

    let nested = &parent_content[..end];
    extract_string(nested, key)
}

/// Extract a nested u64 value from JSON.
fn extract_nested_u64(json: &str, parent: &str, key: &str) -> Option<u64> {
    let parent_pattern = format!("\"{}\":{{", parent);
    let alt_pattern = format!("\"{}\" : {{", parent);

    let parent_start = json
        .find(&parent_pattern)
        .or_else(|| json.find(&alt_pattern))?;

    let parent_content = &json[parent_start..];

    let mut brace_depth = 0;
    let mut started = false;
    let mut end = parent_content.len();

    for (i, ch) in parent_content.chars().enumerate() {
        if ch == '{' {
            brace_depth += 1;
            started = true;
        } else if ch == '}' {
            brace_depth -= 1;
            if started && brace_depth == 0 {
                end = i + 1;
                break;
            }
        }
    }

    let nested = &parent_content[..end];
    extract_u64(nested, key)
}

/// Extract a nested f32 value from JSON.
fn extract_nested_f32(json: &str, parent: &str, key: &str) -> Option<f32> {
    let parent_pattern = format!("\"{}\":{{", parent);
    let alt_pattern = format!("\"{}\" : {{", parent);

    let parent_start = json
        .find(&parent_pattern)
        .or_else(|| json.find(&alt_pattern))?;

    let parent_content = &json[parent_start..];

    let mut brace_depth = 0;
    let mut started = false;
    let mut end = parent_content.len();

    for (i, ch) in parent_content.chars().enumerate() {
        if ch == '{' {
            brace_depth += 1;
            started = true;
        } else if ch == '}' {
            brace_depth -= 1;
            if started && brace_depth == 0 {
                end = i + 1;
                break;
            }
        }
    }

    let nested = &parent_content[..end];
    extract_f32(nested, key)
}

fn extract_object(json: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{}\":{{", key);
    let alt_pattern = format!("\"{}\" : {{", key);
    let start = json
        .find(&pattern)
        .or_else(|| json.find(&alt_pattern))?;
    let brace_index = json[start..].find('{')? + start;
    find_object_slice(json, brace_index)
}

fn extract_nested_object(json: &str, parent: &str, key: &str) -> Option<String> {
    let parent_obj = extract_object(json, parent)?;
    extract_object(&parent_obj, key)
}

fn find_object_slice(text: &str, start: usize) -> Option<String> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape_next = false;

    for (offset, ch) in text[start..].char_indices() {
        if in_string {
            if escape_next {
                escape_next = false;
            } else if ch == '\\' {
                escape_next = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    let end = start + offset + 1;
                    return Some(text[start..end].to_string());
                }
            }
            _ => {}
        }
    }

    None
}

fn parse_env_object(raw: &str) -> Vec<String> {
    let mut envs = Vec::new();
    let trimmed = raw.trim();
    let inner = trimmed
        .strip_prefix('{')
        .and_then(|s| s.strip_suffix('}'))
        .unwrap_or(trimmed);
    let mut iter = inner.chars().peekable();

    loop {
        // Skip whitespace and commas
        while matches!(iter.peek(), Some(ch) if ch.is_whitespace() || *ch == ',') {
            iter.next();
        }
        match iter.peek() {
            None => break,
            Some('}') => {
                iter.next();
                break;
            }
            _ => {}
        }

        let key = match parse_json_string(&mut iter) {
            Some(key) => key,
            None => break,
        };

        while matches!(iter.peek(), Some(ch) if ch.is_whitespace()) {
            iter.next();
        }
        if iter.next() != Some(':') {
            break;
        }
        while matches!(iter.peek(), Some(ch) if ch.is_whitespace()) {
            iter.next();
        }

        let value = parse_json_value(&mut iter).unwrap_or_default();
        envs.push(format!("{}={}", key, value));
    }

    envs.sort();
    envs
}

fn parse_json_string(iter: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<String> {
    if iter.next() != Some('"') {
        return None;
    }
    let mut out = String::new();
    let mut escape_next = false;
    while let Some(ch) = iter.next() {
        if escape_next {
            match ch {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                'u' => {
                    let mut hex = String::new();
                    for _ in 0..4 {
                        if let Some(h) = iter.next() {
                            hex.push(h);
                        } else {
                            break;
                        }
                    }
                    if let Ok(code) = u16::from_str_radix(&hex, 16) {
                        if let Some(c) = char::from_u32(code as u32) {
                            out.push(c);
                        }
                    }
                }
                _ => out.push(ch),
            }
            escape_next = false;
            continue;
        }
        if ch == '\\' {
            escape_next = true;
            continue;
        }
        if ch == '"' {
            return Some(out);
        }
        out.push(ch);
    }
    None
}

fn parse_json_value(
    iter: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Option<String> {
    match iter.peek().copied() {
        Some('"') => parse_json_string(iter),
        Some('{') => {
            let mut depth = 0i32;
            let mut in_string = false;
            let mut escape_next = false;
            let mut out = String::new();
            while let Some(ch) = iter.next() {
                out.push(ch);
                if in_string {
                    if escape_next {
                        escape_next = false;
                    } else if ch == '\\' {
                        escape_next = true;
                    } else if ch == '"' {
                        in_string = false;
                    }
                    continue;
                }
                match ch {
                    '"' => in_string = true,
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            Some(out)
        }
        Some('[') => {
            let mut depth = 0i32;
            let mut in_string = false;
            let mut escape_next = false;
            let mut out = String::new();
            while let Some(ch) = iter.next() {
                out.push(ch);
                if in_string {
                    if escape_next {
                        escape_next = false;
                    } else if ch == '\\' {
                        escape_next = true;
                    } else if ch == '"' {
                        in_string = false;
                    }
                    continue;
                }
                match ch {
                    '"' => in_string = true,
                    '[' => depth += 1,
                    ']' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
            }
            Some(out)
        }
        Some(_) => {
            let mut out = String::new();
            while let Some(ch) = iter.peek().copied() {
                if ch == ',' || ch == '}' {
                    break;
                }
                out.push(ch);
                iter.next();
            }
            Some(out.trim().to_string())
        }
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_empty_array() {
        let result = parse_pm2_json("[]").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_extract_string() {
        let json = r#"{"name":"my-app","version":"1.0"}"#;
        assert_eq!(extract_string(json, "name"), Some("my-app".to_string()));
        assert_eq!(extract_string(json, "version"), Some("1.0".to_string()));
    }

    #[test]
    fn test_extract_u32() {
        let json = r#"{"pm_id":5,"pid":1234}"#;
        assert_eq!(extract_u32(json, "pm_id"), Some(5));
        assert_eq!(extract_u32(json, "pid"), Some(1234));
    }
}
