use ratatui::{buffer::Buffer, layout::Rect, style::Color};

/// Effects draw RGB pixels; this transport renders two pixels per terminal cell.
/// Resolution is bounded so wide monitors cannot create unbounded effect work.
#[derive(Default)]
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pixels: Vec<[u8; 3]>,
}
impl Canvas {
    pub fn resize(&mut self, columns: u16, rows: u16) {
        self.width = usize::from(columns).min(240);
        self.height = usize::from(rows).min(64) * 2;
        self.pixels.resize(self.width * self.height, [0; 3]);
    }
    pub fn clear(&mut self) {
        self.pixels.fill([0; 3]);
    }
    pub fn set(&mut self, x: usize, y: usize, color: [u8; 3]) {
        if x < self.width && y < self.height {
            self.pixels[y * self.width + x] = color;
        }
    }
    pub fn pixels(&self) -> &[[u8; 3]] {
        &self.pixels
    }
    pub fn present(&self, area: Rect, buffer: &mut Buffer) {
        if self.width == 0 || self.height == 0 || area.width == 0 || area.height == 0 {
            return;
        }
        for row in 0..area.height {
            for col in 0..area.width {
                let x = usize::from(col) * self.width / usize::from(area.width);
                let y = usize::from(row) * self.height / usize::from(area.height);
                let top = self.pixels[y * self.width + x];
                let bottom_y =
                    (usize::from(row) * 2 + 1) * self.height / (usize::from(area.height) * 2);
                let bottom = self.pixels[bottom_y.min(self.height - 1) * self.width + x];
                if let Some(cell) = buffer.cell_mut((area.x + col, area.y + row)) {
                    cell.set_symbol("▀")
                        .set_fg(Color::Rgb(top[0], top[1], top[2]))
                        .set_bg(Color::Rgb(bottom[0], bottom[1], bottom[2]));
                }
            }
        }
    }
}

pub fn palette(colors: &[[u8; 3]]) -> [[u8; 3]; 256] {
    std::array::from_fn(|i| {
        let position = i as f32 / 255.0 * (colors.len() - 1) as f32;
        let low = (position.floor() as usize).min(colors.len() - 1);
        let high = (low + 1).min(colors.len() - 1);
        let f = position - low as f32;
        std::array::from_fn(|c| {
            (colors[low][c] as f32 * (1.0 - f) + colors[high][c] as f32 * f) as u8
        })
    })
}
pub fn brightness(color: [u8; 3], amount: f32) -> [u8; 3] {
    color.map(|c| (c as f32 * amount.clamp(0.0, 1.5)).min(255.0) as u8)
}
