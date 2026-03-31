// SPDX-License-Identifier: GPL-3.0-only

// ── Standalone geometry helpers (testable without SDL2) ───────────────────

/// Flip an OS/2 Y coordinate (bottom-left origin) to screen Y (top-left origin).
pub fn flip_y(y: i32, height: u32) -> i32 {
    (height as i32 - 1) - y
}

/// Compute the screen Y for a font glyph row, given OS/2 baseline Y.
/// Font row 0 is the top of the glyph; OS/2 Y is bottom-left origin.
pub fn text_screen_y(y: i32, row: i32, char_h: i32, height: u32) -> i32 {
    height as i32 - y - char_h + row
}

/// Map a font character to its glyph index (0 = space for unknown/control chars).
pub fn glyph_index(ch: char) -> usize {
    let c = ch as u32;
    if (32..=126).contains(&c) { (c - 32) as usize } else { 0 }
}

/// Render text into a raw pixel buffer (no SDL2 dependency).
/// `width`/`height` are buffer dimensions; coordinates are OS/2 bottom-left origin.
pub fn render_text_to_buffer(buf: &mut [u32], width: u32, height: u32, x: i32, y: i32, text: &str, color: u32) {
    if width == 0 || height == 0 { return; }
    let char_w: i32 = 8;
    let char_h: i32 = 16;
    for (i, ch) in text.chars().enumerate() {
        let cx = x + (i as i32 * char_w);
        let gi = glyph_index(ch);
        for row in 0..char_h {
            let bits = crate::font8x16::FONT_8X16[gi * 16 + row as usize];
            for col in 0..char_w {
                if bits & (0x80 >> col) != 0 {
                    let px = cx + col;
                    let py = text_screen_y(y, row, char_h, height);
                    if px >= 0 && px < width as i32 && py >= 0 && py < height as i32 {
                        buf[(py as u32 * width + px as u32) as usize] = color;
                    }
                }
            }
        }
    }
}

/// Draw a filled or outlined rectangle into a raw pixel buffer.
/// Coordinates are OS/2 bottom-left origin.
#[allow(clippy::too_many_arguments)]
pub fn render_rect_to_buffer(buf: &mut [u32], width: u32, height: u32,
                              x1: i32, y1: i32, x2: i32, y2: i32, color: u32, fill: bool) {
    if width == 0 || height == 0 { return; }
    let left = x1.min(x2).max(0).min(width as i32 - 1) as u32;
    let right = x1.max(x2).max(0).min(width as i32 - 1) as u32;
    let bottom = y1.min(y2).max(0).min(height as i32 - 1) as u32;
    let top = y1.max(y2).max(0).min(height as i32 - 1) as u32;
    let top_y = (height - 1) - bottom;
    let bottom_y = (height - 1) - top;

    if fill {
        for y in bottom_y..=top_y {
            for x in left..=right {
                let idx = (y * width + x) as usize;
                if idx < buf.len() { buf[idx] = color; }
            }
        }
    } else {
        for x in left..=right {
            let idx_b = (bottom_y * width + x) as usize;
            let idx_t = (top_y * width + x) as usize;
            if idx_b < buf.len() { buf[idx_b] = color; }
            if idx_t < buf.len() { buf[idx_t] = color; }
        }
        for y in bottom_y..=top_y {
            let idx_l = (y * width + left) as usize;
            let idx_r = (y * width + right) as usize;
            if idx_l < buf.len() { buf[idx_l] = color; }
            if idx_r < buf.len() { buf[idx_r] = color; }
        }
    }
}

