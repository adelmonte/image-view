//! Settings (qView's SettingsManager) and the on-disk config store.
//!
//! The config is an INI file in GLib key-file syntax at
//! `$XDG_CONFIG_HOME/image-view/image-view.conf`, with qView's groups and keys
//! ([General], [options], [recents], [shortcuts]). On first run an existing
//! g-view config is copied over.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Setting {
    BgColorEnabled = 0,
    BgColor,
    TitleBarMode,
    WindowResizeMode,
    MinWindowResizedPercentage,
    MaxWindowResizedPercentage,
    FullScreenDetails,
    CtrlDragWindow,
    FilteringEnabled,
    ScalingEnabled,
    ScalingTwoEnabled,
    ScaleFactor,
    TouchpadZoomSpeed,
    ScrollZoom,
    FractionalZoom,
    CursorZoom,
    CropMode,
    PastActualSizeEnabled,
    Language,
    SortMode,
    SortDescending,
    PreloadingMode,
    LoopFoldersEnabled,
    SlideshowReversed,
    SlideshowTimer,
    AfterDelete,
    AskDelete,
    AllowMimeContentDetection,
    SaveRecents,
    SkipHidden,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i32),
    Double(f64),
    Str(String),
}

impl Value {
    pub fn to_bool(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Int(i) => *i != 0,
            Value::Double(d) => *d != 0.0,
            Value::Str(s) => !(s.is_empty() || s == "0" || s.eq_ignore_ascii_case("false")),
        }
    }
    pub fn to_int(&self) -> i32 {
        match self {
            Value::Bool(b) => *b as i32,
            Value::Int(i) => *i,
            Value::Double(d) => d.round() as i32,
            Value::Str(s) => s.trim().parse().unwrap_or(0),
        }
    }
    pub fn to_double(&self) -> f64 {
        match self {
            Value::Bool(b) => *b as i32 as f64,
            Value::Int(i) => *i as f64,
            Value::Double(d) => *d,
            Value::Str(s) => s.trim().parse().unwrap_or(0.0),
        }
    }
    pub fn to_str(&self) -> String {
        match self {
            Value::Bool(b) => if *b { "true".into() } else { "false".into() },
            Value::Int(i) => i.to_string(),
            Value::Double(d) => fmt_double(*d),
            Value::Str(s) => s.clone(),
        }
    }
}

