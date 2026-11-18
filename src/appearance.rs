#![allow(unsafe_op_in_unsafe_fn)]
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Gdi::*, GdiPlus::*},
};
pub struct DrawingSession(usize);
impl Drop for DrawingSession {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe {
                GdiplusShutdown(self.0);
            }
        }
    }
}
pub fn start() -> DrawingSession {
    let mut token = 0;
    unsafe {
        GdiplusStartup(
            &mut token,
            &GdiplusStartupInput {
                GdiplusVersion: 1,
                ..Default::default()
            },
            std::ptr::null_mut(),
        );
    }
    DrawingSession(token)
}
pub const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    r | (g << 8) | (b << 16)
}
pub const PAPER: u32 = rgb(250, 250, 248);
pub const INPUT: u32 = rgb(239, 240, 237);
pub const INK: u32 = rgb(39, 43, 42);
pub const MUTED: u32 = rgb(112, 118, 116);
pub const LINE: u32 = rgb(227, 230, 225);
pub const SELECT: u32 = rgb(231, 237, 233);
pub const ACCENT: u32 = rgb(63, 104, 88);
pub const WHITE: u32 = rgb(255, 255, 255);
pub fn category_colors(index: usize) -> (u32, u32) {
    match index {
        0 => (rgb(67, 99, 148), rgb(230, 238, 250)),
        1 => (rgb(108, 80, 148), rgb(239, 233, 248)),
        2 => (rgb(132, 96, 41), rgb(247, 238, 220)),
        3 => (rgb(50, 111, 91), rgb(226, 239, 232)),
        _ => (rgb(108, 115, 110), rgb(235, 238, 233)),
    }
}
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
pub struct Painter {
    pub dc: HDC,
    pub scale: f64,
}
impl Painter {
    pub fn px(&self, n: i32) -> i32 {
        (n as f64 * self.scale).round() as i32
    }
    pub fn rect(&self, x: i32, y: i32, w: i32, h: i32) -> RECT {
        RECT {
            left: self.px(x),
            top: self.px(y),
            right: self.px(x + w),
            bottom: self.px(y + h),
        }
    }
    pub unsafe fn fill(&self, x: i32, y: i32, w: i32, h: i32, color: u32) {
        SetDCBrushColor(self.dc, color);
        FillRect(
            self.dc,
            &self.rect(x, y, w, h),
            GetStockObject(DC_BRUSH) as HBRUSH,
        );
    }
    pub unsafe fn round(&self, x: i32, y: i32, w: i32, h: i32, radius: i32, color: u32) {
        let mut graphics = std::ptr::null_mut();
        if GdipCreateFromHDC(self.dc, &mut graphics) == 0 {
            let mut path = std::ptr::null_mut();
            let mut fill = std::ptr::null_mut();
            let argb =
                0xff000000 | ((color & 255) << 16) | (color & 0xff00) | ((color >> 16) & 255);
            GdipSetSmoothingMode(graphics, SmoothingModeAntiAlias);
            GdipCreatePath(FillModeAlternate, &mut path);
            GdipCreateSolidFill(argb, &mut fill);
            if !path.is_null() && !fill.is_null() {
                let diameter = self.px(radius * 2);
                let left = self.px(x);
                let top = self.px(y);
                let right = self.px(x + w);
                let bottom = self.px(y + h);
                for (xx, yy, angle) in [
                    (left, top, 180.0),
                    (right - diameter, top, 270.0),
                    (right - diameter, bottom - diameter, 0.0),
                    (left, bottom - diameter, 90.0),
                ] {
                    GdipAddPathArcI(path, xx, yy, diameter, diameter, angle, 90.0);
                }
                GdipClosePathFigure(path);
                GdipFillPath(graphics, fill.cast(), path);
            }
            if !path.is_null() {
                GdipDeletePath(path);
            }
            if !fill.is_null() {
                GdipDeleteBrush(fill.cast());
            }
            GdipDeleteGraphics(graphics);
            return;
        }
        let brush = CreateSolidBrush(color);
        let oldb = SelectObject(self.dc, brush);
        let oldp = SelectObject(self.dc, GetStockObject(NULL_PEN));
        RoundRect(
            self.dc,
            self.px(x),
            self.px(y),
            self.px(x + w),
            self.px(y + h),
            self.px(radius * 2),
            self.px(radius * 2),
        );
        SelectObject(self.dc, oldp);
        SelectObject(self.dc, oldb);
        DeleteObject(brush);
    }
    pub unsafe fn stroke(&self, points: &[(i32, i32)], color: u32, width: i32) {
        let pen = CreatePen(PS_SOLID, self.px(width).max(1), color);
        let old = SelectObject(self.dc, pen);
        if let Some(&(x, y)) = points.first() {
            MoveToEx(self.dc, self.px(x), self.px(y), std::ptr::null_mut());
            for &(x, y) in points.iter().skip(1) {
                LineTo(self.dc, self.px(x), self.px(y));
            }
        }
        SelectObject(self.dc, old);
        DeleteObject(pen);
    }
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn text(
        &self,
        s: &str,
        font: HFONT,
        color: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        flags: u32,
    ) {
        let old = SelectObject(self.dc, font);
        SetBkMode(self.dc, TRANSPARENT as i32);
        SetTextColor(self.dc, color);
        DrawTextW(
            self.dc,
            wide(s).as_ptr(),
            -1,
            &mut self.rect(x, y, w, h),
            flags | DT_NOPREFIX,
        );
        SelectObject(self.dc, old);
    }
    pub unsafe fn check(&self, x: i32, y: i32, checked: bool) {
        self.round(
            x,
            y,
            16,
            16,
            4,
            if checked { ACCENT } else { rgb(175, 183, 178) },
        );
        if checked {
            self.stroke(
                &[(x + 4, y + 8), (x + 7, y + 11), (x + 12, y + 5)],
                WHITE,
                1,
            );
        } else {
            self.round(x + 1, y + 1, 14, 14, 3, PAPER);
        }
    }
    pub unsafe fn icon(&self, id: i32, x: i32, y: i32, color: u32, pinned: bool) {
        match id {
            111 => {
                self.stroke(&[(x + 4, y + 10), (x + 16, y + 10)], color, 1);
                self.stroke(&[(x + 10, y + 4), (x + 10, y + 16)], color, 1);
            }
            112 => {
                for dx in [4, 10, 16] {
                    self.round(x + dx - 1, y + 9, 3, 3, 2, color);
                }
            }
            117 => {
                self.stroke(&[(x + 5, y + 5), (x + 15, y + 15)], color, 1);
                self.stroke(&[(x + 15, y + 5), (x + 5, y + 15)], color, 1);
            }
            116 | 210 => {
                self.stroke(
                    &[(x + 12, y + 4), (x + 6, y + 10), (x + 12, y + 16)],
                    color,
                    1,
                );
            }
            119 => {
                for (dy, length) in [(5, 14), (10, 10), (15, 6)] {
                    self.stroke(&[(x + 3, y + dy), (x + 3 + length, y + dy)], color, 2);
                }
            }
            118 => {
                self.stroke(
                    &[
                        (x + 3, y + 4),
                        (x + 17, y + 4),
                        (x + 17, y + 16),
                        (x + 3, y + 16),
                        (x + 3, y + 4),
                    ],
                    color,
                    1,
                );
                self.stroke(&[(x + 8, y + 4), (x + 8, y + 16)], color, 1);
                if pinned {
                    self.fill(x + 4, y + 5, 3, 10, color);
                }
            }
            113 => {
                if pinned {
                    self.round(x + 6, y + 4, 8, 8, 2, color);
                } else {
                    self.stroke(
                        &[
                            (x + 6, y + 4),
                            (x + 14, y + 4),
                            (x + 13, y + 11),
                            (x + 15, y + 13),
                            (x + 5, y + 13),
                            (x + 7, y + 11),
                            (x + 6, y + 4),
                        ],
                        color,
                        1,
                    );
                }
                self.stroke(&[(x + 10, y + 12), (x + 10, y + 18)], color, 1);
            }
            _ => {}
        }
    }
}
