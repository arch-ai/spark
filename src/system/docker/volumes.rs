//! Volume size and attachment history from the active Docker context.
//! Docker has no volume last-access timestamp; container stops are evidence only.
use std::collections::{BTreeSet, HashMap};
use std::io;
use std::process::Command;
use std::time::SystemTime;

use chrono::{DateTime, Datelike, Utc};
use serde_json::Value;

use super::command::DockerCommand;
use super::DockerListItem;

const CONTAINER_FORMAT: &str = r#"{"Id":{{json .Id}},"Name":{{json .Name}},"Status":{{json .State.Status}},"FinishedAt":{{json .State.FinishedAt}},"Mounts":{{json .Mounts}},"Image":{{json .Config.Image}},"Project":{{json (index .Config.Labels "com.docker.compose.project")}},"WorkingDir":{{json (index .Config.Labels "com.docker.compose.project.working_dir")}}}"#;

#[derive(Debug, PartialEq)]
struct VolumeSize {
    size: String,
    links: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct VolumeAttachment {
    pub id: String,
    pub container: String,
    pub status: String,
    pub image: String,
    pub project: Option<String>,
    pub project_dir: Option<String>,
    pub(crate) finished_at: Option<DateTime<Utc>>,
}
type Attachment = VolumeAttachment;

#[derive(Default)]
struct VolumeCatalog {
    sizes: HashMap<String, VolumeSize>,
    attachments: HashMap<String, Vec<Attachment>>,
    size_error: Option<String>,
    activity_error: Option<String>,
}

pub fn load_docker_volumes() -> io::Result<Vec<DockerListItem>> {
    let names = docker_text(&["volume", "ls", "--format", "{{.Name}}"])?;
    if names.trim().is_empty() {
        return Ok(Vec::new());
    }
    let catalog = load_catalog();
    let now = DateTime::<Utc>::from(SystemTime::now());
    Ok(names
        .lines()
        .filter(|name| !name.trim().is_empty())
        .map(|name| {
            let name = name.trim();
            let size = catalog
                .sizes
                .get(name)
                .map(|entry| entry.size.clone())
                .unwrap_or_else(|| "Unknown".into());
            let entries = catalog
                .attachments
                .get(name)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let activity = activity_label(entries, catalog.activity_error.as_deref(), now);
            let (containers, images) =
                attachment_summary(entries, catalog.activity_error.as_deref());
            DockerListItem {
                name: name.to_string(),
                id: name.to_string(),
                size,
                detail_left: containers,
                detail_right: images,
                activity: Some(activity),
                activity_age_secs: activity_age(entries, catalog.activity_error.as_deref(), now),
                detail_project: project_summary(entries, catalog.activity_error.as_deref()),
                attachments: catalog.activity_error.is_none().then(|| entries.to_vec()),
            }
        })
        .collect())
}

pub fn inspect_docker_volume(name: &str) -> io::Result<String> {
    let raw = docker_text(&["volume", "inspect", name])?;
    let volume: Value = serde_json::from_str(&raw).map_err(io::Error::other)?;
    let info = volume
        .as_array()
        .and_then(|values| values.first())
        .ok_or_else(|| io::Error::other("Docker returned no volume details"))?;
    Ok(volume_report(
        name,
        info,
        &load_catalog(),
        DateTime::<Utc>::from(SystemTime::now()),
    ))
}

/// The UI confirmation covers attached containers, including running ones.
pub fn delete_docker_volume(name: &str) -> io::Result<String> {
    delete_volume_with(name, docker_text)
}

fn delete_volume_with(
    name: &str,
    mut run: impl FnMut(&[&str]) -> io::Result<String>,
) -> io::Result<String> {
    // Inspect exact mount names rather than matching truncated names or bind paths.
    // Failure to discover references must not be interpreted as an empty list.
    let mut entries = load_attachments_with(&mut run)?
        .remove(name)
        .unwrap_or_default();
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    entries.dedup_by(|a, b| a.id == b.id);
    if entries.iter().any(|entry| entry.id.is_empty()) {
        return Err(io::Error::other(
            "Container inspection omitted an ID; nothing was deleted",
        ));
    }
    let mut removed = Vec::new();
    for entry in entries {
        // Do not pass -v: other volumes attached to these containers must survive.
        if let Err(error) = run(&["rm", "-f", "--", &entry.id]) {
            return Err(delete_error(error, &format!("Could not remove attached container {} ({}). Volume deletion was not attempted.", entry.container, entry.id), &removed));
        }
        removed.push(format!(
            "Removed container {} ({})",
            entry.container, entry.id
        ));
    }
    // Docker refuses removal if a new reference appeared during cleanup. Do not
    // recursively delete containers that were not in the inspected set.
    if let Err(error) = run(&["volume", "rm", "--", name]) {
        return Err(delete_error(
            error,
            "Volume removal did not complete successfully. Refresh its details before retrying.",
            &removed,
        ));
    }
    removed.push(format!("Removed volume {name}"));
    Ok(removed.join("\n"))
}

fn delete_error(error: io::Error, context: &str, removed: &[String]) -> io::Error {
    let progress = if removed.is_empty() {
        "No removals were confirmed.".into()
    } else {
        format!("Completed before the error:\n{}", removed.join("\n"))
    };
    io::Error::new(error.kind(), format!("{context}\n\n{error}\n\n{progress}"))
}

fn docker_text(args: &[&str]) -> io::Result<String> {
    let output = Command::new("docker").args(args).docker_output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "docker {}: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn load_catalog() -> VolumeCatalog {
    // Disk accounting is often the slowest request. It does not block attachment inspection.
    let (sizes, attachments) = std::thread::scope(|scope| {
        let sizes = scope.spawn(|| {
            docker_text(&["system", "df", "-v"]).and_then(|text| parse_volume_sizes(&text))
        });
        let attachments = load_attachments();
        (
            sizes
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("Volume size request stopped"))),
            attachments,
        )
    });
    let mut catalog = VolumeCatalog::default();
    match sizes {
        Ok(sizes) => catalog.sizes = sizes,
        Err(error) => catalog.size_error = Some(error.to_string()),
    }
    match attachments {
        Ok(attachments) => catalog.attachments = attachments,
        Err(error) => catalog.activity_error = Some(error.to_string()),
    }
    catalog
}

