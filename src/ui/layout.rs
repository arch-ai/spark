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
    modal_height.saturating_sub(if is_volume && modal_height >= 12 {
        8
    } else if !is_volume && modal_height < 12 {
        5
    } else {
        6
    })
}
pub fn docker_list_header_height(modal_height: u16) -> u16 {
    u16::from(modal_height >= 6)
}
pub fn docker_list_top_padding(modal_height: u16) -> u16 {
    if modal_height < 12 {
        1
    } else {
        2
    }
}

pub fn sort_menu_area(bounds: Rect, count: usize, selected: usize) -> (Rect, usize) {
    let width = 38.min(bounds.width.saturating_sub(2));
    let height = (count as u16 + 2).min(bounds.height.saturating_sub(2));
    let area = Rect::new(
        bounds.x + (bounds.width - width) / 2,
        bounds.y + (bounds.height - height) / 2,
        width,
        height,
    );
    let capacity = height.saturating_sub(2) as usize;
    let scroll = selected
        .saturating_add(1)
        .saturating_sub(capacity)
        .min(count.saturating_sub(capacity));
    (area, scroll)
}

#[derive(Clone, Copy)]
pub struct CollectionHeader {
    pub area: Rect,
    pub details: Rect,
    pub search: Rect,
    pub columns: bool,
}

/// All main sections share the same header and search geometry.
pub fn collection_header(area: Rect, view: crate::app::ViewMode) -> CollectionHeader {
    use crate::app::ViewMode;
    let columns = area.width >= 70 && area.height >= 16;
    let detail_height = if columns {
        if matches!(view, ViewMode::Docker | ViewMode::Node) {
            2
        } else {
            1
        }
    } else if view == ViewMode::Node {
        if area.height >= 16 {
            2
        } else {
            1
        }
    } else {
        u16::from(area.height >= 14)
    };
    if columns {
        let header = Rect::new(area.x, area.y, area.width, detail_height + 2);
        let inner = Rect::new(
            header.x + 1,
            header.y + 1,
            header.width.saturating_sub(2),
            detail_height,
        );
        let parts = Layout::horizontal([Constraint::Ratio(2, 3), Constraint::Ratio(1, 3)])
            .spacing(3)
            .split(inner);
        CollectionHeader {
            area: header,
            details: parts[0],
            search: parts[1],
            columns,
        }
    } else {
        let search_height = if area.height < 16 { 1 } else { 3 };
        CollectionHeader {
            area: Rect::new(
                area.x,
                area.y,
                area.width,
                1 + detail_height + search_height,
            ),
            details: Rect::new(area.x, area.y + 1, area.width, detail_height),
            search: Rect::new(
                area.x,
                area.y + 1 + detail_height,
                area.width,
                search_height,
            ),
            columns,
        }
    }
}

pub fn process_layout(area: Rect) -> [Rect; 6] {
    let header = collection_header(area, crate::app::ViewMode::Process);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header.area.height),
            Constraint::Length(if area.height < 18 { 2 } else { 4 }),
            Constraint::Min(5),
            Constraint::Length(if area.height < 16 { 1 } else { 2 }),
        ])
        .split(area);
    [
        header.area,
        header.details,
        header.search,
        chunks[1],
        chunks[2],
        chunks[3],
    ]
}

pub fn process_table(width: u16, height: u16) -> Rect {
    process_layout(Rect::new(0, 0, width, height))[4]
}

/// Collection layout shared by ports rendering and input.
pub fn resource_layout(area: Rect) -> [Rect; 5] {
    let header = collection_header(area, crate::app::ViewMode::Ports);
    let chunks = Layout::vertical([
        Constraint::Length(header.area.height),
        Constraint::Min(3),
        Constraint::Length(if area.height < 16 { 1 } else { 2 }),
    ])
    .split(area);
    [
        header.area,
        header.details,
        header.search,
        chunks[1],
        chunks[2],
    ]
}

pub fn resource_table(width: u16, height: u16) -> Rect {
    resource_layout(Rect::new(0, 0, width, height))[3]
}

pub const NODE_TAB_LABELS: [&str; 2] = ["Node.js Processes", "PM2"];
pub const NODE_TAB_DIVIDER: &str = " | ";

pub fn node_layout(area: Rect) -> [Rect; 6] {
    let header = collection_header(area, crate::app::ViewMode::Node);
    let chunks = Layout::vertical([
        Constraint::Length(header.area.height),
        Constraint::Min(3),
        Constraint::Length(if area.height < 16 { 1 } else { 2 }),
    ])
    .split(area);
    [
        header.area,
        Rect::new(header.details.x, header.details.y, header.details.width, 1),
        Rect::new(
            header.details.x,
            header.details.y + 1,
            header.details.width,
            header.details.height.saturating_sub(1),
        ),
        header.search,
        chunks[1],
        chunks[2],
    ]
}

/// Matches the Tabs widget's one-cell padding and divider.
pub fn node_tab_areas(area: Rect) -> [Rect; 2] {
    let mut x = area.x;
    NODE_TAB_LABELS.map(|label| {
        let width = (label.len() as u16 + 2).min(area.right().saturating_sub(x));
        let tab = Rect::new(x, area.y, width, area.height);
        x = x.saturating_add(width + NODE_TAB_DIVIDER.len() as u16);
        tab
    })
}

pub fn node_tables(area: Rect, tab: crate::app::NodeTab) -> (Rect, Rect) {
    let table = node_layout(area)[4];
    let empty = Rect::new(table.x, table.y, table.width, 0);
    match tab {
        crate::app::NodeTab::Processes => (empty, table),
        crate::app::NodeTab::Pm2 => (table, empty),
    }
}

/// Shared by rendering, scrolling and mouse hit testing.
pub fn docker_layout(area: Rect) -> [Rect; 6] {
    let header = collection_header(area, crate::app::ViewMode::Docker);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header.area.height),
            Constraint::Length(if area.height < 22 { 0 } else { 7 }),
            Constraint::Min(3),
            Constraint::Length(if area.height < 16 { 1 } else { 2 }),
        ])
        .split(area);
    [
        header.area,
        header.details,
        header.search,
        chunks[1],
        chunks[2],
        chunks[3],
    ]
}

pub fn docker_table(width: u16, height: u16) -> Rect {
    docker_layout(Rect::new(0, 0, width, height))[4]
}

pub fn docker_disk_row(width: u16, height: u16, y: u16) -> Option<usize> {
    let area = docker_layout(Rect::new(0, 0, width, height))[3];
    if area.height >= 7 && y >= area.y + 2 && y < area.bottom().saturating_sub(1) {
        Some((y - area.y - 2) as usize)
    } else {
        None
    }
}

pub fn sidebar_menu_start(height: u16) -> u16 {
    if height < 18 {
        2
    } else {
        9
    }
}

pub fn prune_confirmation(bounds: Rect) -> (Rect, Rect, Rect) {
    let width = bounds.width.min(64);
    let height = bounds.height.min(10);
    let area = Rect::new(
        bounds.x + (bounds.width - width) / 2,
        bounds.y + (bounds.height - height) / 2,
        width,
        height,
    );
    let button_width = (width.saturating_sub(4) / 2).min(10);
    let x = area.x + (width.saturating_sub(button_width * 2 + 2)) / 2;
    let y = area.bottom().saturating_sub(2);
    (
        area,
        Rect::new(x, y, button_width, 1),
        Rect::new(x + button_width + 2, y, button_width, 1),
    )
}
