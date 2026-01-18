use std::io;
use std::process::Command;

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
        .args(["image", "prune", "-f"])
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