/// Format a double the way QVariant::toString does for "nice" values (5 -> "5", 2.5 -> "2.5").
pub fn fmt_double(d: f64) -> String {
    if d.fract() == 0.0 && d.abs() < 1e15 {
        format!("{}", d as i64)
    } else {
        let s = format!("{:.6}", d);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

#[derive(Clone, Copy)]
pub enum Kind {
    Bool,
    Int,
    Double,
    Str,
}

pub struct SettingDef {
    pub setting: Setting,
    pub key: &'static str,
    pub kind: Kind,
}

macro_rules! def {
    ($s:ident, $k:literal, $kind:ident) => {
        SettingDef { setting: Setting::$s, key: $k, kind: Kind::$kind }
    };
}

pub static DEFS: [SettingDef; 30] = [
    def!(BgColorEnabled, "bgcolorenabled", Bool),
    def!(BgColor, "bgcolor", Str),
    def!(TitleBarMode, "titlebarmode", Int),
    def!(WindowResizeMode, "windowresizemode", Int),
    def!(MinWindowResizedPercentage, "minwindowresizedpercentage", Int),
    def!(MaxWindowResizedPercentage, "maxwindowresizedpercentage", Int),
    def!(FullScreenDetails, "fullscreendetails", Bool),
    def!(CtrlDragWindow, "ctrldragwindow", Bool),
    def!(FilteringEnabled, "filteringenabled", Bool),
    def!(ScalingEnabled, "scalingenabled", Bool),
    def!(ScalingTwoEnabled, "scalingtwoenabled", Bool),
    def!(ScaleFactor, "scalefactor", Int),
    def!(TouchpadZoomSpeed, "touchpadzoomspeed", Int),
    def!(ScrollZoom, "scrollzoom", Int),
    def!(FractionalZoom, "fractionalzoom", Bool),
    def!(CursorZoom, "cursorzoom", Bool),
    def!(CropMode, "cropmode", Int),
    def!(PastActualSizeEnabled, "pastactualsizeenabled", Bool),
    def!(Language, "language", Str),
    def!(SortMode, "sortmode", Int),
    def!(SortDescending, "sortdescending", Bool),
    def!(PreloadingMode, "preloadingmode", Int),
    def!(LoopFoldersEnabled, "loopfoldersenabled", Bool),
    def!(SlideshowReversed, "slideshowreversed", Bool),
    def!(SlideshowTimer, "slideshowtimer", Double),
    def!(AfterDelete, "afterdelete", Int),
    def!(AskDelete, "askdelete", Bool),
    def!(AllowMimeContentDetection, "allowmimecontentdetection", Bool),
    def!(SaveRecents, "saverecents", Bool),
    def!(SkipHidden, "skiphidden", Bool),
];

pub fn default_value(s: Setting) -> Value {
    use Setting::*;
    use Value::*;
    match s {
        BgColorEnabled => Bool(true),
        BgColor => Str("#212121".into()),
        TitleBarMode => Int(1),
        WindowResizeMode => Int(1),
        MinWindowResizedPercentage => Int(20),
        MaxWindowResizedPercentage => Int(70),
        FullScreenDetails => Bool(false),
        CtrlDragWindow => Bool(false),
        FilteringEnabled => Bool(true),
        ScalingEnabled => Bool(true),
        ScalingTwoEnabled => Bool(true),
        ScaleFactor => Int(25),
        TouchpadZoomSpeed => Int(100),
        ScrollZoom => Int(1),
        FractionalZoom => Bool(false),
        CursorZoom => Bool(true),
        CropMode => Int(0),
        PastActualSizeEnabled => Bool(true),
        Language => Str("system".into()),
        SortMode => Int(0),
        SortDescending => Bool(false),
        PreloadingMode => Int(1),
        LoopFoldersEnabled => Bool(true),
        SlideshowReversed => Bool(false),
        SlideshowTimer => Int(5),
        AfterDelete => Int(2),
        AskDelete => Bool(true),
        AllowMimeContentDetection => Bool(false),
        SaveRecents => Bool(true),
        SkipHidden => Bool(true),
    }
}

pub fn parse_value(kind: Kind, s: &str) -> Value {
    match kind {
        Kind::Bool => Value::Bool(Value::Str(s.to_string()).to_bool()),
        Kind::Int => {
            let t = s.trim();
            Value::Int(t.parse::<i32>().unwrap_or_else(|_| t.parse::<f64>().map(|d| d as i32).unwrap_or(0)))
        }
        Kind::Double => Value::Double(s.trim().parse().unwrap_or(0.0)),
        Kind::Str => Value::Str(s.to_string()),
    }
}

// ---- paths ----------------------------------------------------------------------------------

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    match std::env::var_os(var) {
        Some(v) if !v.is_empty() && PathBuf::from(&v).is_absolute() => PathBuf::from(v),
        _ => home().join(fallback),
    }
}

pub fn home_dir() -> PathBuf {
    home()
}

pub fn user_config_dir() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config")
}

pub fn user_data_dir() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share")
}

pub fn config_dir() -> PathBuf {
    user_config_dir().join("image-view")
}

pub fn config_path() -> PathBuf {
    config_dir().join("image-view.conf")
}

/// First run: start from the g-view config if there is one.
pub fn migrate_if_needed() {
    let path = config_path();
    if path.exists() {
        return;
    }
    let old = user_config_dir().join("g-view").join("g-view.conf");
    if old.is_file() {
        let _ = std::fs::create_dir_all(config_dir());
        if std::fs::copy(&old, &path).is_ok() {
            eprintln!("Imported settings from {}", old.display());
        }
    }
}

