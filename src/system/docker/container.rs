use std::io;
use std::process::Command;

#[derive(Clone, Debug)]
pub struct DockerListItem {
    pub name: String,
    pub id: String,
    pub size: String,
    pub detail_left: String,
    pub detail_right: String,
}

pub fn load_container_env(container_id: &str) -> io::Result<Vec<String>> {
    let output = Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{range .Config.Env}}{{println .}}{{end}}",
            container_id,
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("docker inspect failed: {}", stderr.trim()),
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut lines: Vec<String> = stdout
        .lines()
        .map(|line| line.trim_end_matches('\r').to_string())
        .collect();
    if lines.is_empty() {
        lines.push("No env vars found".to_string());
    }
    Ok(lines)
}

pub fn kill_container(container_id: &str) -> io::Result<()> {
    let output = Command::new("docker")
        .args(["kill", container_id])
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("docker kill failed: {}", stderr.trim()),
        ))
    }
}

pub fn kill_containers(container_ids: &[String]) -> (usize, usize) {
    let mut success = 0;
    let mut failed = 0;
    for id in container_ids {
        match kill_container(id) {
            Ok(()) => success += 1,
            Err(_) => failed += 1,
        }
    }
    (success, failed)
}

pub fn start_container(container_id: &str) -> io::Result<()> {
    let output = Command::new("docker")
        .args(["start", container_id])
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("docker start failed: {}", stderr.trim()),
        ))
    }
}

pub fn stop_container(container_id: &str) -> io::Result<()> {
    let output = Command::new("docker")
        .args(["stop", container_id])
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("docker stop failed: {}", stderr.trim()),
        ))
    }
}

pub fn restart_container(container_id: &str) -> io::Result<()> {
    let output = Command::new("docker")
        .args(["restart", container_id])
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("docker restart failed: {}", stderr.trim()),
        ))
    }
}

pub fn prune_build_cache() -> io::Result<String> {
    let output = Command::new("docker")
        .args(["builder", "prune", "-f"])
        .output()?;

    if output.status.success() {
        Ok(prune_output_text(&output))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("docker builder prune failed: {}", stderr.trim()),
        ))
    }
}

pub fn load_container_logs(container_id: &str) -> io::Result<String> {
    let output = Command::new("docker")
        .args(["logs", "--tail", "200", container_id])
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
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("docker logs failed: {}", stderr.trim()),
        ))
    }
}

pub fn prune_dangling_images() -> io::Result<String> {
    let output = Command::new("docker")
        .args(["image", "prune", "-a", "-f"])
        .output()?;

    if output.status.success() {
        Ok(prune_output_text(&output))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("docker image prune failed: {}", stderr.trim()),
        ))
    }
}

pub fn load_docker_images() -> io::Result<Vec<DockerListItem>> {
    let output = Command::new("docker")
        .args([
            "image",
            "ls",
            "--no-trunc",
            "--format",
            "{{.Repository}}:{{.Tag}}|{{.ID}}|{{.Size}}",
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("docker image ls failed: {}", stderr.trim()),
        ));
    }

    Ok(parse_list_items(&output))
}

pub fn load_docker_containers_with_size() -> io::Result<Vec<DockerListItem>> {
    let output = Command::new("docker")
        .args([
            "ps",
            "-a",
            "--no-trunc",
            "--size",
            "--format",
            "{{.Names}}|{{.ID}}|{{.Size}}",
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("docker ps failed: {}", stderr.trim()),
        ));
    }

    Ok(parse_list_items(&output))
}

