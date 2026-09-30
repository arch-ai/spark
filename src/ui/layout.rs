use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::Span;

/// Main content bounds, shared by modal rendering and mouse input.
pub fn main_area(area: Rect) -> Rect {
    if area.width >= 60 {
        Rect::new(area.x + 20, area.y, area.width - 20, area.height)
    } else {
        area
    }
}

pub fn context_menu_area(
    bounds: Rect,
    x: u16,
    y: u16,
    labels: &[&str],
    header: Option<&str>,
) -> Rect {
    let content_width = labels
        .iter()
        .copied()
        .chain(header)
        .map(|text| Span::raw(text).width())
        .max()
        .unwrap_or(0);
    let width = (content_width.saturating_add(6).min(u16::MAX as usize) as u16).min(bounds.width);
    let height =
        (labels.len() + usize::from(header.is_some()) + 2).min(bounds.height as usize) as u16;
    Rect::new(
        x.clamp(bounds.x, bounds.right().saturating_sub(width)),
        y.clamp(bounds.y, bounds.bottom().saturating_sub(height)),
        width,
        height,
    )
}

pub fn delete_confirmation(bounds: Rect) -> (Rect, Rect, Rect) {
    let width = bounds.width.min(80);
    let height = bounds.height.min(14);
    let area = Rect::new(
        bounds.x + (bounds.width - width) / 2,
        bounds.y + (bounds.height - height) / 2,
        width,
        height,
    );
    let button_width = 10.min(width / 2);
    let gap = 4.min(width.saturating_sub(button_width * 2));
    let x = area.x + (width.saturating_sub(button_width * 2 + gap)) / 2;
    let y = area.bottom().saturating_sub(3).max(area.y);
    (
        area,
        Rect::new(x, y, button_width, 1.min(height)),
        Rect::new(x + button_width + gap, y, button_width, 1.min(height)),
    )
}

/// Shared by rendering, scrolling and mouse hit testing.
pub fn docker_list_content_height(modal_height: u16, is_volume: bool) -> u16 {
    modal_height.saturating_sub(if is_volume { 7 } else { 6 })
}

pub fn process_layout(area: Rect) -> [Rect; 6] {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(4),
            Constraint::Min(5),
            Constraint::Length(2),
        ])
        .split(area);
    std::array::from_fn(|index| chunks[index])
}

pub fn process_table(height: u16) -> Rect {
    process_layout(Rect::new(0, 0, 1, height))[4]
}

/// Shared by rendering, scrolling and mouse hit testing.
pub fn docker_layout(area: Rect) -> [Rect; 6] {
    let compact = area.height < 28;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(if compact { 1 } else { 3 }),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(if area.height < 22 { 0 } else { 7 }),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(area);
    std::array::from_fn(|index| chunks[index])
}

pub fn docker_table(height: u16) -> Rect {
    docker_layout(Rect::new(0, 0, 1, height))[4]
}

pub fn docker_disk_row(height: u16, y: u16) -> Option<usize> {
    let area = docker_layout(Rect::new(0, 0, 1, height))[3];
    if area.height >= 7 && y >= area.y + 2 && y < area.bottom().saturating_sub(1) {
        Some((y - area.y - 2) as usize)
    } else {
        None
    }
}