// ---- key file -------------------------------------------------------------------------------

/// Minimal GLib key file: ordered groups of ordered `key=value` lines, values stored escaped.
#[derive(Default)]
pub struct KeyFile {
    groups: Vec<(String, Vec<(String, String)>)>,
}

fn unescape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut it = raw.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(o) => {
                out.push('\\');
                out.push(o);
            }
            None => out.push('\\'),
        }
    }
    out
}

fn escape(s: &str, list_item: bool) -> String {
    let mut v = String::with_capacity(s.len());
    for (i, c) in s.chars().enumerate() {
        match c {
            '\\' => v.push_str("\\\\"),
            ';' if list_item => v.push_str("\\;"),
            '\n' => v.push_str("\\n"),
            '\t' => v.push_str("\\t"),
            '\r' => v.push_str("\\r"),
            ' ' if i == 0 => v.push_str("\\s"),
            c => v.push(c),
        }
    }
    v
}

impl KeyFile {
    pub fn parse(text: &str) -> KeyFile {
        let mut kf = KeyFile::default();
        let mut current: Option<usize> = None;
        for line in text.lines() {
            let t = line.trim_start();
            if t.is_empty() || t.starts_with('#') {
                continue;
            }
            if t.starts_with('[') {
                if let Some(end) = t.find(']') {
                    let name = t[1..end].to_string();
                    current = Some(match kf.groups.iter().position(|(g, _)| *g == name) {
                        Some(i) => i,
                        None => {
                            kf.groups.push((name, vec![]));
                            kf.groups.len() - 1
                        }
                    });
                }
                continue;
            }
            let (Some(g), Some(eq)) = (current, t.find('=')) else { continue };
            let key = t[..eq].trim_end().to_string();
            let val = t[eq + 1..].trim_start().to_string();
            let entries = &mut kf.groups[g].1;
            match entries.iter_mut().find(|(k, _)| *k == key) {
                Some(e) => e.1 = val,
                None => entries.push((key, val)),
            }
        }
        kf
    }

    pub fn to_data(&self) -> String {
        let mut s = String::new();
        for (i, (g, entries)) in self.groups.iter().enumerate() {
            if i > 0 {
                s.push('\n');
            }
            s.push_str(&format!("[{}]\n", g));
            for (k, v) in entries {
                s.push_str(&format!("{}={}\n", k, v));
            }
        }
        s
    }

    fn raw(&self, group: &str, key: &str) -> Option<&str> {
        self.groups.iter().find(|(g, _)| g == group)?.1.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    fn set_raw(&mut self, group: &str, key: &str, value: String) {
        let gi = match self.groups.iter().position(|(g, _)| g == group) {
            Some(i) => i,
            None => {
                self.groups.push((group.to_string(), vec![]));
                self.groups.len() - 1
            }
        };
        let entries = &mut self.groups[gi].1;
        match entries.iter_mut().find(|(k, _)| k == key) {
            Some(e) => e.1 = value,
            None => entries.push((key.to_string(), value)),
        }
    }

    pub fn string(&self, group: &str, key: &str) -> Option<String> {
        self.raw(group, key).map(unescape)
    }

    pub fn string_list(&self, group: &str, key: &str) -> Option<Vec<String>> {
        let raw = self.raw(group, key)?;
        let mut out = vec![];
        let mut cur = String::new();
        let mut it = raw.chars();
        while let Some(c) = it.next() {
            match c {
                '\\' => match it.next() {
                    Some(';') => cur.push(';'),
                    Some(o) => {
                        cur.push('\\');
                        cur.push(o);
                    }
                    None => cur.push('\\'),
                },
                ';' => out.push(unescape(&std::mem::take(&mut cur))),
                c => cur.push(c),
            }
        }
        if !cur.is_empty() {
            out.push(unescape(&cur));
        }
        Some(out)
    }

    pub fn set_string(&mut self, group: &str, key: &str, value: &str) {
        self.set_raw(group, key, escape(value, false));
    }

    #[cfg(test)]
    pub fn set_string_list(&mut self, group: &str, key: &str, list: &[String]) {
        let mut v = String::new();
        for item in list {
            v.push_str(&escape(item, true));
            v.push(';');
        }
        self.set_raw(group, key, v);
    }
}

/// Read-modify-write access to the config file. Every write re-reads the file first so that
/// several running windows don't clobber each other.
pub struct Store;

impl Store {
    pub fn load() -> KeyFile {
        KeyFile::parse(&std::fs::read_to_string(config_path()).unwrap_or_default())
    }

