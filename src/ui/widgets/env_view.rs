//! Environment variables view widget (ratatui implementation)

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Widget},
};

/// Environment variables view for containers or processes
pub struct EnvView<'a> {
    title: &'a str,
    info_left1: &'a str,
    info_right1: &'a str,
    info_left2: &'a str,
    info_right2: &'a str,
    env_vars: &'a [String],
    selected_index: usize,
    scroll_offset: usize,
}

impl<'a> EnvView<'a> {
    pub fn new(
        title: &'a str,
        env_vars: &'a [String],
    ) -> Self {
        Self {
            title,
            info_left1: "",
            info_right1: "",
            info_left2: "",
            info_right2: "",
            env_vars,
            selected_index: 0,
            scroll_offset: 0,
        }
    }

    pub fn info(mut self, left1: &'a str, right1: &'a str, left2: &'a str, right2: &'a str) -> Self {
        self.info_left1 = left1;
        self.info_right1 = right1;
        self.info_left2 = left2;
        self.info_right2 = right2;
        self
    }

    pub fn selected(mut self, index: usize) -> Self {
        self.selected_index = index;
        self
    }

    fn split_env(line: &str) -> (&str, &str) {
        line.split_once('=').unwrap_or((line, ""))
    }
}

impl Widget for EnvView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 10 || area.width < 20 {
            return;
        }

        // Main layout: title, info, env table, help
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),  // Title
                Constraint::Length(5),  // Info box
                Constraint::Min(5),     // Env table
                Constraint::Length(3),  // Help bar
            ])
            .split(area);

        // Title block
        let title_block = Block::default()
            .borders(Borders::ALL)
            .title(self.title);
        title_block.render(chunks[0], buf);

        // Info table
        let info_rows = vec![
            Row::new(vec![
                Cell::from(self.info_left1),
                Cell::from(self.info_right1),
            ]),
            Row::new(vec![
                Cell::from(self.info_left2),
                Cell::from(self.info_right2),
            ]),
        ];
        let info_table = Table::new(
            info_rows,
            [Constraint::Percentage(50), Constraint::Percentage(50)],
        )
        .block(Block::default().borders(Borders::ALL));
        info_table.render(chunks[1], buf);

        // Environment variables table
        let env_rows: Vec<Row> = self.env_vars
            .iter()
            .skip(self.scroll_offset)
            .enumerate()
            .map(|(i, line)| {
                let actual_index = self.scroll_offset + i;
                let (key, value) = Self::split_env(line);
                let style = if actual_index == self.selected_index {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                };
                Row::new(vec![
                    Cell::from(key),
                    Cell::from(value),
                ])
                .style(style)
            })
            .collect();

        let env_table = Table::new(
            env_rows,
            [Constraint::Percentage(30), Constraint::Percentage(70)],
        )
        .header(
            Row::new(vec![
                Cell::from("KEY").style(Style::default().add_modifier(Modifier::BOLD)),
                Cell::from("VALUE").style(Style::default().add_modifier(Modifier::BOLD)),
            ])
        )
        .block(Block::default().borders(Borders::ALL).title("Environment"));
        env_table.render(chunks[2], buf);

        // Help bar
        let help_text = Line::from(vec![
            Span::raw("Actions: "),
            Span::styled("Esc", Style::default().fg(Color::Cyan)),
            Span::raw(" back"),
        ]);
        let help = Paragraph::new(help_text)
            .block(Block::default().borders(Borders::ALL));
        help.render(chunks[3], buf);
    }
}
