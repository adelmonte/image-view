//! Port of QVImageCore: file loading, folder listing & sorting, preloading cache, animation.
//!
//! Methods never call back into the UI; they queue `CoreEvent`s which the app drains afterwards,
//! in the order Qt would have emitted its signals.

use crate::decode::{self, ErrorData, Frame, Image, ReadData};
use crate::settings::{self, Setting};
use crate::util;
use std::collections::VecDeque;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CoreEvent {
    FileChanged,
    UpdateLoadedPixmapItem,
    AnimatedFrameChanged,
}

pub enum LoaderMsg {
    Loaded(ReadData),
    Preloaded(ReadData),
}

#[derive(Clone, Debug)]
pub struct CompatibleFile {
    pub absolute_file_path: String,
    pub file_name: String,
    pub last_modified: i64,
    pub last_created: i64,
    pub size: i64,
    pub mime_type: String,
}

#[derive(Clone, Default)]
pub struct FileDetails {
    /// absolute file path of the loaded/requested file ("" = none)
    pub file_path: String,
    pub folder_file_info_list: Vec<CompatibleFile>,
    pub loaded_index_in_folder: i32,
    pub is_load_requested: bool,
    pub is_pixmap_loaded: bool,
    pub is_movie_loaded: bool,
    pub base_image_size: (i32, i32),
    pub loaded_pixmap_size: (i32, i32),
    pub time_since_loaded: Option<Instant>,
    pub error_data: ErrorData,
}

impl FileDetails {
    pub fn update_loaded_index_in_folder(&mut self) {
        self.loaded_index_in_folder = -1;
        if self.file_path.is_empty() {
            return;
        }
        if let Some(i) = self.folder_file_info_list.iter().position(|f| f.absolute_file_path == self.file_path) {
            self.loaded_index_in_folder = i as i32;
        }
    }
    pub fn is_file(&self) -> bool {
        !self.file_path.is_empty() && std::path::Path::new(&self.file_path).is_file()
    }
    pub fn file_name(&self) -> String {
        util::file_name(&self.file_path)
    }
    pub fn file_size(&self) -> u64 {
        std::fs::metadata(&self.file_path).map(|m| m.len()).unwrap_or(0)
    }
}

#[derive(PartialEq, Clone, Default)]
struct DirInfo {
    dir_path: String,
    file_count: usize,
    sort_mode: i32,
    sort_descending: bool,
}

pub struct Movie {
    pub frames: Arc<Vec<Frame>>,
    pub current: usize,
    pub speed: i32,
    pub paused: bool,
    /// When the next frame is due (None = not playing)
    pub next_at: Option<Instant>,
}

impl Movie {
    pub fn frame_count(&self) -> usize {
        self.frames.len()
    }
    pub fn current_image(&self) -> &Image {
        &self.frames[self.current].image
    }
    fn schedule(&mut self) {
        self.next_at = if self.paused || self.speed == 0 {
            None
        } else {
            let d = self.frames[self.current].delay_ms.max(10) as u64;
            Some(Instant::now() + Duration::from_millis(d * 100 / self.speed as u64))
        };
    }
}

// ---- image cache (QCache<QString, ReadData>) ------------------------------------------------

struct Cache {
    entries: VecDeque<(String, ReadData, usize)>, // front = most recently used
    max_cost: usize,
    total: usize,
}

impl Cache {
    fn trim(&mut self, limit: usize) {
        while self.total > limit {
            match self.entries.pop_back() {
                Some((_, _, c)) => self.total -= c,
                None => break,
            }
        }
    }
    fn set_max_cost(&mut self, c: usize) {
        self.max_cost = c;
        self.trim(c);
    }
    fn take(&mut self, key: &str) -> Option<ReadData> {
        let pos = self.entries.iter().position(|(k, _, _)| k == key)?;
        let (_, rd, c) = self.entries.remove(pos)?;
        self.total -= c;
        Some(rd)
    }
    fn contains(&self, key: &str) -> bool {
        self.entries.iter().any(|(k, _, _)| k == key)
    }
    fn insert(&mut self, key: String, rd: ReadData, cost: usize) {
        if let Some(pos) = self.entries.iter().position(|(k, _, _)| *k == key) {
            let (_, _, c) = self.entries.remove(pos).unwrap();
            self.total -= c;
        }
        if cost > self.max_cost {
            return;
        }
        self.trim(self.max_cost - cost);
        self.total += cost;
        self.entries.push_front((key, rd, cost));
    }
    fn clear(&mut self) {
        self.entries.clear();
        self.total = 0;
    }
}