fn parse_volume_sizes(text: &str) -> io::Result<HashMap<String, VolumeSize>> {
    let mut in_volumes = false;
    let mut saw_section = false;
    let mut sizes = HashMap::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with("Local Volumes space usage:") {
            in_volumes = true;
            saw_section = true;
            continue;
        }
        if !in_volumes {
            continue;
        }
        if line.ends_with("space usage:") {
            break;
        }
        let columns: Vec<_> = line.split_whitespace().collect();
        if columns.is_empty()
            || columns.starts_with(&["NAME", "LINKS", "SIZE"])
            || columns.starts_with(&["VOLUME", "NAME", "LINKS", "SIZE"])
        {
            continue;
        }
        let mut fields = line.split_whitespace();
        let Some(name) = fields.next() else {
            continue;
        };
        let Some(links) = fields.next() else {
            continue;
        };
        let size = fields.collect::<Vec<_>>().join(" ");
        if size.is_empty() {
            continue;
        }
        let size = if size.starts_with('-') || size.eq_ignore_ascii_case("N/A") {
            "Unknown".into()
        } else {
            size
        };
        sizes.insert(
            name.to_string(),
            VolumeSize {
                size,
                links: links.parse().ok(),
            },
        );
    }
    if !saw_section {
        return Err(io::Error::other("Docker did not return volume disk usage"));
    }
    Ok(sizes)
}

fn load_attachments() -> io::Result<HashMap<String, Vec<Attachment>>> {
    load_attachments_with(&mut docker_text)
}

fn load_attachments_with(
    run: &mut impl FnMut(&[&str]) -> io::Result<String>,
) -> io::Result<HashMap<String, Vec<Attachment>>> {
    let ids = run(&["ps", "-a", "-q", "--no-trunc"])?;
    let ids: Vec<_> = ids
        .lines()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .collect();
    let mut attachments = HashMap::new();
    // Batches avoid one subprocess per container and do not exceed the command-line limit.
    for chunk in ids.chunks(32) {
        let mut args = vec![
            "inspect",
            "--type",
            "container",
            "--format",
            CONTAINER_FORMAT,
        ];
        args.extend_from_slice(chunk);
        let text = run(&args)?;
        parse_attachments(&text, &mut attachments)?;
    }
    Ok(attachments)
}

