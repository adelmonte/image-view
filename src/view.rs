//! Port of QVGraphicsView's zoom/fit/scroll math. It mirrors Qt's QGraphicsView with
//! `NoAnchor`, hidden scrollbars and `ScrollHandDrag`, so zoom, fit and panning behave the same.
//!
//! Model, in logical points relative to the view's top-left corner:
//! screen = (sx * scale * px - h, sy * scale * py - v) for image pixel (px, py),
//! where sx/sy are ±1 (mirror/flip) and (h, v) are Qt's integer scrollbar values.

const MARGIN: f64 = -2.0;

pub struct ViewState {
    /// Image size in pixels (0 = nothing loaded)
    pub item: (f64, f64),
    pub scale: f64,
    pub sx: f64,
    pub sy: f64,
    pub h: i64,
    pub v: i64,
    pub current_scale: f64,
    pub is_original_size: bool,
    last_zoom_event_pos: Option<(i32, i32)>,
    last_zoom_rounding_error: (f64, f64),
    last_scroll_rounding_error: (f64, f64),
    pub size: (i32, i32),
}

impl Default for ViewState {
    fn default() -> Self {
        ViewState {
            item: (0.0, 0.0),
            scale: 1.0,
            sx: 1.0,
            sy: 1.0,
            h: 0,
            v: 0,
            current_scale: 1.0,
            is_original_size: false,
            last_zoom_event_pos: None,
            last_zoom_rounding_error: (0.0, 0.0),
            last_scroll_rounding_error: (0.0, 0.0),
            size: (0, 0),
        }
    }
}

pub struct FitOptions {
    pub crop_mode: i32,
    pub past_actual_size: bool,
}

impl ViewState {
    fn m11(&self) -> f64 {
        self.sx * self.scale
    }
    fn m22(&self) -> f64 {
        self.sy * self.scale
    }
    pub fn map_to_scene(&self, p: (f64, f64)) -> (f64, f64) {
        ((p.0 + self.h as f64) / self.m11(), (p.1 + self.v as f64) / self.m22())
    }
    pub fn map_from_scene(&self, p: (f64, f64)) -> (f64, f64) {
        (self.m11() * p.0 - self.h as f64, self.m22() * p.1 - self.v as f64)
    }
    /// QGraphicsView::centerOn (qView's override)
    fn center_on(&mut self, pos: (f64, f64)) {
        let (w, h) = (self.size.0 as f64, self.size.1 as f64);
        self.h = (self.m11() * pos.0 - w / 2.0).trunc() as i64;
        self.v = (self.m22() * pos.1 - h / 2.0).trunc() as i64;
    }
    pub fn center_on_item(&mut self) {
        let (iw, ih) = self.item;
        self.center_on((iw / 2.0, ih / 2.0));
    }

    /// Returns false if the zoom was refused (too far in or out).
    pub fn zoom(&mut self, factor: f64, pos: Option<(i32, i32)>, under_mouse: bool, cursor_zoom: bool) -> bool {
        // don't zoom too far out, dude
        self.current_scale *= factor;
        if self.current_scale >= 500.0 || self.current_scale <= 0.01 {
            self.current_scale /= factor;
            return false;
        }
        if pos != self.last_zoom_event_pos {
            self.last_zoom_event_pos = pos;
            self.last_zoom_rounding_error = (0.0, 0.0);
        }
        let p = pos.map(|(x, y)| (x as f64, y as f64)).unwrap_or((-1.0, -1.0));
        let sp = self.map_to_scene(p);
        let scene_pos = (sp.0 - self.last_zoom_rounding_error.0, sp.1 - self.last_zoom_rounding_error.1);

        self.scale *= factor;

        if self.current_scale > 1.00001 && pos.is_some() && under_mouse && cursor_zoom {
            let p1 = self.map_from_scene(scene_pos);
            self.h = (self.h as f64 + (p1.0 - p.0)).trunc() as i64;
            self.v = (self.v as f64 + (p1.1 - p.1)).trunc() as i64;
            let np = self.map_to_scene(p);
            self.last_zoom_rounding_error = (np.0 - scene_pos.0, np.1 - scene_pos.1);
        } else {
            self.center_on_item();
        }
        true
    }