fn cache_key(path: &str, size: u64) -> String {
    format!("{}\n{}", path, size)
}

pub struct ImageCore {
    /// The loaded image with the current rotation applied
    pub loaded: Option<Image>,
    pub movie: Option<Movie>,
    pub details: FileDetails,
    pub current_rotation: i32,
    last_dir_info: DirInfo,
    random_sort_seed: u64,
    last_files_preloaded: Vec<String>,
    preload_files_in_progress: Vec<String>,
    waiting_on_preload_file: String,
    pub largest_dimension: i32,
    waiting_on_load: bool,
    cache: Cache,
    extensions: Vec<String>,
    pub pending: Vec<CoreEvent>,
    sender: Sender<LoaderMsg>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl ImageCore {
    pub fn new(sender: Sender<LoaderMsg>, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let mut core = ImageCore {
            loaded: None,
            movie: None,
            details: FileDetails { loaded_index_in_folder: -1, ..Default::default() },
            current_rotation: 0,
            last_dir_info: DirInfo::default(),
            random_sort_seed: 0,
            last_files_preloaded: vec![],
            preload_files_in_progress: vec![],
            waiting_on_preload_file: String::new(),
            largest_dimension: 1920,
            waiting_on_load: false,
            cache: Cache { entries: VecDeque::new(), max_cost: 100 * 1024, total: 0 },
            extensions: decode::extensions(),
            pending: vec![],
            sender,
            wake,
        };
        core.settings_updated();
        core
    }

    fn emit(&mut self, e: CoreEvent) {
        self.pending.push(e);
    }

    pub fn load_file(&mut self, file_name: &str, is_reloading: bool) {
        if self.waiting_on_load {
            return;
        }
        let sanitary = util::url_to_local(file_name).unwrap_or_else(|| file_name.to_string());
        let path = util::absolute_clean(&sanitary);

        if std::path::Path::new(&path).is_dir() {
            self.update_folder_info(Some(path));
            if self.details.folder_file_info_list.is_empty() {
                self.close_image();
            } else {
                let first = self.details.folder_file_info_list[0].absolute_file_path.clone();
                self.load_file(&first, false);
            }
            return;
        }

        // Pause playing movie because it feels better that way
        self.set_paused(true);

        self.details.is_load_requested = true;
        self.waiting_on_load = true;

        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let key = cache_key(&path, size);

        let cached = if is_reloading { None } else { self.cache.take(&key) };
        if let Some(rd) = cached {
            self.load_pixmap(rd);
        } else if self.preload_files_in_progress.contains(&path) {
            self.waiting_on_preload_file = path;
        } else {
            self.spawn_read(path, false);
        }
    }

    fn spawn_read(&self, path: String, preload: bool) {
        let sender = self.sender.clone();
        let wake = self.wake.clone();
        let largest = self.largest_dimension;
        std::thread::spawn(move || {
            let rd = decode::read_file(&path, largest);
            let msg = if preload { LoaderMsg::Preloaded(rd) } else { LoaderMsg::Loaded(rd) };
            if sender.send(msg).is_ok() {
                wake();
            }
        });
    }

    pub fn on_loader_msg(&mut self, msg: LoaderMsg) {
        match msg {
            LoaderMsg::Loaded(rd) => self.load_pixmap(rd),
            LoaderMsg::Preloaded(rd) => {
                let path = rd.absolute_file_path.clone();
                if self.waiting_on_preload_file == path {
                    self.waiting_on_preload_file.clear();
                    self.load_pixmap(rd);
                } else {
                    self.add_to_cache(rd);
                }
                self.preload_files_in_progress.retain(|p| *p != path);
            }
        }
    }

    fn load_pixmap(&mut self, rd: ReadData) {
        if rd.error_data.has_error {
            self.details = self.get_empty_file_details();
            self.details.error_data = rd.error_data.clone();
        } else {
            self.details.error_data = ErrorData::default();
        }

        // Do this first so we can keep folder info even when loading errored files
        self.details.file_path = rd.absolute_file_path.clone();
        self.details.update_loaded_index_in_folder();
        if self.details.loaded_index_in_folder == -1 {
            self.update_folder_info(None);
        }

        // Reset mechanism to avoid stalling while loading
        self.waiting_on_load = false;

        let Some(image) = rd.image.as_ref().filter(|_| !self.details.error_data.has_error) else {
            self.load_empty_pixmap();
            return;
        };
        let img = decode::rotated(image, self.current_rotation);

        self.details.is_pixmap_loaded = true;
        self.details.base_image_size = rd.image_size;
        self.details.loaded_pixmap_size = (decode::width(&img), decode::height(&img));
        if self.details.base_image_size == (-1, -1) {
            self.details.base_image_size = self.details.loaded_pixmap_size;
        }
        self.loaded = Some(img);

        self.movie = rd.frames.clone().map(|frames| {
            let mut m = Movie { frames, current: 0, speed: 100, paused: false, next_at: None };
            m.schedule();
            m
        });
        self.details.is_movie_loaded = self.movie.is_some();

        self.add_to_cache(rd);
        self.details.time_since_loaded = Some(Instant::now());
        self.emit(CoreEvent::FileChanged);
        self.request_caching();
    }

    pub fn close_image(&mut self) {
        self.details = self.get_empty_file_details();
        self.load_empty_pixmap();
    }

    fn load_empty_pixmap(&mut self) {
        self.loaded = None;
        self.movie = None;
        self.emit(CoreEvent::FileChanged);
    }

    fn get_empty_file_details(&self) -> FileDetails {
        FileDetails {
            folder_file_info_list: self.details.folder_file_info_list.clone(),
            loaded_index_in_folder: self.details.loaded_index_in_folder,
            ..Default::default()
        }
    }

    fn get_compatible_files(&self, dir_path: &str) -> Vec<CompatibleFile> {
        let sm = settings::get();
        let sort_mode = sm.int(Setting::SortMode);
        let allow_content = sm.bool(Setting::AllowMimeContentDetection);
        let skip_hidden = sm.bool(Setting::SkipHidden);

        let mut list = vec![];
        let Ok(rd) = std::fs::read_dir(dir_path) else { return list };
        for entry in rd.flatten() {
            let file_name = entry.file_name().to_string_lossy().to_string();
            if skip_hidden && file_name.starts_with('.') {
                continue;
            }
            let path = entry.path();
            // QDir::Files follows symlinks to files
            let Ok(meta) = std::fs::metadata(&path) else { continue };
            if !meta.is_file() {
                continue;
            }
            let abs = path.to_string_lossy().to_string();
            let lower = file_name.to_lowercase();
            let mut matched = self.extensions.iter().any(|e| lower.ends_with(e.as_str()));
            if !matched && allow_content {
                matched = decode::content_is_supported(&abs);
            }
            // ignore macOS ._ metadata files
            if file_name.starts_with("._") {
                matched = false;
            }
            if matched {
                let ms = |t: std::io::Result<SystemTime>| t.ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64).unwrap_or(0);
                list.push(CompatibleFile {
                    mime_type: if sort_mode == 4 { decode::mime_for_file(&abs, allow_content) } else { String::new() },
                    absolute_file_path: abs,
                    file_name,
                    last_modified: if sort_mode == 1 { ms(meta.modified()) } else { 0 },
                    last_created: if sort_mode == 2 { ms(meta.created()) } else { 0 },
                    size: if sort_mode == 3 { meta.len() as i64 } else { 0 },
                });
            }
        }
        list
    }

