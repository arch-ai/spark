use std::process::Command;

/// PM2 process information.
#[derive(Clone, Debug)]
pub struct Pm2Process {
    pub pm_id: u32,
    pub name: String,
    pub mode: String,
    pub status: String,
    pub pid: Option<u32>,
    pub cpu: Option<f32>,
    pub memory_bytes: Option<u64>,
    pub restarts: u32,
    pub uptime_ms: Option<u64>,
    pub script: Option<String>,
}

/// Check if PM2 daemon is running.
pub fn is_pm2_running() -> bool {
    Command::new("pm2")
        .args(["ping"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Load PM2 process list using `pm2 jlist`.
/// Returns an empty Vec on error (graceful degradation).
pub fn load_pm2_processes() -> Result<Vec<Pm2Process>, Pm2Error> {
    // Try to run pm2 jlist
    let output = Command::new("pm2")
        .args(["jlist"])
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

    // Restarts count
    let restarts = extract_nested_u32(json, "pm2_env", "restart_time").unwrap_or(0);

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

    // Script path
    let script = extract_nested_string(json, "pm2_env", "pm_exec_path");

    Some(Pm2Process {
        pm_id,
        name,
        mode,
        status,
        pid,
        cpu,
        memory_bytes,
        restarts,
        uptime_ms,
        script,
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

/// Extract a nested u32 value from JSON.
fn extract_nested_u32(json: &str, parent: &str, key: &str) -> Option<u32> {
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
    extract_u32(nested, key)
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
