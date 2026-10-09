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
    logo_frame: u8,
}

impl<'a> Sidebar<'a> {
    pub const LOGO_FRAME_COUNT: u8 = 12;

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
            logo_frame: 0,
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
    pub fn logo_frame(mut self, frame: u8) -> Self {
        self.logo_frame = frame;
        self
    }
}

impl Widget for Sidebar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 3 || area.height < 5 {
            return;
        }

        // Draw the outer block with title
        let block = Block::default().borders(Borders::ALL).title(self.title);
        let inner = block.inner(area);
        block.render(area, buf);

        let logo = spark_logo(self.logo_frame);
        let logo_height =
            crate::ui::layout::sidebar_menu_start(area.height).saturating_sub(2) as usize;
        let logo_x = inner.x + inner.width.saturating_sub(17) / 2;
        for (i, row) in logo.iter().take(logo_height).enumerate() {
            let y = inner.y + i as u16;
            if y < inner.y + inner.height {
                for (x, &(symbol, depth)) in row.iter().take(inner.width as usize).enumerate() {
                    let style = if depth > 0.4 {
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else if depth > -0.2 {
                        Style::default().fg(Color::Gray)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    buf[(logo_x + x as u16, y)]
                        .set_char(symbol)
                        .set_style(style);
                }
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

/// Project a rotating, faceted spark into a fixed ASCII canvas.
fn spark_logo(frame: u8) -> [[(char, f32); 17]; 7] {
    let angle = std::f32::consts::FRAC_PI_8
        + f32::from(frame % Sidebar::LOGO_FRAME_COUNT) * std::f32::consts::TAU
            / f32::from(Sidebar::LOGO_FRAME_COUNT);
    let (sin_yaw, cos_yaw) = angle.sin_cos();
    let (sin_tilt, cos_tilt) = 0.32_f32.sin_cos();
    let rotate = |(x, y, z): (f32, f32, f32)| {
        let (x, z) = (x * cos_yaw + z * sin_yaw, -x * sin_yaw + z * cos_yaw);
        (x, y * cos_tilt - z * sin_tilt, y * sin_tilt + z * cos_tilt)
    };
    let vertices = [
        (1.0, 0.0, 0.0),
        (-1.0, 0.0, 0.0),
        (0.0, 1.0, 0.0),
        (0.0, -1.0, 0.0),
        (0.0, 0.0, 1.0),
        (0.0, 0.0, -1.0),
    ];
    let points = vertices.map(|vertex| {
        let (x, y, z) = rotate(vertex);
        let perspective = 4.2 / (4.2 - z);
        (8.0 + x * perspective * 6.0, 3.0 - y * perspective * 2.6, z)
    });
    // Cull rear faces so the silhouette and near facets remain readable.
    let mut edges = [[false; 6]; 6];
    for x in 0..2 {
        for y in 0..2 {
            for z in 0..2 {
                let normal = rotate((
                    1.0 - 2.0 * x as f32,
                    1.0 - 2.0 * y as f32,
                    1.0 - 2.0 * z as f32,
                ));
                if normal.2 * 4.2 <= 1.0 {
                    continue;
                }
                for (a, b) in [(x, y + 2), (y + 2, z + 4), (x, z + 4)] {
                    edges[a][b] = true;
                }
            }
        }
    }
    let mut canvas = [[(' ', f32::NEG_INFINITY); 17]; 7];
    let mut plot = |x: f32, y: f32, depth: f32, symbol: char| {
        let (x, y) = (x.round() as i32, y.round() as i32);
        if (0..17).contains(&x) && (0..7).contains(&y) {
            let cell = &mut canvas[y as usize][x as usize];
            if depth >= cell.1 {
                *cell = (symbol, depth);
            }
        }
    };
    for a in 0..6 {
        for b in a + 1..6 {
            if !edges[a][b] {
                continue;
            }
            let (x, y, z) = points[a];
            let (dx, dy, dz) = (points[b].0 - x, points[b].1 - y, points[b].2 - z);
            let symbol = if dx.abs() < dy.abs() * 0.4 {
                '|'
            } else if dy.abs() < dx.abs() * 0.4 {
                '-'
            } else if dx * dy > 0.0 {
                '\\'
            } else {
                '/'
            };
            let steps = dx.abs().max(dy.abs()).ceil().max(1.0) as u32;
            for step in 0..=steps {
                let t = step as f32 / steps as f32;
                plot(x + dx * t, y + dy * t, z + dz * t, symbol);
            }
            plot(x, y, z + 0.01, '+');
            plot(points[b].0, points[b].1, points[b].2 + 0.01, '+');
        }
    }
    canvas[3][8] = ('*', f32::INFINITY);
    canvas
}
