//! An in-memory RGBA surface — the substrate for pixel-exact golden tests
//! (brief §7). Needs no display, so CI and the fixture suite never touch fbdev
//! or X. P7 blesses renders produced through this; P9 shares its pixel ops.

use super::{Color, Rect, Surface, TextRun};

/// A flat RGBA8 framebuffer in memory.
#[derive(Debug, Clone)]
pub struct MemSurface {
    width: u32,
    height: u32,
    /// `width * height * 4` bytes, row-major, RGBA.
    pixels: Vec<u8>,
}

impl MemSurface {
    /// A new surface filled with `bg`.
    pub fn new(width: u32, height: u32, bg: Color) -> Self {
        let mut s = MemSurface {
            width,
            height,
            pixels: vec![0; (width as usize) * (height as usize) * 4],
        };
        s.fill_rect(
            Rect { x: 0, y: 0, w: width, h: height },
            bg,
        );
        s
    }

    /// The raw RGBA8 bytes (for encoding a golden PNG).
    pub fn bytes(&self) -> &[u8] {
        &self.pixels
    }

    #[inline]
    fn in_bounds(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && (x as u32) < self.width && (y as u32) < self.height
    }

    #[inline]
    fn index(&self, x: i32, y: i32) -> usize {
        ((y as usize) * (self.width as usize) + (x as usize)) * 4
    }
}

impl Surface for MemSurface {
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn put_pixel(&mut self, x: i32, y: i32, color: Color) {
        if !self.in_bounds(x, y) {
            return;
        }
        let i = self.index(x, y);
        // Straight-alpha source-over. Opaque is the common path; blend the rest.
        if color.a == 255 {
            self.pixels[i] = color.r;
            self.pixels[i + 1] = color.g;
            self.pixels[i + 2] = color.b;
            self.pixels[i + 3] = 255;
        } else if color.a != 0 {
            let sa = color.a as u32;
            let ia = 255 - sa;
            let blend = |s: u8, d: u8| ((s as u32 * sa + d as u32 * ia) / 255) as u8;
            self.pixels[i] = blend(color.r, self.pixels[i]);
            self.pixels[i + 1] = blend(color.g, self.pixels[i + 1]);
            self.pixels[i + 2] = blend(color.b, self.pixels[i + 2]);
            self.pixels[i + 3] = self.pixels[i + 3].max(color.a);
        }
    }

    fn fill_rect(&mut self, rect: Rect, color: Color) {
        let x0 = rect.x.max(0);
        let y0 = rect.y.max(0);
        let x1 = (rect.x + rect.w as i32).min(self.width as i32);
        let y1 = (rect.y + rect.h as i32).min(self.height as i32);
        for y in y0..y1 {
            for x in x0..x1 {
                self.put_pixel(x, y, color);
            }
        }
    }

    fn blit(&mut self, _at: Rect, _image: &crate::img::RgbaImage) {
        todo!("P9: image blit into the mem surface")
    }

    fn draw_text(&mut self, _run: &TextRun) {
        todo!("P5/P7: rasterize a text run via font metrics")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_and_readback_is_opaque() {
        let mut s = MemSurface::new(2, 2, Color::WHITE);
        s.fill_rect(Rect { x: 0, y: 0, w: 1, h: 1 }, Color::BLACK);
        // top-left black, its neighbor still white
        assert_eq!(&s.bytes()[0..4], &[0, 0, 0, 255]);
        assert_eq!(&s.bytes()[4..8], &[255, 255, 255, 255]);
    }

    #[test]
    fn out_of_bounds_pixels_are_ignored() {
        let mut s = MemSurface::new(1, 1, Color::WHITE);
        s.put_pixel(-1, 0, Color::BLACK);
        s.put_pixel(0, 5, Color::BLACK);
        assert_eq!(&s.bytes()[0..4], &[255, 255, 255, 255]);
    }
}