pub fn load_docker_volumes() -> io::Result<Vec<DockerListItem>> {
    let output = Command::new("docker")
        .args(["volume", "ls", "--format", "{{.Name}}"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("docker volume ls failed: {}", stderr.trim()),
        ));
    }

    let sizes = load_volume_sizes().unwrap_or_default();
    let associations = load_volume_associations().unwrap_or_default();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut items = Vec::new();
    for raw_line in stdout.lines() {
        let name = raw_line.trim_end_matches('\r').trim();
        if name.is_empty() {
            continue;
        }
        let size = sizes.get(name).cloned().unwrap_or_else(|| "-".to_string());
        let (container_display, source_display) = associations
            .get(name)
            .cloned()
            .unwrap_or_else(|| ("-".to_string(), "-".to_string()));
        items.push(DockerListItem {
            name: name.to_string(),
            id: name.to_string(),
            size,
            detail_left: container_display,
            detail_right: source_display,
        });
    }

    Ok(items)
}

pub fn inspect_docker_volume(volume_name: &str) -> io::Result<String> {
    let output = Command::new("docker")
        .args(["volume", "inspect", volume_name])
        .output()?;

    if output.status.success() {
        Ok(command_output_text(&output))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("docker volume inspect failed: {}", stderr.trim()),
        ))
    }
}

pub fn inspect_docker_image(image_id: &str) -> io::Result<String> {
    let output = Command::new("docker")
        .args(["image", "inspect", image_id])
        .output()?;

    if output.status.success() {
        Ok(command_output_text(&output))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("docker image inspect failed: {}", stderr.trim()),
        ))
    }
}

pub fn inspect_docker_container(container_id: &str) -> io::Result<String> {
    let output = Command::new("docker")
        .args(["inspect", container_id])
        .output()?;

    if output.status.success() {
        Ok(command_output_text(&output))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("docker inspect failed: {}", stderr.trim()),
        ))
    }
}

pub fn delete_docker_image(image_id: &str) -> io::Result<()> {
    let output = Command::new("docker")
        .args(["image", "rm", "-f", image_id])
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("docker image rm failed: {}", stderr.trim()),
        ))
    }
}

pub fn delete_docker_container(container_id: &str) -> io::Result<()> {
    let output = Command::new("docker")
        .args(["rm", "-f", container_id])
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("docker rm failed: {}", stderr.trim()),
        ))
    }
}

pub fn delete_docker_volume(volume_name: &str) -> io::Result<()> {
    let output = Command::new("docker")
        .args(["volume", "rm", volume_name])
        .output()?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("docker volume rm failed: {}", stderr.trim()),
        ))
    }
}

pub fn prune_volumes() -> io::Result<String> {
    let output = Command::new("docker")
        .args(["volume", "prune", "-f"])
        .output()?;

    if output.status.success() {
        Ok(prune_output_text(&output))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("docker volume prune failed: {}", stderr.trim()),
        ))
    }
}

fn prune_output_text(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    };
    combined.trim().to_string()
}

fn command_output_text(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stdout.trim().is_empty() {
        stderr.trim().to_string()
    } else if stderr.trim().is_empty() {
        stdout.trim().to_string()
    } else {
        format!("{}\n{}", stdout.trim(), stderr.trim())
    }
}

fn parse_list_items(output: &std::process::Output) -> Vec<DockerListItem> {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut items = Vec::new();
    for raw_line in stdout.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let mut parts = line.splitn(3, '|');
        let name = parts.next().unwrap_or("").trim();
        let id = parts.next().unwrap_or("").trim();
        let size = parts.next().unwrap_or("").trim();
        if name.is_empty() && id.is_empty() {
            continue;
        }
        let display_name = if name.is_empty() { id } else { name };
        items.push(DockerListItem {
            name: display_name.to_string(),
            id: id.to_string(),
            size: if size.is_empty() { "-".to_string() } else { size.to_string() },
            detail_left: "-".to_string(),
            detail_right: "-".to_string(),
        });
    }
    items
}

