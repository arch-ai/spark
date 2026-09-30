use crate::system::docker::command::DockerCommand;
use std::io;
use std::process::Command;

#[derive(Clone, Debug)]
pub struct DockerListItem {
    pub name: String,
    pub id: String,
    pub size: String,
    pub detail_left: String,
    pub detail_right: String,
    /// Volume activity inferred from containers, never a file access timestamp.
    pub activity: Option<String>,
}

pub fn load_container_env(container_id: &str) -> io::Result<Vec<String>> {
    let output = Command::new("docker")
        .args([
            "inspect",
            "--format",
            "{{range .Config.Env}}{{println .}}{{end}}",
            container_id,
        ])
        .docker_output()?;

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
        .docker_output()?;

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

pub fn start_container(container_id: &str) -> io::Result<()> {
    let output = Command::new("docker")
        .args(["start", container_id])
        .docker_output()?;

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
        .docker_output()?;

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
        .docker_output()?;

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
        .docker_output()?;

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
        .docker_output()?;

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
        .docker_output()?;

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
        .docker_output()?;

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
        .docker_output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("docker ps failed: {}", stderr.trim()),
        ));
    }

    Ok(parse_list_items(&output))
}

pub fn inspect_docker_image(image_id: &str) -> io::Result<String> {
    let output = Command::new("docker")
        .args(["image", "inspect", image_id])
        .docker_output()?;

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
        .docker_output()?;

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
        .docker_output()?;

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
        .docker_output()?;

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

pub fn prune_volumes() -> io::Result<String> {
    let output = Command::new("docker")
        .args(["volume", "prune", "-f"])
        .docker_output()?;

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
            activity: None,
        });
    }
    items
}
