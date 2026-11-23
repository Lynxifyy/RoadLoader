//! Software renderer for the companion process.
//!
//! In a full deployment the game's `AppPlatform::swapBuffers` hook hands us the
//! real framebuffer through [`crate::ipc::region::Region::begin_frame`]. Until a
//! 32-bit `libminecraftpe.so` is attached we render a deterministic test scene:
//! it proves the whole frame path (shared memory -> UI SurfaceView) end to end
//! and gives the mobile test a moving, verifiable picture.

use core::time::Duration;

use crate::util::monotonic_ns;

/// `AABBGGRR` little-endian, i.e. bytes R, G, B, A in memory.
const fn rgba(r: u8, g: u8, b: u8, a: u8) -> u32 {
    (a as u32) << 24 | (b as u32) << 16 | (g as u32) << 8 | r as u32
}

const BACKGROUND_TOP: u32 = rgba(12, 18, 34, 255);
const BACKGROUND_BOTTOM: u32 = rgba(28, 64, 74, 255);
const GRID: u32 = rgba(255, 255, 255, 18);
const ACCENT: u32 = rgba(94, 234, 212, 255);
const ACCENT_DIM: u32 = rgba(45, 120, 112, 255);
const CURSOR: u32 = rgba(255, 176, 64, 255);
const CURSOR_RING: u32 = rgba(255, 255, 255, 220);
const OK: u32 = rgba(74, 222, 128, 255);
const FAIL: u32 = rgba(248, 113, 113, 255);
const WHITE: u32 = rgba(245, 245, 245, 255);

/// State of the companion-side test scene.
pub struct Scene {
    width: u32,
    height: u32,
    started_ns: u64,
    frame: u64,
    cursor_x: f32,
    cursor_y: f32,
    cursor_active: bool,
    self_test: u32,
    safe_mode: bool,
}

impl Scene {
    pub fn new(width: u32, height: u32, safe_mode: bool) -> Self {
        Self {
            width,
            height,
            started_ns: monotonic_ns(),
            frame: 0,
            cursor_x: width as f32 * 0.5,
            cursor_y: height as f32 * 0.5,
            cursor_active: false,
            self_test: 0,
            safe_mode,
        }
    }

    pub fn set_cursor(&mut self, x: f32, y: f32, active: bool) {
        self.cursor_x = x.clamp(0.0, self.width as f32 - 1.0);
        self.cursor_y = y.clamp(0.0, self.height as f32 - 1.0);
        self.cursor_active = active;
    }

    pub fn set_self_test(&mut self, mask: u32) {
        self.self_test = mask;
    }

    /// Enables the reduced-effects path used by "Safe Mode".
    pub fn set_safe_mode(&mut self, enabled: bool) {
        self.safe_mode = enabled;
    }

    /// Renders one frame into `buf` and returns the time it took.
    pub fn render(&mut self, buf: &mut [u8]) -> Duration {
        let started = monotonic_ns();
        let width = self.width as usize;
        let height = self.height as usize;
        let needed = width * height * 4;
        if buf.len() < needed {
            return Duration::ZERO;
        }
        // SAFETY: `buf` is at least width*height*4 bytes and `u32` has no
        // alignment requirement beyond 4, which the allocator guarantees for
        // the frame slot (4096-byte aligned inside the mapping).
        let pixels = unsafe {
            core::slice::from_raw_parts_mut(buf.as_mut_ptr() as *mut u32, width * height)
        };

        self.frame += 1;
        let elapsed = (monotonic_ns().saturating_sub(self.started_ns)) as f32 / 1e9;
        self.draw_background(pixels);
        self.draw_grid(pixels);
        self.draw_sweep(pixels, elapsed);
        self.draw_status_bars(pixels);
        self.draw_crosshair(pixels);
        if self.cursor_active {
            self.draw_cursor(pixels);
        }
        let took = Duration::from_nanos(monotonic_ns().saturating_sub(started));
        took
    }

    fn draw_background(&self, pixels: &mut [u32]) {
        let width = self.width as usize;
        let height = self.height as usize;
        for y in 0..height {
            let t = y as f32 / (height.max(2) - 1) as f32;
            let color = lerp_rgb(BACKGROUND_TOP, BACKGROUND_BOTTOM, t);
            pixels[y * width..(y + 1) * width].fill(color);
        }
    }

    fn draw_grid(&self, pixels: &mut [u32]) {
        let width = self.width as usize;
        let height = self.height as usize;
        let step = (height / 9).max(24);
        for y in (0..height).step_by(step) {
            let row = y * width;
            for x in 0..width {
                pixels[row + x] = blend(pixels[row + x], GRID);
            }
        }
        for x in (0..width).step_by(step) {
            for y in 0..height {
                let index = y * width + x;
                pixels[index] = blend(pixels[index], GRID);
            }
        }
    }

    /// A bright band moving diagonally; makes dropped/duplicated frames obvious.
    fn draw_sweep(&self, pixels: &mut [u32], elapsed: f32) {
        let width = self.width as i32;
        let height = self.height as i32;
        let band_width = (height / 14).max(18);
        let travel = (width + height) as f32;
        let offset = ((elapsed * 320.0) % travel) as i32;
        for y in 0..height {
            let start = (offset - y).rem_euclid(travel as i32) - band_width;
            for x in (start.max(0))..(start + band_width).min(width) {
                let index = (y * width + x) as usize;
                pixels[index] = blend(pixels[index], ACCENT_DIM);
            }
        }
    }

