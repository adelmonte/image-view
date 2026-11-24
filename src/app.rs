//! The window: MainWindow + the drawing half of QVGraphicsView, on egui.

use crate::core::{CoreEvent, ImageCore, LoaderMsg};
use crate::decode::{self, Image};
use crate::i18n::tr;
use crate::settings::{self, Setting, Store};
use crate::shortcuts::{self, ShortcutManager};
use crate::util;
use crate::view::{FitOptions, ViewState};
use eframe::egui::{self, Color32, ColorImage, Pos2, Rect, TextureHandle, TextureOptions, Vec2, ViewportCommand};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAX_EXPENSIVE_SCALING_SIZE: f64 = 3.0;
const MAX_FILTERING_SIZE: f64 = 5000.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum GoTo {
    First,
    Previous,
    Next,
    Last,
    Random,
}

struct DeletedPaths {
    path_in_trash: String,
    previous_path: String,
}

/// The image as GPU textures, split into tiles when larger than the max texture size
struct Tex {
    tiles: Vec<(TextureHandle, Rect)>,
    smooth: bool,
}

/// A copy scaled on the CPU to exactly the on-screen size (sharper than GPU filtering)
struct Scaled {
    tex: TextureHandle,
    size: Vec2,
    item_gen: u64,
    scale: f64,
}

struct ScaleResult {
    job: u64,
    item_gen: u64,
    scale: f64,
    image: ColorImage,
}

enum Dialog {
    Error { title: String, text: String },
    Delete { permanent: bool, text: String, dont_ask: bool },
    Rename { path: String, name: String, init: bool },
}

pub struct App {
    core: ImageCore,
    view: ViewState,
    loader_rx: Receiver<LoaderMsg>,
    shortcuts: ShortcutManager,
    config_mtime: Option<std::time::SystemTime>,

    full: Option<Tex>,
    full_dirty: bool,
    item_gen: u64,
    scaled: Option<Scaled>,
    scale_job: u64,
    scale_tx: Sender<ScaleResult>,
    scale_rx: Receiver<ScaleResult>,
    expensive_at: Option<Instant>,

    initial_file: Option<String>,
    started: bool,
    title: String,
    sent_title: String,
    fullscreen: bool,
    maximized: bool,
    focused: bool,
    stored_maximized: bool,
    just_launched_with_image: bool,
    normal_size: Option<Vec2>,
    custom_bg: Option<Color32>,
    slideshow_at: Option<Instant>,
    last_deleted: Vec<DeletedPaths>,
    hand_drag: Option<Pos2>,
    under_mouse: bool,
    dialog: Option<Dialog>,
    dialog_new: bool,
    info_open: bool,
    info: Vec<(String, String)>,
    picked_rx: Option<Receiver<Vec<String>>>,
    save_rx: Option<Receiver<Option<(std::path::PathBuf, Image)>>>,
    commands: Vec<ViewportCommand>,
    ctx_hint: egui::Context,
}

fn ctx_waker(ctx: &egui::Context) -> Arc<dyn Fn() + Send + Sync> {
    let ctx = ctx.clone();
    Arc::new(move || ctx.request_repaint())
}

fn config_mtime() -> Option<std::time::SystemTime> {
    std::fs::metadata(settings::config_path()).and_then(|m| m.modified()).ok()
}