    /// QVGraphicsView::fitInViewMarginless
    pub fn fit(&mut self, opts: &FitOptions) {
        let (iw, ih) = self.item;
        let mut adj = (iw, ih);
        match opts.crop_mode {
            1 => adj.0 = 1.0,
            2 => adj.1 = 1.0,
            _ => {}
        }
        if adj.0 <= 0.0 || adj.1 <= 0.0 {
            return;
        }
        let (w, h) = (self.size.0 as f64, self.size.1 as f64);
        let view_rect = if opts.past_actual_size || adj.0 >= w - MARGIN || adj.1 >= h - MARGIN {
            (w - 2.0 * MARGIN, h - 2.0 * MARGIN)
        } else {
            (iw, ih)
        };
        if view_rect.0 <= 0.0 || view_rect.1 <= 0.0 {
            return;
        }
        self.scale = (view_rect.0 / adj.0).min(view_rect.1 / adj.1);
        self.center_on_item();
        self.is_original_size = false;
        self.current_scale = 1.0;
    }

    /// Returns true if it switched to original size, false if it should reset instead.
    pub fn original_size(&mut self) -> bool {
        if self.is_original_size && self.scale == 1.0 && self.sx == 1.0 && self.sy == 1.0 {
            return false;
        }
        self.scale = 1.0;
        self.sx = 1.0;
        self.sy = 1.0;
        self.center_on_item();
        self.is_original_size = true;
        true
    }

    /// Wheel/touchpad scrolling in qView angle-delta units (120 per notch)
    pub fn scroll(&mut self, ax: f64, ay: f64, swap: bool) {
        let mut sx = -ax / 2.0;
        let mut sy = -ay / 2.0;
        if swap {
            std::mem::swap(&mut sx, &mut sy);
        }
        let target = (sx - self.last_scroll_rounding_error.0, sy - self.last_scroll_rounding_error.1);
        let rounded = (target.0.round(), target.1.round());
        self.h += rounded.0 as i64;
        self.v += rounded.1 as i64;
        self.last_scroll_rounding_error = (rounded.0 - target.0, rounded.1 - target.1);
    }

    pub fn pan(&mut self, dx: i64, dy: i64) {
        self.h -= dx;
        self.v -= dy;
    }

    /// Screen positions (relative to the view) of image pixel (0,0) and (w,h)
    pub fn corners(&self) -> ((f64, f64), (f64, f64)) {
        (self.map_from_scene((0.0, 0.0)), self.map_from_scene(self.item))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(img: (f64, f64), size: (i32, i32)) -> ViewState {
        ViewState { item: img, size, ..Default::default() }
    }

    #[test]
    fn fit_centers_and_scales() {
        let mut v = view((2000.0, 1000.0), (1000, 800));
        v.fit(&FitOptions { crop_mode: 0, past_actual_size: true });
        assert!((v.scale - 1004.0 / 2000.0).abs() < 1e-9);
        let (a, b) = v.corners();
        assert!((a.0 + b.0 - 1000.0).abs() <= 2.0, "{a:?} {b:?}");
        assert!((a.1 + b.1 - 800.0).abs() <= 2.0, "{a:?} {b:?}");
    }

    #[test]
    fn small_image_not_enlarged() {
        let mut v = view((100.0, 50.0), (1000, 800));
        v.fit(&FitOptions { crop_mode: 0, past_actual_size: false });
        assert_eq!(v.scale, 1.0);
    }

    #[test]
    fn cursor_zoom_keeps_point_fixed() {
        let mut v = view((4000.0, 3000.0), (800, 600));
        v.fit(&FitOptions { crop_mode: 0, past_actual_size: true });
        let pos = (200, 150);
        let before = v.map_to_scene((200.0, 150.0));
        for _ in 0..5 {
            assert!(v.zoom(1.25, Some(pos), true, true));
        }
        let after = v.map_to_scene((200.0, 150.0));
        assert!((before.0 - after.0).abs() < 2.0 && (before.1 - after.1).abs() < 2.0, "{before:?} {after:?}");
    }

    #[test]
    fn mirrored_corners() {
        let mut v = view((100.0, 100.0), (200, 200));
        v.sx = -1.0;
        v.fit(&FitOptions { crop_mode: 0, past_actual_size: false });
        let (a, b) = v.corners();
        assert!(a.0 > b.0);
        assert!((a.0 + b.0 - 200.0).abs() <= 2.0);
    }
}