    pub fn update_folder_info(&mut self, dir_path: Option<String>) {
        let dir_path = match dir_path {
            Some(d) => d,
            None => {
                if self.details.file_path.is_empty() {
                    return;
                }
                util::parent_dir(&self.details.file_path)
            }
        };

        let mut list = self.get_compatible_files(&dir_path);
        let (sort_mode, desc) = {
            let sm = settings::get();
            (sm.int(Setting::SortMode), sm.bool(Setting::SortDescending))
        };
        let dir_info = DirInfo { dir_path, file_count: list.len(), sort_mode, sort_descending: desc };
        if self.last_dir_info != dir_info {
            self.random_sort_seed = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
        }
        self.last_dir_info = dir_info;

        use std::cmp::Ordering;
        let rev = |o: Ordering| if desc { o.reverse() } else { o };
        match sort_mode {
            0 => list.sort_by(|a, b| rev(util::natural_cmp(&a.file_name, &b.file_name))),
            // Note: qView sorts dates and sizes newest/largest first when "ascending"
            1 => list.sort_by(|a, b| rev(b.last_modified.cmp(&a.last_modified))),
            2 => list.sort_by(|a, b| rev(b.last_created.cmp(&a.last_created))),
            3 => list.sort_by(|a, b| rev(b.size.cmp(&a.size))),
            4 => list.sort_by(|a, b| rev(util::natural_cmp(&a.mime_type, &b.mime_type))),
            5 => {
                use rand::seq::SliceRandom;
                use rand::SeedableRng;
                let mut rng = rand::rngs::StdRng::seed_from_u64(self.random_sort_seed);
                list.shuffle(&mut rng);
            }
            _ => {}
        }
        self.details.folder_file_info_list = list;
        self.details.update_loaded_index_in_folder();
    }