/// Start another independent window (one process per window)
pub fn spawn_window(file: Option<&str>) {
    let Ok(exe) = std::env::current_exe() else { return };
    let mut cmd = std::process::Command::new(exe);
    if let Some(f) = file {
        cmd.arg("--").arg(f);
    }
    if let Ok(mut child) = cmd.spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

fn xdg_open(target: &str) {
    match std::process::Command::new("xdg-open").arg(target).spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => eprintln!("xdg-open: {}", e),
    }
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, file: Option<String>) -> Self {
        let ctx = &cc.egui_ctx;
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        ctx.set_visuals(egui::Visuals::dark());
        let mut fonts = egui::FontDefinitions::default();
        fonts.font_data.insert("Lato".into(), Arc::new(egui::FontData::from_static(include_bytes!("../resources/Lato-Regular.ttf"))));
        fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "Lato".into());
        ctx.set_fonts(fonts);

        let (tx, loader_rx) = channel();
        let (scale_tx, scale_rx) = channel();
        let core = ImageCore::new(tx, ctx_waker(ctx));
        let mut app = App {
            core,
            view: ViewState::default(),
            loader_rx,
            shortcuts: ShortcutManager::new(),
            config_mtime: config_mtime(),
            full: None,
            full_dirty: false,
            item_gen: 0,
            scaled: None,
            scale_job: 0,
            scale_tx,
            scale_rx,
            expensive_at: None,
            just_launched_with_image: file.is_some(),
            initial_file: file,
            started: false,
            title: "image-view".into(),
            sent_title: String::new(),
            fullscreen: false,
            maximized: false,
            focused: false,
            stored_maximized: false,
            normal_size: None,
            custom_bg: None,
            slideshow_at: None,
            last_deleted: vec![],
            hand_drag: None,
            under_mouse: false,
            dialog: None,
            dialog_new: false,
            info_open: false,
            info: vec![],
            picked_rx: None,
            save_rx: None,
            commands: vec![],
            ctx_hint: ctx.clone(),
        };
        app.settings_updated();
        app
    }

    // ---- core events ----

    /// Deliver queued core events (Qt signal delivery)
    fn pump(&mut self) {
        loop {
            let events = std::mem::take(&mut self.core.pending);
            if events.is_empty() {
                break;
            }
            for e in events {
                match e {
                    CoreEvent::FileChanged => {
                        self.update_loaded_pixmap_item();
                        self.file_changed();
                    }
                    CoreEvent::UpdateLoadedPixmapItem => self.update_loaded_pixmap_item(),
                    CoreEvent::AnimatedFrameChanged => {
                        self.item_gen += 1;
                        self.full_dirty = true;
                    }
                }
            }
        }
    }

    fn update_loaded_pixmap_item(&mut self) {
        let d = &self.core.details;
        self.view.item = if d.is_pixmap_loaded { (d.loaded_pixmap_size.0 as f64, d.loaded_pixmap_size.1 as f64) } else { (0.0, 0.0) };
        self.item_gen += 1;
        self.full_dirty = true;
        self.scaled = None;
        self.reset_scale();
        self.set_window_size();
        // Scale right away instead of after the timer, so a new image is crisp sooner
        if settings::get().bool(Setting::ScalingEnabled) && self.core.details.is_pixmap_loaded {
            self.expensive_at = None;
            self.scale_expensively();
        }
    }

    fn file_changed(&mut self) {
        if self.info_open {
            self.refresh_info();
        }
        self.update_window_title();
    }

    // ---- loading ----

    fn load_file(&mut self, file: &str) {
        self.core.load_file(file, false);
        self.pump();
    }

    fn open_file(&mut self, file: &str) {
        self.load_file(file);
        self.cancel_slideshow();
    }

    fn reload_file(&mut self) {
        if !self.core.details.is_pixmap_loaded {
            return;
        }
        let p = self.core.details.file_path.clone();
        self.core.load_file(&p, true);
        self.pump();
    }

    fn load_paths(&mut self, paths: &[String]) {
        for (i, p) in paths.iter().enumerate() {
            if i == 0 {
                self.open_file(p);
            } else {
                spawn_window(Some(p));
            }
        }
    }

    // ---- scaling ----

    fn fit_options() -> FitOptions {
        let sm = settings::get();
        FitOptions { crop_mode: sm.int(Setting::CropMode), past_actual_size: sm.bool(Setting::PastActualSizeEnabled) }
    }

    fn reset_scale(&mut self) {
        if !self.core.details.is_pixmap_loaded {
            return;
        }
        self.view.fit(&Self::fit_options());
        if settings::get().bool(Setting::ScalingEnabled) {
            self.start_expensive_timer();
        }
    }

    fn start_expensive_timer(&mut self) {
        self.expensive_at = Some(Instant::now() + Duration::from_millis(50));
    }

    fn zoom(&mut self, factor: f64, pos: Option<(i32, i32)>) {
        if !self.core.details.is_pixmap_loaded {
            return;
        }
        let cursor_zoom = settings::get().bool(Setting::CursorZoom);
        if !self.view.zoom(factor, pos, self.under_mouse, cursor_zoom) {
            return;
        }
        if settings::get().bool(Setting::ScalingEnabled) && !self.view.is_original_size {
            self.start_expensive_timer();
        }
    }

    fn zoom_step(&mut self, zoom_in: bool) {
        let f = settings::get().int(Setting::ScaleFactor) as f64 / 100.0 + 1.0;
        self.zoom(if zoom_in { f } else { 1.0 / f }, None);
    }

    fn original_size(&mut self) {
        if !self.core.details.is_pixmap_loaded {
            return;
        }
        if !self.view.original_size() {
            self.reset_scale();
            return;
        }
        self.scaled = None;
    }

    /// QVGraphicsView::scaleExpensively, done on a worker thread
    fn scale_expensively_with(&mut self, ppp: f64) {
        let sm = settings::get();
        let cs = self.view.current_scale;
        if cs >= MAX_EXPENSIVE_SCALING_SIZE || (!sm.bool(Setting::ScalingTwoEnabled) && cs > 1.00001) || self.core.details.is_movie_loaded {
            self.scaled = None;
            return;
        }
        let Some(img) = self.core.loaded.clone() else { return };
        let (pw, ph) = (img.size[0] as i64, img.size[1] as i64);
        let (dw_f, dh_f) = (pw as f64 * self.view.scale * ppp, ph as f64 * self.view.scale * ppp);
        if (dw_f - pw as f64).abs() < 1.0 && (dh_f - ph as f64).abs() < 1.0 {
            self.scaled = None;
            return;
        }
        let (dw, dh) = (dw_f.round() as i64, dh_f.round() as i64);
        // QSize::scale(KeepAspectRatio)
        let rw = dh * pw / ph.max(1);
        let (sw, sh) = if rw <= dw { (rw, dh) } else { (dw, dw * ph / pw.max(1)) };
        if sw <= 0 || sh <= 0 {
            self.scaled = None;
            return;
        }
        self.scale_job += 1;
        let (job, item_gen, scale) = (self.scale_job, self.item_gen, self.view.scale);
        let tx = self.scale_tx.clone();
        let wake = ctx_waker(&self.ctx_hint);
        std::thread::spawn(move || {
            let _t = util::Trace::new("scale_expensively");
            if let Some(image) = decode::scale_smooth(&img, sw as i32, sh as i32) {
                if tx.send(ScaleResult { job, item_gen, scale, image }).is_ok() {
                    wake();
                }
            }
        });
    }

    fn scale_expensively(&mut self) {
        let ppp = self.ctx_hint.pixels_per_point() as f64;
        self.scale_expensively_with(ppp);
    }

    // ---- settings ----

    fn settings_updated(&mut self) {
        self.core.settings_updated();
        self.pump();
        if self.core.details.is_pixmap_loaded {
            self.reset_scale();
        }
        let sm = settings::get();
        self.custom_bg = if sm.bool(Setting::BgColorEnabled) {
            util::parse_color(&sm.string(Setting::BgColor)).map(|(r, g, b)| Color32::from_rgb(r, g, b))
        } else {
            None
        };
        if self.slideshow_at.is_some() {
            self.start_slideshow_timer();
        }
        self.full_dirty = true;
        self.update_window_title();
        self.set_window_size();
    }

    fn check_config_changed(&mut self) {
        let m = config_mtime();
        if m == self.config_mtime {
            return;
        }
        self.config_mtime = m;
        self.shortcuts.update_shortcuts();
        if settings::get().load_settings() {
            self.settings_updated();
        }
    }

    // ---- window ----

    fn update_window_title(&mut self) {
        let d = &self.core.details;
        let mut s = "image-view".to_string();
        if d.is_file() {
            let idx = d.loaded_index_in_folder + 1;
            let count = d.folder_file_info_list.len();
            match settings::get().int(Setting::TitleBarMode) {
                1 => s = d.file_name(),
                2 => s = format!("{}/{} - {}", idx, count, d.file_name()),
                3 => {
                    s = format!("{}/{} - {}", idx, count, d.file_name());
                    if !d.error_data.has_error {
                        s += &format!(" - {}x{} - {}", d.base_image_size.0, d.base_image_size.1, util::format_bytes(d.file_size()));
                    }
                    s += " - image-view";
                }
                _ => {}
            }
        }
        self.title = s;
    }

    /// MainWindow::setWindowSize
    fn set_window_size(&mut self) {
        if !self.core.details.is_pixmap_loaded || !self.started {
            return;
        }
        let sm = settings::get();
        let mode = sm.int(Setting::WindowResizeMode);
        if !(mode == 2 || (mode == 1 && self.just_launched_with_image)) {
            return;
        }
        self.just_launched_with_image = false;
        if self.maximized || self.fullscreen {
            return;
        }
        let Some(screen) = self.ctx_hint.input(|i| i.viewport().monitor_size) else { return };
        let (sw, sh) = (screen.x as f64, screen.y as f64);
        let min_pct = sm.int(Setting::MinWindowResizedPercentage) as f64 / 100.0;
        let max_pct = sm.int(Setting::MaxWindowResizedPercentage) as f64 / 100.0;
        let size = self.core.details.loaded_pixmap_size;
        let mut img = (size.0 as i64 - 4, size.1 as i64 - 4);
        let bounded = |p: f64| -> (i64, i64) { ((sw * p).round() as i64, (sh * p).round() as i64) };
        let (min_size, max_size) = (bounded(min_pct), bounded(max_pct));
        let scale_keep = |s: (i64, i64), to: (i64, i64)| -> (i64, i64) {
            if s.0 <= 0 || s.1 <= 0 {
                return to;
            }
            let rw = to.1 * s.0 / s.1;
            if rw <= to.0 {
                (rw, to.1)
            } else {
                (to.0, to.0 * s.1 / s.0)
            }
        };
        if img.0 < min_size.0 && img.1 < min_size.1 {
            img = scale_keep(img, min_size);
        } else if img.0 > max_size.0 || img.1 > max_size.1 {
            img = scale_keep(img, max_size);
        }
        // Wayland: windows can't position themselves; the compositor places them
        self.commands.push(ViewportCommand::InnerSize(Vec2::new(img.0.max(1) as f32, img.1.max(1) as f32)));
    }

    fn toggle_full_screen(&mut self) {
        if self.fullscreen {
            self.commands.push(ViewportCommand::Fullscreen(false));
            if self.stored_maximized {
                self.commands.push(ViewportCommand::Maximized(true));
            }
        } else {
            self.stored_maximized = self.maximized;
            self.commands.push(ViewportCommand::Fullscreen(true));
        }
    }

    fn save_geometry(&self) {
        let size = self.normal_size.unwrap_or(Vec2::new(640.0, 480.0));
        Store::set("General", "geometry", &format!("0,0,{},{},{}", size.x.round() as i32, size.y.round() as i32, self.maximized as i32));
    }

    // ---- navigation ----

    fn go_to_file(&mut self, mode: GoTo) {
        let mut retry_folder_info = false;
        let expired = self.core.details.time_since_loaded.map(|t| t.elapsed() > Duration::from_millis(3000)).unwrap_or(true);
        if expired {
            if std::path::Path::new(&self.core.details.file_path).exists() {
                self.core.update_folder_info(None);
            } else {
                retry_folder_info = true;
            }
        }
        let list = &self.core.details.folder_file_info_list;
        if list.is_empty() {
            return;
        }
        let len = list.len() as i32;
        let mut idx = self.core.details.loaded_index_in_folder;
        let loop_folders = settings::get().bool(Setting::LoopFoldersEnabled);
        let search;
        match mode {
            GoTo::First => {
                idx = 0;
                search = 1;
            }
            GoTo::Previous | GoTo::Next => {
                let at_end = if mode == GoTo::Next { idx == len - 1 } else { idx == 0 };
                if at_end {
                    if !loop_folders {
                        self.cancel_slideshow();
                        return;
                    }
                    idx = if mode == GoTo::Next { 0 } else { len - 1 };
                } else {
                    idx += if mode == GoTo::Next { 1 } else { -1 };
                }
                search = if mode == GoTo::Next { 1 } else { -1 };
            }
            GoTo::Last => {
                idx = len - 1;
                search = -1;
            }
            GoTo::Random => {
                if len > 1 {
                    use rand::RngExt;
                    let mut rng = rand::rng();
                    loop {
                        let r = rng.random_range(0..len);
                        if r != idx {
                            idx = r;
                            break;
                        }
                    }
                }
                search = 1;
            }
        }
        let exists = |i: i32| list.get(i as usize).map(|f| std::path::Path::new(&f.absolute_file_path).exists()).unwrap_or(false);
        while search == 1 && idx < len - 1 && !exists(idx) {
            idx += 1;
        }
        while search == -1 && idx > 0 && !exists(idx) {
            idx -= 1;
        }
        let next = list.get(idx.max(0) as usize).map(|f| f.absolute_file_path.clone()).unwrap_or_default();
        if next.is_empty() || !std::path::Path::new(&next).exists() || next == self.core.details.file_path {
            return;
        }
        if retry_folder_info {
            self.core.update_folder_info(Some(util::parent_dir(&next)));
        }
        self.load_file(&next);
    }

    // ---- slideshow ----

    fn start_slideshow_timer(&mut self) {
        let secs = settings::get().double(Setting::SlideshowTimer);
        let ms = ((secs * 1000.0) as i64).max(1) as u64;
        self.slideshow_at = Some(Instant::now() + Duration::from_millis(ms));
    }

    fn toggle_slideshow(&mut self) {
        if self.slideshow_at.is_some() {
            self.slideshow_at = None;
        } else {
            self.start_slideshow_timer();
        }
    }

    fn cancel_slideshow(&mut self) {
        self.slideshow_at = None;
    }

    // ---- actions ----

    fn action_enabled(&self, key: &str) -> bool {
        let d = &self.core.details;
        match key {
            "saveframeas" | "pause" | "nextframe" | "decreasespeed" | "resetspeed" | "increasespeed" => d.is_movie_loaded,
            "firstfile" | "previousfile" | "nextfile" | "lastfile" | "randomfile" => !d.folder_file_info_list.is_empty(),
            "undo" => self.last_deleted.last().map(|l| !l.path_in_trash.is_empty()).unwrap_or(false),
            "quit" | "newwindow" | "open" | "paste" | "fullscreen" | "options" => true,
            _ => d.is_pixmap_loaded,
        }
    }

    /// ActionManager::actionTriggered
    fn trigger(&mut self, key: &str) {
        match key {
            "quit" => self.commands.push(ViewportCommand::Close),
            "newwindow" => spawn_window(None),
            "open" => self.pick_files(),
            "options" => self.open_config(),
            "reloadfile" => self.reload_file(),
            "opencontainingfolder" => xdg_open(&util::parent_dir(&self.core.details.file_path)),
            "showfileinfo" => {
                self.refresh_info();
                self.info_open = true;
            }
            "delete" => self.ask_delete_file(false),
            "deletepermanent" => self.ask_delete_file(true),
            "undo" => self.undo_delete(),
            "copy" => self.copy(),
            "paste" => self.paste(),
            "rename" => {
                let path = self.core.details.file_path.clone();
                self.dialog = Some(Dialog::Rename { name: util::file_name(&path), path, init: true });
                self.dialog_new = true;
            }
            "zoomin" => self.zoom_step(true),
            "zoomout" => self.zoom_step(false),
            "resetzoom" => self.reset_scale(),
            "originalsize" => self.original_size(),
            "rotateright" | "rotateleft" => {
                self.core.rotate_image(if key == "rotateright" { 90 } else { -90 });
                self.pump();
                self.reset_scale();
            }
            "mirror" => {
                self.view.sx = -self.view.sx;
                self.reset_scale();
            }
            "flip" => {
                self.view.sy = -self.view.sy;
                self.reset_scale();
            }
            "fullscreen" => self.toggle_full_screen(),
            "firstfile" => self.go_to_file(GoTo::First),
            "previousfile" => self.go_to_file(GoTo::Previous),
            "nextfile" => self.go_to_file(GoTo::Next),
            "lastfile" => self.go_to_file(GoTo::Last),
            "randomfile" => self.go_to_file(GoTo::Random),
            "saveframeas" => self.save_frame_as(),
            "pause" => {
                let p = self.core.is_paused();
                self.core.set_paused(!p);
            }
            "nextframe" => {
                self.core.jump_to_next_frame();
                self.pump();
            }
            "decreasespeed" => self.core.set_speed(self.core.speed() - 25),
            "resetspeed" => self.core.set_speed(100),
            "increasespeed" => self.core.set_speed(self.core.speed() + 25),
            "slideshow" => self.toggle_slideshow(),
            _ => {}
        }
    }

    fn error(&mut self, text: String) {
        self.dialog = Some(Dialog::Error { title: tr("MainWindow", "Error"), text });
        self.dialog_new = true;
    }

    /// "Settings": open the config file in the default text editor
    fn open_config(&mut self) {
        let path = settings::config_path();
        if !path.exists() {
            Store::set("General", "configversion", "7");
        }
        xdg_open(&path.to_string_lossy());
    }

    fn pick_files(&mut self) {
        let dir = Store::get("recents", "lastFileDialogDir").unwrap_or_else(|| settings::home_dir().to_string_lossy().to_string());
        let exts: Vec<String> = decode::extensions().iter().map(|e| e.trim_start_matches('.').to_string()).collect();
        let (tx, rx) = channel();
        let wake = ctx_waker(&self.ctx_hint);
        let title = tr("QVApplication", "Open...");
        let filter = tr("QVApplication", "Supported Images");
        let all = tr("QVApplication", "All Files");
        std::thread::spawn(move || {
            let files = rfd::FileDialog::new().set_title(title).set_directory(dir).add_filter(filter, &exts).add_filter(all, &["*"]).pick_files();
            let files: Vec<String> = files.unwrap_or_default().iter().map(|p| p.to_string_lossy().to_string()).collect();
            let _ = tx.send(files);
            wake();
        });
        self.picked_rx = Some(rx);
    }

    fn save_frame_as(&mut self) {
        if !self.core.details.is_movie_loaded {
            return;
        }
        if !self.core.is_paused() {
            self.core.set_paused(true);
        }
        let Some(m) = self.core.movie.as_ref() else { return };
        let frame = m.current_image().clone();
        let name = format!("{}-{}.png", util::base_name(&self.core.details.file_path), m.current);
        let dir = Store::get("recents", "lastFileDialogDir").unwrap_or_else(|| settings::home_dir().to_string_lossy().to_string());
        let title = tr("MainWindow", "Save Frame As...");
        let (tx, rx) = channel();
        let wake = ctx_waker(&self.ctx_hint);
        std::thread::spawn(move || {
            let file = rfd::FileDialog::new().set_title(title).set_directory(dir).set_file_name(name).save_file();
            let _ = tx.send(file.map(|f| (f, frame)));
            wake();
        });
        self.save_rx = Some(rx);
    }

    fn write_frame(&mut self, mut file: std::path::PathBuf, frame: Image) {
        if file.extension().is_none() {
            file.set_extension("png");
        }
        let rgba = decode::to_rgba(&frame);
        if let Some(buf) = image::RgbaImage::from_raw(frame.size[0] as u32, frame.size[1] as u32, rgba) {
            if let Err(e) = buf.save(&file) {
                self.error(e.to_string());
            }
        }
    }

    fn ask_delete_file(&mut self, permanent: bool) {
        if !permanent && !settings::get().bool(Setting::AskDelete) {
            self.delete_file(permanent);
            return;
        }
        let path = &self.core.details.file_path;
        let file_name = util::file_name(path);
        if !util::is_writable(path) {
            self.error(tr("MainWindow", "Can't delete %1:\nNo write permission or file is read-only.").replace("%1", &file_name));
            return;
        }
        let text = if permanent {
            tr("MainWindow", "Are you sure you want to delete %1 permanently? This can't be undone.").replace("%1", &file_name)
        } else {
            tr("MainWindow", "Are you sure you want to move %1 to the Trash?").replace("%1", &file_name)
        };
        self.dialog = Some(Dialog::Delete { permanent, text, dont_ask: false });
        self.dialog_new = true;
    }

    fn delete_file(&mut self, permanent: bool) {
        let path = self.core.details.file_path.clone();
        let file_name = util::file_name(&path);
        self.core.close_image();
        self.pump();

        let mut trash_path = String::new();
        let success = if permanent {
            std::fs::remove_file(&path).is_ok()
        } else {
            match crate::trash::move_to_trash(&path) {
                Some(t) => {
                    trash_path = t;
                    true
                }
                None => false,
            }
        };
        if !success || std::path::Path::new(&path).exists() {
            self.open_file(&path);
            self.error(tr("MainWindow", "Can't delete %1.").replace("%1", &file_name));
            return;
        }
        let after = settings::get().int(Setting::AfterDelete);
        if after > 1 {
            self.go_to_file(GoTo::Next);
        } else if after < 1 {
            self.go_to_file(GoTo::Previous);
        }
        if !trash_path.is_empty() {
            self.last_deleted.push(DeletedPaths { path_in_trash: trash_path, previous_path: path });
        }
    }

    fn undo_delete(&mut self) {
        let Some(last) = self.last_deleted.pop() else { return };
        if last.path_in_trash.is_empty() || last.previous_path.is_empty() {
            return;
        }
        let name = util::file_name(&last.path_in_trash);
        if !util::is_writable(&last.path_in_trash) {
            self.error(tr("MainWindow", "Can't undo deletion of %1:\nNo write permission or file is read-only.").replace("%1", &name));
            return;
        }
        let failed = !crate::trash::restore(&last.path_in_trash, &last.previous_path);
        self.open_file(&last.previous_path);
        if failed {
            self.error(tr("MainWindow", "Failed undoing deletion of %1.").replace("%1", &name));
        }
    }

    fn rename_to(&mut self, path: &str, new_name: &str) {
        let fname = util::file_name(path);
        if !util::is_writable(path) {
            self.error(tr("QVRenameDialog", "Could not rename %1:\nNo write permission or file is read-only.").replace("%1", &fname));
            return;
        }
        let new_path = util::absolute_clean(&format!("{}/{}", util::parent_dir(path), new_name));
        if new_path == path {
            return;
        }
        if std::fs::rename(path, &new_path).is_ok() {
            self.open_file(&new_path);
        } else {
            self.error(tr("QVRenameDialog", "Could not rename %1:\n(Check that all characters are valid)").replace("%1", &fname));
        }
    }

    fn copy(&mut self) {
        let Some(img) = self.core.loaded.clone() else { return };
        // The clipboard is served from this thread until something else is copied
        std::thread::spawn(move || {
            use arboard::SetExtLinux;
            let data = arboard::ImageData { width: img.size[0], height: img.size[1], bytes: decode::to_rgba(&img).into() };
            match arboard::Clipboard::new() {
                Ok(mut c) => {
                    if let Err(e) = c.set().wait().image(data) {
                        eprintln!("copy: {}", e);
                    }
                }
                Err(e) => eprintln!("clipboard: {}", e),
            }
        });
    }

    fn paste(&mut self) {
        let Ok(mut c) = arboard::Clipboard::new() else { return };
        let mut paths: Vec<String> = c.get().file_list().unwrap_or_default().iter().map(|p| p.to_string_lossy().to_string()).collect();
        if paths.is_empty() {
            if let Ok(text) = c.get_text() {
                paths = text
                    .lines()
                    .map(|l| l.trim())
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .map(|l| util::url_to_local(l).unwrap_or_else(|| l.to_string()))
                    .filter(|l| std::path::Path::new(l).exists())
                    .collect();
            }
        }
        if !paths.is_empty() {
            self.load_paths(&paths);
        }
    }

    fn refresh_info(&mut self) {
        let d = &self.core.details;
        let path = d.file_path.clone();
        let (width, height) = d.base_image_size;
        let frames = self.core.movie.as_ref().map(|m| m.frame_count()).unwrap_or(0);
        let meta = std::fs::metadata(&path).ok();
        let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
        let t = |s: &str| tr("QVInfoDialog", s);
        let megapixels = ((width as f64 * height as f64) / 1_000_000.0 * 10.0).round() / 10.0;
        let modified = meta
            .and_then(|m| m.modified().ok())
            .map(|t| chrono::DateTime::<chrono::Local>::from(t).format("%A, %B %-d, %Y %-I:%M:%S %p %Z").to_string())
            .unwrap_or_default();
        let gcd = |mut a: i32, mut b: i32| {
            while b != 0 {
                (a, b) = (b, a % b);
            }
            a
        };
        let g = gcd(width, height);
        let mut rows = vec![
            (t("Name:"), util::file_name(&path)),
            (t("Type:"), decode::mime_for_file(&path, true)),
            (t("Location:"), util::parent_dir(&path)),
            (t("Size:"), t("%1 (%2 bytes)").replace("%1", &util::format_bytes(size)).replace("%2", &util::group_digits(size))),
            (t("Modified:"), modified),
            (
                t("Dimensions:"),
                t("%1 x %2 (%3 MP)").replace("%1", &width.to_string()).replace("%2", &height.to_string()).replace("%3", &settings::fmt_double(megapixels)),
            ),
            (t("Aspect Ratio:"), if g != 0 { format!("{}:{}", width / g, height / g) } else { String::new() }),
        ];
        if frames != 0 {
            rows.push((t("Frames:"), frames.to_string()));
        }
        self.info = rows;
    }

    // ---- input ----

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        for e in events {
            let egui::Event::Key { key, physical_key, pressed: true, modifiers, .. } = e else { continue };
            let combos = shortcuts::combos_for_event(key, physical_key, modifiers);
            if let Some(action) = self.shortcuts.action_for(&combos) {
                if self.action_enabled(action) {
                    self.trigger(action);
                }
            } else if key == egui::Key::Escape && self.fullscreen && !self.shortcuts.escape_is_bound() {
                self.toggle_full_screen();
            }
        }
    }

    fn handle_pointer(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let ctx = ui.ctx().clone();
        let response = ui.interact(rect, ui.scope_id().with("view"), egui::Sense::click_and_drag());
        let hover = ctx.input(|i| i.pointer.hover_pos());
        // hovered() is false when a window (File Info) is on top
        self.under_mouse = response.hovered() || self.hand_drag.is_some();
        let rel = |p: Pos2| ((p.x - rect.min.x).round() as i32, (p.y - rect.min.y).round() as i32);

        if response.double_clicked() {
            self.toggle_full_screen();
        }
        let events = ctx.input(|i| i.events.clone());
        let sm = settings::get();
        for e in events {
            match e {
                egui::Event::PointerButton { pos, button, pressed: true, modifiers } if rect.contains(pos) && response.hovered() => match button {
                    egui::PointerButton::Primary => {
                        if modifiers.ctrl && sm.bool(Setting::CtrlDragWindow) && !self.fullscreen && !self.maximized {
                            self.commands.push(ViewportCommand::StartDrag);
                        } else {
                            self.hand_drag = Some(pos);
                        }
                    }
                    egui::PointerButton::Middle => self.reset_scale(),
                    egui::PointerButton::Extra1 => self.go_to_file(GoTo::Previous),
                    egui::PointerButton::Extra2 => self.go_to_file(GoTo::Next),
                    _ => {}
                },
                egui::Event::PointerButton { button: egui::PointerButton::Primary, pressed: false, .. } => self.hand_drag = None,
                egui::Event::PointerMoved(pos) => {
                    if let Some(last) = self.hand_drag {
                        let (dx, dy) = ((pos.x - last.x).round(), (pos.y - last.y).round());
                        if dx != 0.0 || dy != 0.0 {
                            self.view.pan(dx as i64, dy as i64);
                            self.hand_drag = Some(Pos2::new(last.x + dx, last.y + dy));
                        }
                    }
                }
                egui::Event::MouseWheel { unit, delta, modifiers, .. } if self.under_mouse => {
                    let touch = unit == egui::MouseWheelUnit::Point;
                    let k = match unit {
                        egui::MouseWheelUnit::Line => 120.0,
                        egui::MouseWheelUnit::Page => 360.0,
                        // Touchpads report points; panning follows the fingers 1:1
                        egui::MouseWheelUnit::Point => 2.0,
                    };
                    let (ax, ay) = (delta.x as f64 * k, delta.y as f64 * k);
                    let scroll_zoom = sm.int(Setting::ScrollZoom);
                    let mut dont_zoom = scroll_zoom == 2;
                    if modifiers.ctrl {
                        dont_zoom = !dont_zoom;
                    }
                    if touch && scroll_zoom == 1 {
                        dont_zoom = !modifiers.ctrl;
                    }
                    if dont_zoom {
                        self.view.scroll(ax, ay, modifiers.shift);
                        continue;
                    }
                    if ay == 0.0 {
                        continue;
                    }
                    let mut f = sm.int(Setting::ScaleFactor) as f64 / 100.0;
                    if touch {
                        f *= ay.abs() / 120.0 * 6.0 * sm.int(Setting::TouchpadZoomSpeed).max(1) as f64 / 100.0;
                    } else if sm.bool(Setting::FractionalZoom) {
                        f *= ay.abs() / 120.0;
                    }
                    f += 1.0;
                    if ay < 0.0 {
                        f = 1.0 / f;
                    }
                    self.zoom(f, hover.map(rel));
                }
                egui::Event::Zoom(f) if self.under_mouse && f > 0.0 => self.zoom(f as f64, hover.map(rel)),
                _ => {}
            }
        }
        if self.hand_drag.is_some() {
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        }
    }

    // ---- drawing ----

    fn upload(&mut self, ctx: &egui::Context) {
        if !self.full_dirty {
            let want = settings::get().bool(Setting::FilteringEnabled) && self.view.current_scale < MAX_FILTERING_SIZE;
            if self.full.as_ref().map(|t| t.smooth == want).unwrap_or(true) {
                return;
            }
        }
        self.full_dirty = false;
        let img = if self.core.details.is_movie_loaded { self.core.current_movie_image() } else { self.core.loaded.clone() };
        let Some(img) = img else {
            self.full = None;
            return;
        };
        let _t = util::Trace::new("upload");
        let smooth = settings::get().bool(Setting::FilteringEnabled) && self.view.current_scale < MAX_FILTERING_SIZE;
        let opts = if smooth { TextureOptions { mipmap_mode: Some(egui::TextureFilter::Linear), ..TextureOptions::LINEAR } } else { TextureOptions::NEAREST };
        let max = ctx.input(|i| i.max_texture_side).max(256);
        let [w, h] = img.size;
        // Reuse the texture for animation frames of the same size
        if let Some(t) = self.full.as_mut() {
            if t.tiles.len() == 1 && t.smooth == smooth && t.tiles[0].0.size() == [w, h] {
                t.tiles[0].0.set(egui::ImageData::Color(img), opts);
                return;
            }
        }
        let mut tiles = vec![];
        if w <= max && h <= max {
            tiles.push((ctx.load_texture("image", egui::ImageData::Color(img), opts), Rect::from_min_size(Pos2::ZERO, Vec2::new(w as f32, h as f32))));
        } else {
            for ty in (0..h).step_by(max) {
                for tx in (0..w).step_by(max) {
                    let (tw, th) = ((w - tx).min(max), (h - ty).min(max));
                    let mut px = Vec::with_capacity(tw * th);
                    for row in ty..ty + th {
                        px.extend_from_slice(&img.pixels[row * w + tx..row * w + tx + tw]);
                    }
                    let tex = ctx.load_texture(format!("tile{tx},{ty}"), ColorImage::new([tw, th], px), opts);
                    tiles.push((tex, Rect::from_min_size(Pos2::new(tx as f32, ty as f32), Vec2::new(tw as f32, th as f32))));
                }
            }
        }
        self.full = Some(Tex { tiles, smooth });
    }

    fn draw(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let painter = ui.painter_at(rect);
        let bg = self.custom_bg.unwrap_or(ui.visuals().panel_fill);
        painter.rect_filled(rect, 0.0, bg);

        let ppp = ui.ctx().pixels_per_point();
        let snap = |v: f32| (v * ppp).round() / ppp;
        let ((ax, ay), _) = self.view.corners();
        let origin = Pos2::new(rect.min.x + ax as f32, rect.min.y + ay as f32);
        let snapped = Pos2::new(snap(origin.x), snap(origin.y));
        let shift = snapped - origin;
        let to_screen = |px: f64, py: f64| -> Pos2 {
            let (x, y) = self.view.map_from_scene((px, py));
            Pos2::new(rect.min.x + x as f32, rect.min.y + y as f32) + shift
        };
        let quad = |tex: &TextureHandle, a: Pos2, b: Pos2| {
            let mut mesh = egui::Mesh::with_texture(tex.id());
            mesh.add_rect_with_uv(Rect { min: a, max: b }, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            painter.add(egui::Shape::mesh(mesh));
        };

        let scaled = self.scaled.as_ref().filter(|s| s.item_gen == self.item_gen && s.scale == self.view.scale && !self.view.is_original_size);
        if let Some(s) = scaled {
            let size = Vec2::new(self.view.sx as f32 * s.size.x / ppp, self.view.sy as f32 * s.size.y / ppp);
            quad(&s.tex, snapped, snapped + size);
        } else if let Some(t) = &self.full {
            for (tex, r) in &t.tiles {
                quad(tex, to_screen(r.min.x as f64, r.min.y as f64), to_screen(r.max.x as f64, r.max.y as f64));
            }
        }

        let d = &self.core.details;
        if d.error_data.has_error {
            let e = &d.error_data;
            let msg = tr("MainWindow", "Error occurred opening\n%3\n%2 (Error %1)")
                .replace("%1", &e.error_num.to_string())
                .replace("%2", &e.error_string)
                .replace("%3", &util::file_name(&d.file_path));
            let light = util::perceived_brightness(bg.r() as f64 / 255.0, bg.g() as f64 / 255.0, bg.b() as f64 / 255.0) > 0.5;
            let color = if light { Color32::BLACK } else { Color32::WHITE };
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, msg, egui::FontId::proportional(15.0), color);
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if self.info_open {
            let mut open = true;
            egui::Window::new(tr("QVInfoDialog", "File Info")).open(&mut open).resizable(false).collapsible(false).show(ctx, |ui| {
                egui::Grid::new("info").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                    for (k, v) in &self.info {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| ui.label(k));
                        ui.add(egui::Label::new(v).wrap().selectable(true));
                        ui.end_row();
                    }
                });
            });
            self.info_open = open;
        }

        let Some(dialog) = self.dialog.take() else { return };
        // The key that opened the dialog (e.g. Ctrl+Return for rename) must not also confirm it
        let enter = ctx.input(|i| i.key_pressed(egui::Key::Enter)) && !self.dialog_new;
        self.dialog_new = false;
        let mut keep = true;
        match dialog {
            Dialog::Error { title, text } => {
                let r = egui::Modal::new(egui::Id::unique("error")).show(ctx, |ui| {
                    ui.heading(&title);
                    ui.label(&text);
                    ui.add_space(6.0);
                    ui.button("OK").clicked()
                });
                if r.inner || r.should_close() || enter {
                    keep = false;
                }
                if keep {
                    self.dialog = Some(Dialog::Error { title, text });
                }
            }
            Dialog::Delete { permanent, text, mut dont_ask } => {
                let mut yes = enter;
                let r = egui::Modal::new(egui::Id::unique("delete")).show(ctx, |ui| {
                    ui.heading(tr("MainWindow", "Delete"));
                    ui.label(&text);
                    if !permanent {
                        ui.checkbox(&mut dont_ask, tr("MainWindow", "Do not ask again").replace('&', ""));
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        yes |= ui.button("Yes").clicked();
                        ui.button("No").clicked()
                    })
                    .inner
                });
                if yes {
                    if !permanent {
                        Store::set("options", "askdelete", if dont_ask { "false" } else { "true" });
                        self.check_config_changed();
                    }
                    self.delete_file(permanent);
                } else if !(r.inner || r.should_close()) {
                    self.dialog = Some(Dialog::Delete { permanent, text, dont_ask });
                }
            }
            Dialog::Rename { path, mut name, init } => {
                let mut ok = false;
                let r = egui::Modal::new(egui::Id::unique("rename")).show(ctx, |ui| {
                    ui.heading(tr("QVRenameDialog", "Rename..."));
                    ui.label(tr("QVRenameDialog", "File name:"));
                    let out = egui::TextEdit::singleline(&mut name).desired_width(320.0).show(ui);
                    if init {
                        out.response.request_focus();
                        let stem = name.rfind('.').map(|i| name[..i].chars().count()).unwrap_or_else(|| name.chars().count());
                        let mut state = out.state;
                        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(stem))));
                        state.store(ui.ctx(), out.response.id);
                    }
                    ok |= enter;
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ok |= ui.button("OK").clicked();
                        ui.button("Cancel").clicked()
                    })
                    .inner
                });
                if ok {
                    self.rename_to(&path, &name);
                } else if !(r.inner || r.should_close()) {
                    self.dialog = Some(Dialog::Rename { path, name, init: false });
                }
            }
        }
    }

    fn next_deadline(&self) -> Option<Instant> {
        [self.core.next_frame_at(), self.slideshow_at, self.expensive_at].into_iter().flatten().min()
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.ctx_hint = ctx.clone();
        let now = Instant::now();

        while let Ok(msg) = self.loader_rx.try_recv() {
            self.core.on_loader_msg(msg);
            self.pump();
        }
        while let Ok(r) = self.scale_rx.try_recv() {
            if r.job == self.scale_job && r.item_gen == self.item_gen {
                let size = Vec2::new(r.image.size[0] as f32, r.image.size[1] as f32);
                let tex = ctx.load_texture("scaled", r.image, TextureOptions::NEAREST);
                self.scaled = Some(Scaled { tex, size, item_gen: r.item_gen, scale: r.scale });
            }
        }
        if let Some(rx) = &self.picked_rx {
            if let Ok(files) = rx.try_recv() {
                self.picked_rx = None;
                if let Some(first) = files.first() {
                    Store::set("recents", "lastFileDialogDir", &util::parent_dir(first));
                }
                self.load_paths(&files);
            }
        }
        if let Some(rx) = &self.save_rx {
            if let Ok(res) = rx.try_recv() {
                self.save_rx = None;
                if let Some((file, frame)) = res {
                    self.write_frame(file, frame);
                    self.reset_scale();
                }
            }
        }

        // Window state
        let (fs, max, focused, monitor, inner, close) = ctx.input(|i| {
            let v = i.viewport();
            (
                v.fullscreen.unwrap_or(false),
                v.maximized.unwrap_or(false),
                v.focused.unwrap_or(false),
                v.monitor_size.zip(v.native_pixels_per_point),
                v.inner_rect.map(|r| r.size()),
                v.close_requested(),
            )
        });
        self.fullscreen = fs;
        self.maximized = max;
        if !fs && !max {
            if let Some(s) = inner {
                self.normal_size = Some(s);
            }
        }
        if let Some((m, ppp)) = monitor {
            self.core.largest_dimension = ((m.x.max(m.y) * ppp) as i32).max(1);
        }
        if focused && !self.focused {
            self.check_config_changed();
        }
        self.focused = focused;
        if close {
            self.save_geometry();
        }

        if !self.started {
            self.started = true;
            if let Some(f) = self.initial_file.take() {
                self.open_file(&f);
            }
        }

        // Timers
        self.core.movie_tick(now);
        self.pump();
        if self.slideshow_at.map(|t| t <= now).unwrap_or(false) {
            if settings::get().bool(Setting::SlideshowReversed) {
                self.go_to_file(GoTo::Previous);
            } else {
                self.go_to_file(GoTo::Next);
            }
            if self.slideshow_at.is_some() {
                self.start_slideshow_timer();
            }
        }
        if self.expensive_at.map(|t| t <= now).unwrap_or(false) {
            self.expensive_at = None;
            self.scale_expensively_with(ctx.pixels_per_point() as f64);
        }

        if let Some(t) = self.next_deadline() {
            ctx.request_repaint_after(t.saturating_duration_since(Instant::now()));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let mut rect = ui.max_rect();

        if self.fullscreen && settings::get().bool(Setting::FullScreenDetails) {
            let font = egui::FontId::proportional(14.0);
            let h = ctx.fonts_mut(|f| f.row_height(&font)) + 6.0;
            let bar = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.min.y + h));
            let p = ui.painter_at(bar);
            p.rect_filled(bar, 0.0, ui.visuals().panel_fill);
            p.text(bar.min + Vec2::new(3.0, 3.0), egui::Align2::LEFT_TOP, &self.title, font, ui.visuals().text_color());
            rect.min.y += h;
        }

        let size = ((rect.width()).round() as i32, (rect.height()).round() as i32);
        if size != self.view.size {
            self.view.size = size;
            if !self.view.is_original_size {
                self.reset_scale();
            } else {
                self.view.center_on_item();
            }
        }

        if self.dialog.is_none() {
            self.handle_keys(&ctx);
            self.handle_pointer(ui, rect);
        } else {
            self.hand_drag = None;
        }
        if self.info_open && self.info.is_empty() {
            self.refresh_info();
        }

        self.upload(&ctx);
        self.draw(ui, rect);
        self.dialogs(&ctx);

        if self.title != self.sent_title {
            self.sent_title = self.title.clone();
            ctx.send_viewport_cmd(ViewportCommand::Title(self.title.clone()));
        }
        for c in self.commands.drain(..) {
            ctx.send_viewport_cmd(c);
        }
        if let Some(t) = self.next_deadline() {
            ctx.request_repaint_after(t.saturating_duration_since(Instant::now()));
        }
    }
}
