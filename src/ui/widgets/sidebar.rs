//! Sidebar navigation widget

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::{Block, Borders, Widget},
};

/// Sidebar navigation widget
pub struct Sidebar<'a> {
    items: Vec<&'a str>,
    active_index: usize,
    selected_index: usize,
    hover_index: Option<usize>,
    has_focus: bool,
    title: &'a str,
    active_color: Color,
    hover_color: Color,
}

impl<'a> Sidebar<'a> {
    pub fn new(items: Vec<&'a str>) -> Self {
        Self {
            items,
            active_index: 0,
            selected_index: 0,
            hover_index: None,
            has_focus: false,
            title: "SPARK",
            active_color: Color::Cyan,
            hover_color: Color::DarkGray,
        }
    }

    pub fn active_index(mut self, index: usize) -> Self {
        self.active_index = index;
        self
    }

    pub fn selected_index(mut self, index: usize) -> Self {
        self.selected_index = index;
        self
    }

    pub fn hover_index(mut self, index: Option<usize>) -> Self {
        self.hover_index = index;
        self
    }

    pub fn has_focus(mut self, focus: bool) -> Self {
        self.has_focus = focus;
        self
    }
}

impl Widget for Sidebar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 3 || area.height < 5 {
            return;
        }

        // Draw the outer block with title
        let block = Block::default()
            .borders(Borders::ALL)
            .title(self.title);
        let inner = block.inner(area);
        block.render(area, buf);

        // Logo area (simplified)
        let logo_lines = [
            "     .     .",
            "  .  |\\   /|  .",
            "     | \\ / |",
            " ---+-- * --+---",
            "     | / \\ |",
            "  .  |/   \\|  .",
            "     '     '",
        ];

        let logo_height = logo_lines.len().min(inner.height.saturating_sub(2) as usize);
        for (i, line) in logo_lines.iter().take(logo_height).enumerate() {
            let y = inner.y + i as u16;
            if y < inner.y + inner.height {
                let truncated: String = line.chars().take(inner.width as usize).collect();
                buf.set_string(inner.x, y, &truncated, Style::default());
            }
        }

        // Menu items below logo
        let menu_start = inner.y + logo_height as u16 + 1;
        let available_height = inner.height.saturating_sub(logo_height as u16 + 1);

        for (i, item) in self.items.iter().enumerate() {
            if i as u16 >= available_height {
                break;
            }
            let y = menu_start + i as u16;

            let style = if self.has_focus && i == self.selected_index {
                Style::default().add_modifier(Modifier::REVERSED)
            } else if self.hover_index == Some(i) {
                // Subtle hover background
                Style::default().bg(self.hover_color)
            } else if i == self.active_index {
                Style::default().fg(self.active_color)
            } else {
                Style::default()
            };

            // Pad label to full width for background color to fill the line
            let width = inner.width as usize;
            let label = format!(" {:<width$}", item, width = width.saturating_sub(1));
            buf.set_string(inner.x, y, &label, style);
        }
    }
}
