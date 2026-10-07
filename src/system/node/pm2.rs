use crate::system::command::run_output;
use serde_json::Value;
use std::io;
use std::process::{Command, Output};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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

// Use the inherited PATH first (including test doubles). A login shell is only
// needed when an interactive shell profile supplies an nvm/global npm PATH.
fn pm2_output(args: &[&str], timeout: Duration) -> io::Result<Output> {
    match run_output(Command::new("pm2").args(args), timeout, "PM2") {
        Err(err) if err.kind() == io::ErrorKind::NotFound => run_output(
            Command::new("bash")
                .args(["-lc", "exec pm2 \"$@\"", "spark-pm2"])
                .args(args),
            timeout,
            "PM2",
        ),
        result => result,
    }
}

fn load_pm2_json() -> Result<Value, Pm2Error> {
    let output = pm2_output(&["jlist", "--silent"], Duration::from_secs(15))
        .map_err(|err| Pm2Error::CommandFailed(err.to_string()))?;
    if !output.status.success() {
        let message = command_error(&output);
        return Err(if output.status.code() == Some(127) {
            Pm2Error::NotInstalled
        } else if message.contains("PM2 is not running") {
            Pm2Error::DaemonNotRunning
        } else {
            Pm2Error::CommandFailed(message)
        });
    }
    parse_array(&String::from_utf8_lossy(&output.stdout))
}

fn parse_array(text: &str) -> Result<Value, Pm2Error> {
    let json: Value =
        serde_json::from_str(text).map_err(|err| Pm2Error::ParseError(err.to_string()))?;
    if !json.is_array() {
        return Err(Pm2Error::ParseError("Expected a JSON array".into()));
    }
    Ok(json)
}

pub fn load_pm2_processes() -> Result<Vec<Pm2Process>, Pm2Error> {
    parse_processes(&load_pm2_json()?)
}

fn command_error(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stderr.trim().is_empty() {
        stderr.trim().into()
    } else if !stdout.trim().is_empty() {
        stdout.trim().into()
    } else {
        format!("Command exited with {}", output.status)
    }
}

fn mutate(action: &str, pm_id: u32) -> io::Result<()> {
    let id = pm_id.to_string();
    let output = pm2_output(&[action, &id], Duration::from_secs(120))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "pm2 {action} failed: {}",
            command_error(&output)
        )))
    }
}

pub fn pm2_start(pm_id: u32) -> io::Result<()> {
    mutate("start", pm_id)
}
pub fn pm2_stop(pm_id: u32) -> io::Result<()> {
    mutate("stop", pm_id)
}
pub fn pm2_restart(pm_id: u32) -> io::Result<()> {
    mutate("restart", pm_id)
}

pub fn load_pm2_logs(pm_id: u32) -> io::Result<String> {
    let id = pm_id.to_string();
    let output = pm2_output(
        &["logs", &id, "--lines", "200", "--nostream"],
        Duration::from_secs(15),
    )?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "pm2 logs failed: {}",
            command_error(&output)
        )));
    }
    Ok(format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    ))
}

pub fn load_pm2_env(pm_id: u32) -> Result<Vec<String>, Pm2Error> {
    parse_env(&load_pm2_json()?, pm_id)
}

fn parse_env(json: &Value, pm_id: u32) -> Result<Vec<String>, Pm2Error> {
    let process = json
        .as_array()
        .unwrap()
        .iter()
        .find(|process| process.get("pm_id").and_then(Value::as_u64) == Some(pm_id as u64))
        .ok_or_else(|| Pm2Error::ParseError("PM2 process no longer exists".into()))?;
    let mut env = process["pm2_env"]["env"]
        .as_object()
        .map(|env| {
            env.iter()
                .map(|(key, value)| {
                    format!(
                        "{key}={}",
                        value
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| value.to_string())
                    )
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    env.sort();
    Ok(env)
}

/// Errors that can occur when interacting with PM2.
#[derive(Debug)]
pub enum Pm2Error {
    NotInstalled,
    DaemonNotRunning,
    CommandFailed(String),
    ParseError(String),
}

impl std::error::Error for Pm2Error {}

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

fn parse_processes(json: &Value) -> Result<Vec<Pm2Process>, Pm2Error> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    json.as_array()
        .unwrap()
        .iter()
        .map(|process| {
            let pm_id = process
                .get("pm_id")
                .and_then(Value::as_u64)
                .and_then(|id| u32::try_from(id).ok())
                .ok_or_else(|| Pm2Error::ParseError("Process is missing a valid PM2 id".into()))?;
            let env = &process["pm2_env"];
            let status = env["status"].as_str().unwrap_or("unknown").to_owned();
            let pid = process["pid"]
                .as_u64()
                .and_then(|pid| u32::try_from(pid).ok())
                .filter(|pid| *pid != 0);
            Ok(Pm2Process {
                pm_id,
                name: process["name"].as_str().unwrap_or("unknown").to_owned(),
                mode: if env["exec_mode"].as_str().unwrap_or("").contains("cluster") {
                    "cluster"
                } else {
                    "fork"
                }
                .into(),
                uptime_ms: if status == "online" {
                    env["pm_uptime"]
                        .as_u64()
                        .filter(|start| *start > 0)
                        .and_then(|start| now.checked_sub(start))
                } else {
                    None
                },
                status,
                pid,
                cpu: process["monit"]["cpu"]
                    .as_f64()
                    .map(|value| value as f32)
                    .filter(|value| value.is_finite() && *value >= 0.0),
                memory_bytes: process["monit"]["memory"].as_u64(),
                script: env["pm_exec_path"].as_str().map(str::to_owned),
                cwd: env["pm_cwd"].as_str().map(str::to_owned),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn json_handles_whitespace_unicode_and_nested_duplicate_fields() {
        let json = parse_array(r#"[
            {"pm2_env": {"env": {"name": "wrong", "pm_id": 99, "BRACES": "{value}", "UNICODE": "\uD83D\uDE80"},
                "status": "online", "exec_mode": "cluster_mode", "pm_exec_path": "/tmp/项目/app.js"},
             "pm_id": 7, "name": "café \"worker\"", "pid": 123,
             "monit": {"cpu": 1.5, "memory": 12345}}
        ]"#).unwrap();
        let rows = parse_processes(&json).unwrap();
        assert_eq!(rows[0].pm_id, 7);
        assert_eq!(rows[0].name, "café \"worker\"");
        assert_eq!(rows[0].script.as_deref(), Some("/tmp/项目/app.js"));
        assert_eq!(rows[0].memory_bytes, Some(12345));
        assert!(parse_env(&json, 7)
            .unwrap()
            .contains(&"UNICODE=🚀".to_string()));
        assert!(parse_env(&json, 99).is_err());
    }
    #[test]
    fn invalid_output_is_an_error_and_stopped_processes_have_no_pid_or_uptime() {
        for raw in ["[broken]", "{}", "[{}]", "[{\"pm_id\": 4294967296}]"] {
            assert!(parse_array(raw)
                .and_then(|json| parse_processes(&json))
                .is_err());
        }
        let json =
            parse_array(r#"[{"pm_id":0,"pid":0,"pm2_env":{"status":"stopped","pm_uptime":1}}]"#)
                .unwrap();
        let rows = parse_processes(&json).unwrap();
        assert!(rows[0].pid.is_none());
        assert!(rows[0].uptime_ms.is_none());
    }
}
