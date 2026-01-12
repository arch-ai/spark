//! Help bar widget showing keyboard shortcuts

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

/// A segment of help text with optional styling
#[derive(Clone)]
pub struct HelpItem {
    pub text: &'static str,
    pub is_key: bool,
    pub is_highlighted: bool,
}

impl HelpItem {
    pub fn plain(text: &'static str) -> Self {
        Self {
            text,
            is_key: false,
            is_highlighted: false,
        }
    }

    pub fn key(text: &'static str) -> Self {
        Self {
            text,
            is_key: true,
            is_highlighted: false,
        }
    }

}

/// Help bar widget showing keyboard shortcuts at the bottom of the screen
pub struct HelpBar<'a> {
    rows: Vec<Vec<HelpItem>>,
    key_color: Color,
    highlight_fg: Color,
    highlight_bg: Color,
    _phantom: std::marker::PhantomData<&'a ()>,
}

impl<'a> HelpBar<'a> {
    pub fn new(rows: Vec<Vec<HelpItem>>) -> Self {
        Self {
            rows,
            key_color: Color::Cyan,
            highlight_fg: Color::Black,
            highlight_bg: Color::Yellow,
            _phantom: std::marker::PhantomData,
        }
    }

    fn build_line(&self, items: &[HelpItem]) -> Line<'static> {
        let spans: Vec<Span> = items
            .iter()
            .map(|item| {
                let style = if item.is_highlighted {
                    Style::default()
                        .fg(self.highlight_fg)
                        .bg(self.highlight_bg)
                } else if item.is_key {
                    Style::default().fg(self.key_color)
                } else {
                    Style::default()
                };
                Span::styled(item.text.to_string(), style)
            })
            .collect();
        Line::from(spans)
    }
}

impl Widget for HelpBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let lines: Vec<Line> = self.rows.iter().map(|row| self.build_line(row)).collect();
        let paragraph = Paragraph::new(lines);
        paragraph.render(area, buf);
    }
}