fn parse_attachments(
    text: &str,
    attachments: &mut HashMap<String, Vec<Attachment>>,
) -> io::Result<()> {
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let info: Value = serde_json::from_str(line).map_err(io::Error::other)?;
        let container = info["Name"]
            .as_str()
            .filter(|name| !name.is_empty())
            .ok_or_else(|| io::Error::other("Container inspection omitted its name"))?;
        let status = info["Status"]
            .as_str()
            .ok_or_else(|| io::Error::other("Container inspection omitted its state"))?;
        let project = info["Project"].as_str().filter(|value| !value.is_empty());
        let image = info["Image"]
            .as_str()
            .filter(|value| !value.is_empty())
            .unwrap_or("Unknown");
        let entry = Attachment {
            id: info["Id"].as_str().unwrap_or_default().to_string(),
            container: container.trim_start_matches('/').to_string(),
            status: status.to_string(),
            image: image.to_string(),
            project: project.map(str::to_string),
            project_dir: info["WorkingDir"]
                .as_str()
                .filter(|value| !value.trim().is_empty())
                .map(str::to_string),
            finished_at: parse_timestamp(info["FinishedAt"].as_str().unwrap_or("")),
        };
        // Docker's Go template may encode an empty mount slice as null.
        if info.get("Mounts").is_some_and(Value::is_null) {
            continue;
        }
        let mounts = info["Mounts"]
            .as_array()
            .ok_or_else(|| io::Error::other("Container inspection omitted its mounts"))?;
        for mount in mounts {
            if mount["Type"].as_str() != Some("volume") {
                continue;
            }
            let Some(name) = mount["Name"].as_str().filter(|name| !name.is_empty()) else {
                continue;
            };
            let volume = attachments.entry(name.to_string()).or_default();
            if !volume
                .iter()
                .any(|existing| existing.container == entry.container)
            {
                volume.push(entry.clone());
            }
        }
    }
    Ok(())
}

fn parse_timestamp(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .filter(|time| time.year() > 1)
        .map(|time| time.with_timezone(&Utc))
}

fn age(time: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let seconds = now.signed_duration_since(time).num_seconds();
    if seconds < 0 {
        return "future timestamp".into();
    }
    match seconds {
        0..=59 => "<1m ago".into(),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86399 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86400),
    }
}

fn activity_label(entries: &[Attachment], error: Option<&str>, now: DateTime<Utc>) -> String {
    if error.is_some() {
        return "Unknown".into();
    }
    if entries.iter().any(|entry| entry.status == "running") {
        return "Attached now".into();
    }
    if entries.iter().any(|entry| entry.status == "paused") {
        return "Paused".into();
    }
    if entries.iter().any(|entry| entry.status == "restarting") {
        return "Restarting".into();
    }
    match entries.iter().filter_map(|entry| entry.finished_at).max() {
        Some(time) => format!("Stop {}", age(time, now)),
        None => "Unknown".into(),
    }
}