/// Draw a line into a raw pixel buffer using Bresenham's algorithm.
/// Coordinates are OS/2 bottom-left origin.
#[allow(clippy::too_many_arguments)]
pub fn render_line_to_buffer(buf: &mut [u32], width: u32, height: u32,
                              x1: i32, y1: i32, x2: i32, y2: i32, color: u32) {
    if width == 0 || height == 0 { return; }
    let flip = |y: i32| -> i32 { (height as i32 - 1) - y };
    let mut x = x1;
    let mut y = y1;
    let dx = (x2 - x1).abs();
    let dy = (y2 - y1).abs();
    let sx = if x1 < x2 { 1 } else { -1 };
    let sy = if y1 < y2 { 1 } else { -1 };
    let mut err = dx - dy;

    loop {
        let fy = flip(y);
        if x >= 0 && x < width as i32 && fy >= 0 && fy < height as i32 {
            buf[(fy as u32 * width + x as u32) as usize] = color;
        }
        if x == x2 && y == y2 { break; }
        let e2 = 2 * err;
        if e2 > -dy { err -= dy; x += sx; }
        if e2 < dx { err += dx; y += sy; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flip_y_origin() {
        assert_eq!(flip_y(0, 480), 479);
        assert_eq!(flip_y(479, 480), 0);
        assert_eq!(flip_y(239, 480), 240);
    }

    #[test]
    fn test_text_screen_y_not_inverted() {
        let height = 480u32;
        let char_h = 16i32;
        let y = 100i32;
        let top_row_y = text_screen_y(y, 0, char_h, height);
        let bot_row_y = text_screen_y(y, 15, char_h, height);
        assert!(top_row_y < bot_row_y, "font row 0 must be above row 15 on screen");
        assert_eq!(bot_row_y - top_row_y, 15);
    }

    #[test]
    fn test_text_screen_y_values() {
        assert_eq!(text_screen_y(100, 0, 16, 480), 364);
        assert_eq!(text_screen_y(100, 15, 16, 480), 379);
    }

    #[test]
    fn test_glyph_index() {
        assert_eq!(glyph_index(' '), 0);
        assert_eq!(glyph_index('!'), 1);
        assert_eq!(glyph_index('A'), 33);
        assert_eq!(glyph_index('~'), 94);
        assert_eq!(glyph_index('\n'), 0);
        assert_eq!(glyph_index('\x01'), 0);
    }

    #[test]
    fn test_render_text_writes_pixels() {
        let (w, h) = (80u32, 32u32);
        let mut buf = vec![0u32; (w * h) as usize];
        render_text_to_buffer(&mut buf, w, h, 0, 0, "A", 0xFF0000);
        let colored = buf.iter().filter(|&&p| p == 0xFF0000).count();
        assert!(colored > 0, "rendering 'A' should produce colored pixels");
    }

    #[test]
    fn test_render_text_right_side_up() {
        let (w, h) = (16u32, 32u32);
        let mut buf = vec![0u32; (w * h) as usize];
        let color = 0xFFFFFF;
        render_text_to_buffer(&mut buf, w, h, 0, 8, "T", color);

        let mut top_row = h as i32;
        let mut bot_row = -1i32;
        for py in 0..h as i32 {
            for px in 0..8 {
                if buf[(py as u32 * w + px as u32) as usize] == color {
                    if py < top_row { top_row = py; }
                    if py > bot_row { bot_row = py; }
                }
            }
        }
        assert!(top_row < bot_row);

        let count_at = |row: i32| -> usize {
            (0..8).filter(|&px| buf[(row as u32 * w + px as u32) as usize] == color).count()
        };
        let top_count = count_at(top_row);
        let mid_count = count_at((top_row + bot_row) / 2);
        assert!(top_count > mid_count,
            "top row should have more pixels (bar of T) than middle (stem): top={}, mid={}", top_count, mid_count);
    }

    #[test]
    fn test_filled_rect() {
        let (w, h) = (20u32, 20u32);
        let mut buf = vec![0u32; (w * h) as usize];
        let color = 0xFF0000;
        render_rect_to_buffer(&mut buf, w, h, 5, 5, 14, 14, color, true);
        let colored = buf.iter().filter(|&&p| p == color).count();
        assert_eq!(colored, 100);
    }

    #[test]
    fn test_outline_rect() {
        let (w, h) = (20u32, 20u32);
        let mut buf = vec![0u32; (w * h) as usize];
        let color = 0x00FF00;
        render_rect_to_buffer(&mut buf, w, h, 5, 5, 14, 14, color, false);
        let colored = buf.iter().filter(|&&p| p == color).count();
        assert_eq!(colored, 36);
    }

    #[test]
    fn test_line_horizontal() {
        let (w, h) = (20u32, 10u32);
        let mut buf = vec![0u32; (w * h) as usize];
        let color = 0x0000FF;
        render_line_to_buffer(&mut buf, w, h, 0, 5, 19, 5, color);
        let colored = buf.iter().filter(|&&p| p == color).count();
        assert_eq!(colored, 20);
    }

    #[test]
    fn test_line_vertical() {
        let (w, h) = (10u32, 20u32);
        let mut buf = vec![0u32; (w * h) as usize];
        let color = 0x0000FF;
        render_line_to_buffer(&mut buf, w, h, 5, 0, 5, 19, color);
        let colored = buf.iter().filter(|&&p| p == color).count();
        assert_eq!(colored, 20);
    }

    #[test]
    fn test_line_diagonal() {
        let (w, h) = (10u32, 10u32);
        let mut buf = vec![0u32; (w * h) as usize];
        let color = 0x0000FF;
        render_line_to_buffer(&mut buf, w, h, 0, 0, 9, 9, color);
        let colored = buf.iter().filter(|&&p| p == color).count();
        assert_eq!(colored, 10);
    }

    #[test]
    fn test_rect_y_flip() {
        let (w, h) = (10u32, 10u32);
        let mut buf = vec![0u32; (w * h) as usize];
        let color = 0xFF0000;
        render_rect_to_buffer(&mut buf, w, h, 0, 0, 9, 0, color, true);
        for x in 0..10 {
            assert_eq!(buf[9 * 10 + x], color, "pixel at screen row 9 (OS/2 y=0) should be set");
        }
        for x in 0..10 {
            assert_eq!(buf[0 * 10 + x], 0, "pixel at screen row 0 should be empty");
        }
    }
}