fn load_volume_sizes() -> io::Result<std::collections::HashMap<String, String>> {
    let output = Command::new("docker")
        .args(["system", "df", "-v"])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("docker system df -v failed: {}", stderr.trim()),
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut map = std::collections::HashMap::new();
    let mut in_volumes = false;
    let mut header_positions: Option<(usize, usize)> = None;

    for raw_line in stdout.lines() {
        let line = raw_line.trim_end_matches('\r');
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if in_volumes {
                continue;
            }
            continue;
        }
        if in_volumes && trimmed.ends_with("space usage:") {
            break;
        }
        if trimmed.starts_with("Local Volumes space usage:") {
            in_volumes = true;
            continue;
        }
        if !in_volumes {
            continue;
        }
        if trimmed.starts_with("VOLUME NAME") {
            let name_start = trimmed.find("VOLUME NAME").unwrap_or(0);
            let size_start = trimmed.find("SIZE").unwrap_or(trimmed.len());
            header_positions = Some((name_start, size_start));
            continue;
        }
        let (name, size) = if let Some((name_start, size_start)) = header_positions {
            let line_len = line.len();
            let size_start = size_start.min(line_len);
            let name_slice = line.get(name_start..size_start).unwrap_or("");
            let size_slice = line.get(size_start..).unwrap_or("");
            (name_slice.trim(), size_slice.trim())
        } else {
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }
            (parts[0], *parts.last().unwrap_or(&"-"))
        };
        if name.is_empty() {
            continue;
        }
        map.insert(name.to_string(), size.to_string());
    }

    Ok(map)
}

fn load_volume_associations() -> io::Result<std::collections::HashMap<String, (String, String)>> {
    let output = Command::new("docker")
        .args([
            "ps",
            "-a",
            "--no-trunc",
            "--format",
            "{{.Names}}|{{.Image}}|{{.Labels}}|{{.Mounts}}",
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("docker ps failed: {}", stderr.trim()),
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut map: std::collections::HashMap<String, Vec<(String, String)>> = std::collections::HashMap::new();

    for raw_line in stdout.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let mut parts = line.splitn(4, '|');
        let name = parts.next().unwrap_or("").trim();
        let image = parts.next().unwrap_or("").trim();
        let labels = parts.next().unwrap_or("").trim();
        let mounts = parts.next().unwrap_or("").trim();
        if name.is_empty() {
            continue;
        }

        let project = labels
            .split(',')
            .find_map(|part| {
                let mut kv = part.splitn(2, '=');
                let key = kv.next().unwrap_or("").trim();
                let value = kv.next().unwrap_or("").trim();
                if key == "com.docker.compose.project" && !value.is_empty() {
                    Some(value.to_string())
                } else {
                    None
                }
            });
        let source = project.unwrap_or_else(|| if image.is_empty() { "-".to_string() } else { image.to_string() });

        for mount in mounts.split(',') {
            let entry = mount.trim();
            if entry.is_empty() || entry == "-" {
                continue;
            }
            let first = entry.split(':').next().unwrap_or("").trim();
            if first.is_empty() || first.starts_with('/') || first.starts_with('.') {
                continue;
            }
            map.entry(first.to_string())
                .or_default()
                .push((name.to_string(), source.clone()));
        }
    }

    let mut summary = std::collections::HashMap::new();
    for (volume, entries) in map {
        if entries.is_empty() {
            continue;
        }
        let mut containers: Vec<String> = entries.iter().map(|(c, _)| c.clone()).collect();
        containers.sort();
        containers.dedup();
        let container_display = if containers.len() == 1 {
            containers[0].clone()
        } else {
            format!("{} (+{})", containers[0], containers.len().saturating_sub(1))
        };

        let mut sources: Vec<String> = entries.iter().map(|(_, s)| s.clone()).collect();
        sources.sort();
        sources.dedup();
        let source_display = if sources.len() == 1 {
            sources[0].clone()
        } else {
            format!("{} (+{})", sources[0], sources.len().saturating_sub(1))
        };

        summary.insert(volume, (container_display, source_display));
    }

    Ok(summary)
}