fn attachment_summary(entries: &[Attachment], error: Option<&str>) -> (String, String) {
    if error.is_some() {
        return ("Containers: Unknown".into(), "Images: Unknown".into());
    }
    if entries.is_empty() {
        return (
            "Containers: None".into(),
            "Images: None (no attached containers)".into(),
        );
    }
    let containers = entries
        .iter()
        .map(|entry| entry.container.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(", ");
    let images = entries
        .iter()
        .map(|entry| entry.image.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(", ");
    (
        format!("Containers: {containers}"),
        format!("Images: {images}"),
    )
}

fn activity_age(entries: &[Attachment], error: Option<&str>, now: DateTime<Utc>) -> Option<u64> {
    if error.is_some() {
        return None;
    }
    if entries
        .iter()
        .any(|entry| matches!(entry.status.as_str(), "running" | "paused" | "restarting"))
    {
        return Some(0);
    }
    entries
        .iter()
        .filter_map(|entry| entry.finished_at)
        .max()
        .and_then(|time| u64::try_from(now.signed_duration_since(time).num_seconds()).ok())
}

fn project_summary(entries: &[Attachment], error: Option<&str>) -> String {
    if error.is_some() {
        return "Projects: Unknown".into();
    }
    if entries.is_empty() {
        return "Projects: None".into();
    }
    let projects = entries
        .iter()
        .map(|entry| {
            format!(
                "{}: {}",
                entry.project.as_deref().unwrap_or("Unmanaged"),
                entry.project_dir.as_deref().unwrap_or("Unknown directory")
            )
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(", ");
    format!("Projects: {projects}")
}

fn volume_report(name: &str, info: &Value, catalog: &VolumeCatalog, now: DateTime<Utc>) -> String {
    let entries = catalog
        .attachments
        .get(name)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut lines = vec![format!("VOLUME: {name}"), String::new()];
    match catalog.sizes.get(name) {
        Some(size) => {
            lines.push(format!("Size: {} (Docker disk usage)", size.size));
            if size.size == "Unknown" {
                lines.push(
                    "Size unavailable: the volume driver did not report a usable size.".into(),
                );
            }
            lines.push(format!(
                "Container references: {}",
                size.links
                    .map(|links| links.to_string())
                    .unwrap_or_else(|| "Unknown".into())
            ));
        }
        None => lines.push(format!(
            "Size: Unknown - {}",
            catalog
                .size_error
                .as_deref()
                .unwrap_or("Docker did not report a size; the volume driver may not support it")
        )),
    }
    let (containers, images) = attachment_summary(entries, catalog.activity_error.as_deref());
    lines.push(containers);
    lines.push(images);
    lines.push(project_summary(entries, catalog.activity_error.as_deref()));
    lines.push(format!(
        "Container activity: {}",
        activity_label(entries, catalog.activity_error.as_deref(), now)
    ));
    lines.push(String::new());
    lines.push("ATTACHED CONTAINERS".into());
    if entries.is_empty() {
        lines.push(
            if catalog.activity_error.is_some() {
                "Unknown"
            } else {
                "None"
            }
            .into(),
        );
    }
    let mut ordered_entries: Vec<_> = entries.iter().collect();
    ordered_entries.sort_by_key(|entry| &entry.container);
    for entry in ordered_entries {
        lines.push(format!("Container: {} | {}", entry.container, entry.status));
        lines.push(format!("  Image: {}", entry.image));
        if let Some(project) = &entry.project {
            lines.push(format!("  Project: {project}"));
        }
        lines.push(format!(
            "  Project directory: {}",
            entry
                .project_dir
                .as_deref()
                .unwrap_or("Unknown (no Compose working directory label)")
        ));
        if let Some(time) = entry.finished_at {
            lines.push(format!(
                "  Last stop: {} ({})",
                time.format("%Y-%m-%d %H:%M:%S UTC"),
                age(time, now)
            ));
        }
    }
    lines.push(String::new());
    if let Some(error) = &catalog.activity_error {
        lines.push(format!("Activity unavailable: {error}"));
    } else if entries.iter().any(|entry| entry.status == "running") {
        lines.push("Usage evidence: attached to a currently running container.".into());
    } else if let Some(time) = entries.iter().filter_map(|entry| entry.finished_at).max() {
        lines.push(format!(
            "Most recent known container stop: {} ({})",
            time.format("%Y-%m-%d %H:%M:%S UTC"),
            age(time, now)
        ));
    } else if entries.is_empty() {
        lines.push("No existing containers reference this volume.".into());
    } else {
        lines.push("Attached containers have no recorded stop time.".into());
    }
    lines.push("Last file access: Unknown - Docker does not record it.".into());
    lines.push("Container activity is usage evidence, not a file-access timestamp. Removed containers' history is unavailable.".into());
    lines.push(String::new());
    for (label, field) in [
        ("Created", "CreatedAt"),
        ("Driver", "Driver"),
        ("Mountpoint", "Mountpoint"),
        ("Scope", "Scope"),
    ] {
        lines.push(format!(
            "{label}: {}",
            info[field].as_str().unwrap_or("Unknown")
        ));
    }
    lines.push(String::new());
    lines.push("RAW DOCKER INSPECT".into());
    lines.push(serde_json::to_string_pretty(info).unwrap_or_default());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    #[test]
    fn project_directories_remain_attached_to_each_container_without_guessing_missing_paths() {
        let mut parsed = HashMap::new();
        parse_attachments(r#"{"Id":"a","Name":"/api","Status":"running","Image":"demo:latest","Project":"demo","WorkingDir":"/home/dev/项目 with spaces","Mounts":[{"Type":"volume","Name":"data"}]}
{"Id":"b","Name":"/backup","Status":"exited","Project":"demo","WorkingDir":"/srv/backup","Mounts":[{"Type":"volume","Name":"data"}]}
{"Id":"c","Name":"/unmanaged","Status":"exited","Mounts":[{"Type":"volume","Name":"data"}]}"#,&mut parsed).unwrap();
        assert_eq!(
            parsed["data"][0].project_dir.as_deref(),
            Some("/home/dev/项目 with spaces")
        );
        assert!(parsed["data"][2].project_dir.is_none());
        let catalog = VolumeCatalog {
            attachments: parsed,
            ..Default::default()
        };
        let report = volume_report("data", &serde_json::json!({}), &catalog, now());
        assert!(report.contains("Container: api | running\n  Image: demo:latest\n  Project: demo\n  Project directory: /home/dev/项目 with spaces"));
        assert!(report.contains("Project directory: /srv/backup"));
        assert!(report.contains("Project directory: Unknown"));
    }
    use super::*;
    use serde_json::json;

    fn deletion_fixture() -> String {
        [
            json!({"Id":"aaa", "Name":"/database", "Status":"running", "Mounts":[
                {"Type":"volume", "Name":"data"}, {"Type":"volume", "Name":"data"},
                {"Type":"volume", "Name":"keep-this-volume"}]}),
            json!({"Id":"bbb", "Name":"/backup", "Status":"exited", "Mounts":[
                {"Type":"volume", "Name":"data"}]}),
            json!({"Id":"ccc", "Name":"/unrelated", "Status":"running", "Mounts":[
                {"Type":"volume", "Name":"data-other"}, {"Type":"bind", "Name":"data"}]}),
        ]
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
    }

    #[test]
    fn deleting_volume_removes_running_and_stopped_references_before_only_that_volume() {
        let mut mutations = Vec::new();
        let result = delete_volume_with("data", |args| match args[0] {
            "ps" => Ok("aaa\nbbb\nccc".into()),
            "inspect" => Ok(deletion_fixture()),
            _ => {
                mutations.push(args.iter().map(|s| s.to_string()).collect::<Vec<_>>());
                Ok(String::new())
            }
        })
        .unwrap();
        assert_eq!(
            mutations,
            vec![
                vec!["rm", "-f", "--", "aaa"],
                vec!["rm", "-f", "--", "bbb"],
                vec!["volume", "rm", "--", "data"],
            ]
        );
        assert!(result.contains("Removed container database (aaa)"));
        assert!(result.contains("Removed container backup (bbb)"));
        assert!(result.contains("Removed volume data"));
    }

    #[test]
    fn failed_container_removal_stops_cleanup_and_reports_partial_success() {
        let error = delete_volume_with("data", |args| match args[0] {
            "ps" => Ok("aaa\nbbb\nccc".into()),
            "inspect" => Ok(deletion_fixture()),
            "rm" if args.last() == Some(&"aaa") => Ok(String::new()),
            "rm" => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "permission denied",
            )),
            _ => panic!("Volume must not be removed after a container failure"),
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        let error = error.to_string();
        assert!(error.contains("backup (bbb)"));
        assert!(error.contains("Removed container database (aaa)"));
        assert!(error.contains("Volume deletion was not attempted"));
    }

    #[test]
    fn failed_or_incomplete_attachment_discovery_cannot_delete_anything() {
        for inspection in [
            "not json",
            r#"{"Name":"/database","Status":"running","Mounts":[{"Type":"volume","Name":"data"}]}"#,
        ] {
            assert!(delete_volume_with("data", |args| match args[0] {
                "ps" => Ok("aaa".into()),
                "inspect" => Ok(inspection.into()),
                _ => panic!("Incomplete inspection must not trigger removals"),
            })
            .is_err());
        }
    }

    #[test]
    fn a_late_reference_or_timeout_preserves_the_volume_error_and_removed_container_report() {
        for kind in [io::ErrorKind::Other, io::ErrorKind::TimedOut] {
            let error = delete_volume_with("data", |args| match args[0] {
                "ps" => Ok("aaa\nbbb\nccc".into()),
                "inspect" => Ok(deletion_fixture()),
                "rm" => Ok(String::new()),
                "volume" => Err(io::Error::new(kind, "volume still busy")),
                _ => unreachable!(),
            })
            .unwrap_err();
            assert_eq!(error.kind(), kind);
            assert!(error.to_string().contains("volume still busy"));
            assert!(error.to_string().contains("Removed container backup (bbb)"));
        }
    }

    fn now() -> DateTime<Utc> {
        parse_timestamp("2026-09-15T12:00:00Z").unwrap()
    }

    #[test]
    fn attachment_summaries_show_containers_and_unique_images_with_explicit_unknowns() {
        let first = attachment("running", "");
        let mut second = first.clone();
        second.container = "backup".into();
        let (containers, images) = attachment_summary(&[first.clone(), second.clone()], None);
        assert_eq!(containers, "Containers: backup, database");
        assert_eq!(images, "Images: postgres:17");
        second.image = "alpine:3".into();
        assert_eq!(
            attachment_summary(&[first, second], None).1,
            "Images: alpine:3, postgres:17"
        );
        assert_eq!(attachment_summary(&[], None).0, "Containers: None");
        assert_eq!(
            attachment_summary(&[], Some("inspection failed")),
            ("Containers: Unknown".into(), "Images: Unknown".into())
        );
        let mut parsed = HashMap::new();
        parse_attachments(r#"{"Name":"/db","Status":"running","Project":"demo","Mounts":[{"Type":"volume","Name":"data"}]}"#, &mut parsed).unwrap();
        assert_eq!(parsed["data"][0].image, "Unknown");
        assert_eq!(parsed["data"][0].project.as_deref(), Some("demo"));
    }

    fn attachment(status: &str, finished: &str) -> Attachment {
        Attachment {
            id: "abc123".into(),
            container: "database".into(),
            status: status.into(),
            image: "postgres:17".into(),
            project: Some("demo".into()),
            project_dir: Some("/home/dev/demo".into()),
            finished_at: parse_timestamp(finished),
        }
    }

    #[test]
    fn null_mounts_are_empty_and_a_volume_named_name_is_not_a_header() {
        let mut entries = HashMap::new();
        parse_attachments(
            r#"{"Name":"/api","Status":"running","Mounts":null}"#,
            &mut entries,
        )
        .unwrap();
        assert!(entries.is_empty());
        let sizes =
            parse_volume_sizes("Local Volumes space usage:\nNAME\tLINKS\tSIZE\nNAME 1 24 B")
                .unwrap();
        assert_eq!(sizes["NAME"].size, "24 B");
    }

    #[test]
    fn sizes_parse_both_headers_links_and_spaced_units() {
        for header in [
            "NAME                         LINKS       SIZE",
            "VOLUME NAME                  LINKS       SIZE",
        ] {
            let text = format!("Images space usage:\nimage-data\n\nLocal Volumes space usage:\n\n{header}\nvolume-a     2      36 B\nvolume-b 0 1.25 GB\nempty 0 0B\n\nBuild cache space usage:\nnot-a-volume 1 500MB\n");
            let sizes = parse_volume_sizes(&text).unwrap();
            assert_eq!(sizes.len(), 3);
            assert_eq!(
                sizes["volume-a"],
                VolumeSize {
                    size: "36 B".into(),
                    links: Some(2)
                }
            );
            assert_eq!(sizes["volume-b"].size, "1.25 GB");
            assert_eq!(sizes["empty"].size, "0B");
        }
    }

    #[test]
    fn unavailable_sizes_are_unknown_and_missing_sections_are_errors() {
        let sizes = parse_volume_sizes(
            "Local Volumes space usage:\nNAME LINKS SIZE\nremote -1 -1B\nplugin 0 N/A\n",
        )
        .unwrap();
        assert_eq!(sizes["remote"].size, "Unknown");
        assert_eq!(sizes["remote"].links, None);
        assert_eq!(sizes["plugin"].size, "Unknown");
        assert!(parse_volume_sizes("Docker connection failed").is_err());
    }

    #[test]
    fn full_mount_names_are_matched_and_bind_mounts_are_ignored() {
        let info = json!({"Name":"/api", "Status":"exited", "FinishedAt":"2026-09-13T12:00:00.123456789Z", "Project":"demo", "Image":"app:latest", "Mounts":[
            {"Type":"volume", "Name":"a-very-long-volume-name-that-ps-can-truncate"},
            {"Type":"volume", "Name":"a-very-long-volume-name-that-ps-can-truncate"},
            {"Type":"bind", "Name":"ignore-bind", "Source":"/tmp"}
        ]});
        let mut entries = HashMap::new();
        parse_attachments(&info.to_string(), &mut entries).unwrap();
        assert_eq!(entries.len(), 1);
        let volume = &entries["a-very-long-volume-name-that-ps-can-truncate"];
        assert_eq!(volume.len(), 1);
        assert_eq!(volume[0].container, "api");
        assert_eq!(volume[0].project.as_deref(), Some("demo"));
        assert_eq!(volume[0].image, "app:latest");
        assert!(parse_attachments("not json", &mut entries).is_err());
    }

    #[test]
    fn latest_stop_is_based_on_absolute_time_and_active_attachments_take_precedence() {
        let mut entries = vec![
            attachment("exited", "2026-09-10T12:00:00Z"),
            attachment("exited", "2026-09-13T15:00:00+03:00"),
        ];
        assert_eq!(activity_label(&entries, None, now()), "Stop 2d ago");
        entries.push(attachment("running", "0001-01-01T00:00:00Z"));
        assert_eq!(activity_label(&entries, None, now()), "Attached now");
        assert_eq!(
            activity_label(&entries, Some("incomplete inspection"), now()),
            "Unknown"
        );
        assert_eq!(
            activity_label(&[attachment("paused", "")], None, now()),
            "Paused"
        );
        assert_eq!(
            activity_label(&[attachment("restarting", "")], None, now()),
            "Restarting"
        );
    }

    #[test]
    fn missing_history_and_future_clocks_do_not_invent_last_use() {
        assert!(parse_timestamp("0001-01-01T00:00:00.000000000Z").is_none());
        assert!(parse_timestamp("").is_none());
        assert_eq!(
            activity_label(
                &[attachment("created", "0001-01-01T00:00:00Z")],
                None,
                now()
            ),
            "Unknown"
        );
        assert_eq!(activity_label(&[], None, now()), "Unknown");
        let time = parse_timestamp("2026-09-16T12:00:00Z").unwrap();
        assert_eq!(age(time, now()), "future timestamp");
    }

    #[test]
    fn detailed_report_explains_size_age_and_limits_without_using_creation_as_last_use() {
        let mut catalog = VolumeCatalog::default();
        catalog.sizes.insert(
            "data".into(),
            VolumeSize {
                size: "2.5 GB".into(),
                links: Some(1),
            },
        );
        catalog.attachments.insert(
            "data".into(),
            vec![attachment("exited", "2026-09-13T12:00:00Z")],
        );
        let info = json!({"CreatedAt":"2015-01-01T00:00:00Z", "Driver":"local", "Mountpoint":"/volumes/data", "Scope":"local"});
        let report = volume_report("data", &info, &catalog, now());
        assert!(report.contains("Size: 2.5 GB"));
        assert!(report.contains("Container references: 1"));
        assert!(report.contains("Containers: database"));
        assert!(report.contains("Images: postgres:17"));
        assert!(
            report.contains("Container: database | exited\n  Image: postgres:17\n  Project: demo")
        );
        assert!(report.contains("2026-09-13 12:00:00 UTC (2d ago)"));
        assert!(report.contains("Last file access: Unknown"));
        assert!(report.contains("RAW DOCKER INSPECT"));
        let orphan = volume_report("orphan", &info, &catalog, now());
        assert!(orphan.contains("Container activity: Unknown"));
        assert!(!orphan.contains("Most recent known container stop"));
    }

    #[test]
    fn failed_queries_are_visible_and_do_not_imply_zero_size_or_no_references() {
        let catalog = VolumeCatalog {
            size_error: Some("disk usage timed out".into()),
            activity_error: Some("container disappeared".into()),
            ..VolumeCatalog::default()
        };
        let report = volume_report("data", &json!({}), &catalog, now());
        assert!(report.contains("Size: Unknown - disk usage timed out"));
        assert!(report.contains("Activity unavailable: container disappeared"));
        assert!(!report.contains("No existing containers reference"));
    }
}
