//! Exact, reviewable volume cleanup. Reference changes invalidate the preview.
use super::docker::command::DockerCommand;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::process::Command;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AffectedContainer {
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: String,
    pub project: String,
    pub retained_volumes: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CleanupPlan {
    pub volumes: Vec<String>,
    pub references: BTreeMap<String, BTreeSet<String>>,
    pub containers: Vec<AffectedContainer>,
}
fn docker(args: &[&str]) -> io::Result<String> {
    let output = Command::new("docker").args(args).docker_output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
pub fn preview(names: &[String]) -> io::Result<CleanupPlan> {
    preview_with(names, docker)
}
fn references(
    names: &[String],
    run: &mut impl FnMut(&[&str]) -> io::Result<String>,
) -> io::Result<BTreeMap<String, BTreeSet<String>>> {
    let mut refs = BTreeMap::new();
    for name in names {
        if name.is_empty() || name.starts_with('-') {
            return Err(io::Error::other("Invalid volume name"));
        }
        let raw = run(&[
            "ps",
            "-a",
            "--no-trunc",
            "--filter",
            &format!("volume={name}"),
            "--format",
            "{{.ID}}",
        ])?;
        refs.insert(
            name.clone(),
            raw.lines()
                .filter(|id| !id.trim().is_empty())
                .map(|id| id.trim().to_string())
                .collect(),
        );
    }
    Ok(refs)
}
fn preview_with(
    names: &[String],
    mut run: impl FnMut(&[&str]) -> io::Result<String>,
) -> io::Result<CleanupPlan> {
    let volumes: Vec<_> = names
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if volumes.is_empty() {
        return Err(io::Error::other("Select at least one volume"));
    }
    let references = references(&volumes, &mut run)?;
    let ids: BTreeSet<_> = references
        .values()
        .flat_map(|ids| ids.iter().cloned())
        .collect();
    let mut containers = Vec::new();
    for id in ids {
        let raw = run(&["inspect", "--", &id])?;
        let json: Value = serde_json::from_str(&raw).map_err(io::Error::other)?;
        let entry = json
            .as_array()
            .and_then(|v| v.first())
            .ok_or_else(|| io::Error::other("Container inspection missing"))?;
        let actual = entry["Id"]
            .as_str()
            .ok_or_else(|| io::Error::other("Container ID missing"))?;
        if actual != id {
            return Err(io::Error::other(
                "Container identity changed; refresh storage",
            ));
        }
        let mounts = entry["Mounts"]
            .as_array()
            .ok_or_else(|| io::Error::other("Mount inspection unavailable"))?;
        for (name, ids) in &references {
            if ids.contains(&id)
                && !mounts.iter().any(|mount| {
                    mount["Type"].as_str() == Some("volume") && mount["Name"].as_str() == Some(name)
                })
            {
                return Err(io::Error::other("Volume filter did not identify an exact named-volume mount; nothing was deleted"));
            }
        }
        let retained_volumes = entry["Mounts"]
            .as_array()
            .ok_or_else(|| io::Error::other("Mount inspection unavailable"))?
            .iter()
            .filter(|m| m["Type"].as_str() == Some("volume"))
            .filter_map(|m| m["Name"].as_str())
            .filter(|name| !volumes.iter().any(|v| v == name))
            .map(str::to_string)
            .collect();
        containers.push(AffectedContainer {
            id,
            name: entry["Name"]
                .as_str()
                .unwrap_or(actual)
                .trim_start_matches('/')
                .into(),
            image: entry["Config"]["Image"]
                .as_str()
                .unwrap_or("Unknown")
                .into(),
            status: entry["State"]["Status"]
                .as_str()
                .unwrap_or("Unknown")
                .into(),
            project: entry["Config"]["Labels"]["com.docker.compose.project"]
                .as_str()
                .unwrap_or("Unassigned")
                .into(),
            retained_volumes,
        });
    }
    Ok(CleanupPlan {
        volumes,
        references,
        containers,
    })
}
pub fn execute(plan: &CleanupPlan) -> io::Result<String> {
    execute_with(plan, docker)
}
fn execute_with(
    plan: &CleanupPlan,
    mut run: impl FnMut(&[&str]) -> io::Result<String>,
) -> io::Result<String> {
    // Validate the complete batch before deleting its first container.
    let current = references(&plan.volumes, &mut run)?;
    if current != plan.references {
        return Err(io::Error::other("Volume references changed since review. Nothing was deleted; refresh and review again."));
    }
    let mut completed = Vec::new();
    let result = (|| {
        for container in &plan.containers {
            run(&["rm", "-f", "--", &container.id])?;
            completed.push(format!(
                "Removed container {} ({})",
                container.name, container.id
            ));
        }
        for name in &plan.volumes {
            run(&["volume", "rm", "--", name])?;
            completed.push(format!("Removed volume {name}"));
        }
        Ok(completed.join("\n"))
    })();
    result.map_err(|error:io::Error|io::Error::new(error.kind(),format!("Cleanup stopped: {error}\nConfirmed removals:\n{}\nRefresh storage before retrying.",if completed.is_empty(){"None".into()}else{completed.join("\n")})))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn plan() -> CleanupPlan {
        CleanupPlan {
            volumes: vec!["data".into(), "cache".into()],
            references: BTreeMap::from([
                ("data".into(), BTreeSet::from(["abc".into()])),
                ("cache".into(), BTreeSet::from(["abc".into()])),
            ]),
            containers: vec![AffectedContainer {
                id: "abc".into(),
                name: "api".into(),
                image: "api:v1".into(),
                status: "running".into(),
                project: "app".into(),
                retained_volumes: vec!["keep".into()],
            }],
        }
    }
    #[test]
    fn changed_references_block_every_mutation() {
        let mut mutations = 0;
        let error = execute_with(&plan(), |args| {
            if args[0] == "ps" {
                Ok("new-owner\n".into())
            } else {
                mutations += 1;
                Ok(String::new())
            }
        })
        .unwrap_err();
        assert_eq!(mutations, 0);
        assert!(error.to_string().contains("Nothing was deleted"));
    }
    #[test]
    fn shared_owner_is_removed_once_and_other_volumes_survive() {
        let mut calls = Vec::new();
        execute_with(&plan(), |args| {
            calls.push(args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
            Ok(if args[0] == "ps" {
                "abc\n".into()
            } else {
                String::new()
            })
        })
        .unwrap();
        let mutations: Vec<_> = calls.iter().filter(|a| a[0] != "ps").collect();
        assert_eq!(mutations.len(), 3);
        assert_eq!(
            mutations[0],
            &vec!["rm".to_string(), "-f".into(), "--".into(), "abc".into()]
        );
        assert!(!calls.iter().flatten().any(|s| s == "-v" || s == "keep"));
    }
    #[test]
    fn partial_failure_keeps_confirmed_progress() {
        let error = execute_with(&plan(), |args| {
            if args[0] == "volume" {
                Err(io::Error::other("disk busy"))
            } else {
                Ok(if args[0] == "ps" {
                    "abc\n".into()
                } else {
                    String::new()
                })
            }
        })
        .unwrap_err();
        assert!(error.to_string().contains("Removed container api"));
        assert!(error.to_string().contains("disk busy"));
    }
}