    fn request_caching(&mut self) {
        let (mode, loop_folders) = {
            let sm = settings::get();
            (sm.int(Setting::PreloadingMode), sm.bool(Setting::LoopFoldersEnabled))
        };
        if mode == 0 {
            self.cache.clear();
            return;
        }
        let distance: i32 = if mode > 1 { 4 } else { 1 };
        let cur = self.details.loaded_index_in_folder;
        let len = self.details.folder_file_info_list.len() as i32;
        let mut files_to_preload = vec![];
        for i in (cur - distance)..=(cur + distance) {
            let mut index = i;
            if index == cur {
                continue;
            }
            if loop_folders {
                if index > len - 1 {
                    index -= len;
                } else if index < 0 {
                    index += len;
                }
            }
            if index > len - 1 || index < 0 || len == 0 {
                continue;
            }
            let path = self.details.folder_file_info_list[index as usize].absolute_file_path.clone();
            files_to_preload.push(path.clone());
            self.request_caching_file(path);
        }
        self.last_files_preloaded = files_to_preload;
    }

    fn request_caching_file(&mut self, path: String) {
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let key = cache_key(&path, size);
        if self.cache.contains(&key) || self.last_files_preloaded.contains(&path) {
            return;
        }
        if (size / 1024) as usize > self.cache.max_cost / 2 {
            return;
        }
        if self.preload_files_in_progress.contains(&path) {
            return;
        }
        self.preload_files_in_progress.push(path.clone());
        self.spawn_read(path, true);
    }

    fn add_to_cache(&mut self, rd: ReadData) {
        if rd.image.is_none() {
            return;
        }
        let key = cache_key(&rd.absolute_file_path, rd.file_size);
        let cost = rd.cost_kib();
        self.cache.insert(key, rd, cost);
    }

    pub fn settings_updated(&mut self) {
        // Cost is in KiB
        let cost = match settings::get().int(Setting::PreloadingMode) {
            0 => 0,
            2 => 2_048_000,
            _ => 256_000,
        };
        self.cache.set_max_cost(cost);
        // update folder info to reflect new settings (e.g. sort order)
        self.update_folder_info(None);
    }

    // ---- movie controls ----

    pub fn jump_to_next_frame(&mut self) {
        let Some(m) = self.movie.as_mut() else { return };
        m.current = (m.current + 1) % m.frames.len();
        self.emit(CoreEvent::AnimatedFrameChanged);
    }

    pub fn set_paused(&mut self, paused: bool) {
        if let Some(m) = self.movie.as_mut() {
            if m.paused != paused {
                m.paused = paused;
                m.schedule();
            }
        }
    }

    pub fn is_paused(&self) -> bool {
        self.movie.as_ref().map(|m| m.paused).unwrap_or(false)
    }

    pub fn set_speed(&mut self, speed: i32) {
        if let Some(m) = self.movie.as_mut() {
            m.speed = speed.clamp(0, 1000);
            m.schedule();
        }
    }

    pub fn speed(&self) -> i32 {
        self.movie.as_ref().map(|m| m.speed).unwrap_or(100)
    }

    pub fn next_frame_at(&self) -> Option<Instant> {
        self.movie.as_ref()?.next_at
    }

    /// Advance the animation if a frame is due
    pub fn movie_tick(&mut self, now: Instant) {
        let Some(m) = self.movie.as_mut() else { return };
        match m.next_at {
            Some(t) if t <= now => {}
            _ => return,
        }
        m.current = (m.current + 1) % m.frames.len();
        m.schedule();
        self.emit(CoreEvent::AnimatedFrameChanged);
    }

    /// Current movie frame with the current rotation applied
    pub fn current_movie_image(&self) -> Option<Image> {
        let m = self.movie.as_ref()?;
        Some(decode::rotated(m.current_image(), self.current_rotation))
    }

    pub fn rotate_image(&mut self, rotation: i32) {
        self.current_rotation = ((self.current_rotation + rotation) % 360 + 360) % 360;
        let new = if self.details.is_movie_loaded { self.current_movie_image() } else { self.loaded.as_ref().map(|r| decode::rotated(r, rotation)) };
        if let Some(r) = new {
            self.details.loaded_pixmap_size = (decode::width(&r), decode::height(&r));
            self.loaded = Some(r);
        }
        self.emit(CoreEvent::UpdateLoadedPixmapItem);
    }
}