    fn save(kf: &KeyFile) {
        let _ = std::fs::create_dir_all(config_dir());
        let path = config_path();
        let tmp = path.with_extension("conf.tmp");
        if std::fs::write(&tmp, kf.to_data().as_bytes()).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    pub fn get(group: &str, key: &str) -> Option<String> {
        Self::load().string(group, key)
    }

    pub fn set(group: &str, key: &str, value: &str) {
        let mut kf = Self::load();
        kf.set_string(group, key, value);
        Self::save(&kf);
    }

}

// ---- SettingsManager ------------------------------------------------------------------------

pub struct SettingsManager {
    values: RefCell<Vec<Value>>,
    mtime: RefCell<Option<std::time::SystemTime>>,
}

thread_local! {
    static SETTINGS: Rc<SettingsManager> = Rc::new(SettingsManager::new());
}

/// The process-wide settings (lives on the UI thread)
pub fn get() -> Rc<SettingsManager> {
    SETTINGS.with(|s| s.clone())
}

impl SettingsManager {
    fn new() -> Self {
        let sm = SettingsManager {
            values: RefCell::new(DEFS.iter().map(|d| default_value(d.setting)).collect()),
            mtime: RefCell::new(None),
        };
        sm.load_settings();
        sm
    }

    /// Returns true if any value changed.
    pub fn load_settings(&self) -> bool {
        *self.mtime.borrow_mut() = std::fs::metadata(config_path()).and_then(|m| m.modified()).ok();
        let kf = Store::load();
        let mut changed = false;
        let mut values = self.values.borrow_mut();
        for (i, def) in DEFS.iter().enumerate() {
            let new = kf.string("options", def.key).map(|s| parse_value(def.kind, &s)).unwrap_or_else(|| default_value(def.setting));
            if values[i] != new {
                changed = true;
                values[i] = new;
            }
        }
        changed
    }

    fn value(&self, s: Setting) -> Value {
        self.values.borrow()[s as usize].clone()
    }

    pub fn bool(&self, s: Setting) -> bool {
        self.value(s).to_bool()
    }
    pub fn int(&self, s: Setting) -> i32 {
        self.value(s).to_int()
    }
    pub fn double(&self, s: Setting) -> f64 {
        self.value(s).to_double()
    }
    pub fn string(&self, s: Setting) -> String {
        self.value(s).to_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyfile_lists_and_escapes() {
        let kf = KeyFile::parse("[shortcuts]\nresetspeed=\\\\;\nzoomin=Ctrl++;Ctrl+=;\nslideshow=\nsemi=Ctrl+\\;;\n[options]\nbgcolor=#4c566a\n");
        assert_eq!(kf.string_list("shortcuts", "resetspeed").unwrap(), vec!["\\"]);
        assert_eq!(kf.string_list("shortcuts", "zoomin").unwrap(), vec!["Ctrl++", "Ctrl+="]);
        assert!(kf.string_list("shortcuts", "slideshow").unwrap().is_empty());
        assert_eq!(kf.string_list("shortcuts", "semi").unwrap(), vec!["Ctrl+;"]);
        assert_eq!(kf.string("options", "bgcolor").unwrap(), "#4c566a");

        let mut kf = KeyFile::default();
        kf.set_string_list("s", "k", &["\\".into(), "a;b".into()]);
        let back = KeyFile::parse(&kf.to_data());
        assert_eq!(back.string_list("s", "k").unwrap(), vec!["\\", "a;b"]);
    }
}
