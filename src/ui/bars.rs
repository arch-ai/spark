pub(crate) fn format_memory_bar(label: &str, total: u64, available: u64, width: usize) -> String {
    let used = total.saturating_sub(available);
    let bar_width = bar_width_for_screen(width);
    let bar = progress_bar(used, total, bar_width);
    let used_gb = bytes_to_gb(used);
    let total_gb = bytes_to_gb(total);
    let free_gb = bytes_to_gb(available);
    format!(
        "{label}[{bar}] used {used_gb:.2} / {total_gb:.2} GB (free {free_gb:.2} GB)"
    )
}

pub(crate) fn format_cpu_bar(label: &str, usage: f32, width: usize) -> String {
    let bar_width = bar_width_for_screen(width);
    let ratio = (usage as f64 / 100.0).clamp(0.0, 1.0);
    let bar = progress_bar_ratio(ratio, bar_width);
    format!("{label}[{bar}] {usage:.1}%")
}

pub(crate) fn format_swap_bar(label: &str, total: u64, used: u64, width: usize) -> String {
    let free = total.saturating_sub(used);
    let bar_width = bar_width_for_screen(width);
    let bar = progress_bar(used, total, bar_width);
    let used_gb = bytes_to_gb(used);
    let total_gb = bytes_to_gb(total);
    let free_gb = bytes_to_gb(free);
    format!(
        "{label}[{bar}] used {used_gb:.2} / {total_gb:.2} GB (free {free_gb:.2} GB)"
    )
}

fn progress_bar(used: u64, total: u64, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let ratio = if total == 0 { 0.0 } else { used as f64 / total as f64 };
    progress_bar_ratio(ratio, width)
}

fn progress_bar_ratio(ratio: f64, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let ratio = ratio.clamp(0.0, 1.0);
    let filled = ((ratio * width as f64).round() as usize).min(width);
    let empty = width.saturating_sub(filled);
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

fn bar_width_for_screen(width: usize) -> usize {
    let target = width.saturating_sub(40);
    target.clamp(10, 30)
}

fn bytes_to_gb(bytes: u64) -> f64 {
    bytes as f64 / 1024.0 / 1024.0 / 1024.0
}