    fn draw_status_bars(&self, pixels: &mut [u32]) {
        let width = self.width as usize;
        let height = self.height as usize;
        let bar_height = (height / 40).max(6);
        let bar_width = (width / 6).max(40);
        let margin = (height / 40).max(8) as i32;
        // Self-test indicator: green when every subsystem reported success.
        let ok = self.self_test & 0x8000_0000 != 0;
        let color = if self.self_test == 0 { ACCENT_DIM } else if ok { OK } else { FAIL };
        self.fill_rect(pixels, margin, margin, bar_width as i32, bar_height as i32, color);
        // Safe mode indicator.
        if self.safe_mode {
            self.fill_rect(
                pixels,
                margin,
                margin + bar_height as i32 + margin,
                bar_width as i32,
                bar_height as i32,
                CURSOR,
            );
        }
        // Frame heartbeat pulse in the top-right corner.
        let pulse = (self.frame % 60) as f32 / 60.0;
        let size = ((height / 30) as f32 * (0.4 + 0.6 * pulse)) as i32;
        self.fill_rect(
            pixels,
            width as i32 - margin - size,
            margin,
            size,
            size,
            WHITE,
        );
    }

    fn draw_crosshair(&self, pixels: &mut [u32]) {
        let cx = self.width as i32 / 2;
        let cy = self.height as i32 / 2;
        let arm = (self.height / 24).max(10) as i32;
        let thickness = (self.height / 180).max(2) as i32;
        self.fill_rect(pixels, cx - arm, cy - thickness / 2, arm * 2, thickness, WHITE);
        self.fill_rect(pixels, cx - thickness / 2, cy - arm, thickness, arm * 2, WHITE);
    }

    fn draw_cursor(&self, pixels: &mut [u32]) {
        let cx = self.cursor_x as i32;
        let cy = self.cursor_y as i32;
        let radius = (self.height / 22).max(12) as i32;
        for y in -radius..=radius {
            for x in -radius..=radius {
                let distance_sq = x * x + y * y;
                if distance_sq > radius * radius {
                    continue;
                }
                let px = cx + x;
                let py = cy + y;
                if px < 0 || py < 0 || px >= self.width as i32 || py >= self.height as i32 {
                    continue;
                }
                let index = (py * self.width as i32 + px) as usize;
                let ring = distance_sq > (radius - 3).max(1) * (radius - 3).max(1);
                pixels[index] = blend(pixels[index], if ring { CURSOR_RING } else { CURSOR });
            }
        }
    }

    fn fill_rect(&self, pixels: &mut [u32], x: i32, y: i32, w: i32, h: i32, color: u32) {
        let width = self.width as i32;
        let height = self.height as i32;
        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + w).min(width);
        let y1 = (y + h).min(height);
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        for py in y0..y1 {
            let row = (py * width) as usize;
            for px in x0..x1 {
                let index = row + px as usize;
                pixels[index] = blend(pixels[index], color);
            }
        }
    }
}

fn lerp_rgb(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let channel = |shift: u32| -> u32 {
        let ca = ((a >> shift) & 0xFF) as f32;
        let cb = ((b >> shift) & 0xFF) as f32;
        ((ca + (cb - ca) * t) as u32) & 0xFF
    };
    (0xFF << 24) | (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

/// Alpha-blends `over` (whose alpha is its byte 3) onto `under`.
fn blend(under: u32, over: u32) -> u32 {
    let alpha = (over >> 24) & 0xFF;
    if alpha == 0 {
        return under;
    }
    if alpha == 0xFF {
        return over | 0xFF00_0000;
    }
    let mix = |shift: u32| -> u32 {
        let u = (under >> shift) & 0xFF;
        let o = (over >> shift) & 0xFF;
        ((o * alpha + u * (255 - alpha)) / 255) & 0xFF
    };
    (0xFF << 24) | (mix(16) << 16) | (mix(8) << 8) | mix(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_into_rgba_buffer() {
        let mut scene = Scene::new(64, 48, false);
        let mut buffer = vec![0u8; 64 * 48 * 4];
        let took = scene.render(&mut buffer);
        assert!(took.as_nanos() > 0);
        // Background is opaque everywhere.
        assert!(buffer.chunks_exact(4).all(|pixel| pixel[3] == 255));
        // The frame is not uniformly black.
        assert!(buffer.chunks_exact(4).any(|pixel| pixel[0] > 8 || pixel[1] > 8));
    }

    #[test]
    fn cursor_is_clamped_into_the_frame() {
        let mut scene = Scene::new(64, 48, false);
        scene.set_cursor(-100.0, 1e9, true);
        let mut buffer = vec![0u8; 64 * 48 * 4];
        scene.render(&mut buffer);
        assert_eq!(scene.cursor_x, 0.0);
        assert_eq!(scene.cursor_y, 47.0);
    }

    #[test]
    fn rejects_short_buffers() {
        let mut scene = Scene::new(64, 48, false);
        let mut buffer = vec![0u8; 16];
        assert_eq!(scene.render(&mut buffer), Duration::ZERO);
    }

    #[test]
    fn blend_is_transparent_safe() {
        assert_eq!(blend(0x11223344, 0x00000000), 0x11223344);
        assert_eq!(blend(0x11223344, 0xFF00FF00), 0xFF00FF00);
    }
}
