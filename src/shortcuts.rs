//! Port of ShortcutManager, plus parsing/matching of Qt key sequence strings
//! (e.g. "Ctrl+Shift+O", "Ctrl++") against egui key events.

use crate::settings::Store;
use eframe::egui::{Key, Modifiers};

#[derive(Clone, Debug)]
pub struct SShortcut {
    pub name: &'static str,
    pub default_shortcuts: Vec<String>,
    pub shortcuts: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyCombo {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
    pub key: String,
}

impl KeyCombo {
    pub fn parse(s: &str) -> Option<KeyCombo> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        let mut kc = KeyCombo { ctrl: false, shift: false, alt: false, meta: false, key: String::new() };
        let mut rest = s;
        loop {
            let lower = rest.to_ascii_lowercase();
            let take = |p: &str| lower.starts_with(p) && rest.len() > p.len();
            if take("ctrl+") {
                kc.ctrl = true;
                rest = &rest[5..];
            } else if take("shift+") {
                kc.shift = true;
                rest = &rest[6..];
            } else if take("alt+") {
                kc.alt = true;
                rest = &rest[4..];
            } else if take("meta+") {
                kc.meta = true;
                rest = &rest[5..];
            } else if take("num+") {
                rest = &rest[4..];
            } else {
                break;
            }
        }
        kc.key = normalize_key_name(rest);
        Some(kc)
    }
}

fn normalize_key_name(name: &str) -> String {
    if name.chars().count() == 1 {
        return name.to_uppercase();
    }
    let lower = name.to_ascii_lowercase();
    let n = match lower.as_str() {
        "esc" | "escape" => "Esc",
        "del" | "delete" => "Del",
        "ins" | "insert" => "Ins",
        // egui doesn't tell the keypad Enter apart from Return
        "return" | "enter" => "Return",
        "backspace" => "Backspace",
        "tab" => "Tab",
        "backtab" => "Backtab",
        "space" => "Space",
        "home" => "Home",
        "end" => "End",
        "left" => "Left",
        "right" => "Right",
        "up" => "Up",
        "down" => "Down",
        "pgup" | "pageup" => "PgUp",
        "pgdown" | "pagedown" => "PgDown",
        "copy" => "Copy",
        "cut" => "Cut",
        "paste" => "Paste",
        _ => {
            if lower.starts_with('f') && lower[1..].parse::<u32>().is_ok() {
                return format!("F{}", &lower[1..]);
            }
            return name.to_string();
        }
    };
    n.to_string()
}

/// Qt key name for an egui key
fn key_name(k: Key) -> String {
    let n = match k {
        Key::ArrowDown => "Down",
        Key::ArrowLeft => "Left",
        Key::ArrowRight => "Right",
        Key::ArrowUp => "Up",
        Key::Escape => "Esc",
        Key::Enter => "Return",
        Key::Insert => "Ins",
        Key::Delete => "Del",
        Key::PageUp => "PgUp",
        Key::PageDown => "PgDown",
        Key::Minus => "-",
        Key::Quote => "'",
        Key::Space => "Space",
        other => other.symbol_or_name(),
    };
    n.to_string()
}

/// All combos a key press could match: the logical key, then the key at the same physical
/// position on a US layout (so shortcuts keep working with non-Latin layouts).
pub fn combos_for_event(key: Key, physical: Option<Key>, m: Modifiers) -> Vec<KeyCombo> {
    let mut out: Vec<KeyCombo> = vec![];
    let mut names = vec![key_name(key)];
    if let Some(p) = physical {
        let n = key_name(p);
        if !names.contains(&n) {
            names.push(n);
        }
    }
    for n in names {
        let is_letter = n.chars().count() == 1 && n.chars().next().unwrap().is_alphabetic();
        let mut push = |shift: bool| {
            let c = KeyCombo { ctrl: m.ctrl, shift, alt: m.alt, meta: false, key: n.clone() };
            if !out.contains(&c) {
                out.push(c);
            }
        };
        push(m.shift);
        // "Ctrl++" is typed as Ctrl+Shift+= on most layouts
        if m.shift && !is_letter && n.chars().count() == 1 {
            push(false);
        }
    }
    out
}

pub struct ShortcutManager {
    pub list: Vec<SShortcut>,
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

impl ShortcutManager {
    pub fn new() -> Self {
        let mut sm = ShortcutManager { list: Self::initial_list() };
        sm.update_shortcuts();
        sm
    }

    fn initial_list() -> Vec<SShortcut> {
        let mk = |name: &'static str, defaults: Vec<String>| SShortcut { name, default_shortcuts: defaults, shortcuts: vec![] };
        // Defaults are what Qt's QKeySequence standard bindings resolve to on Linux.
        vec![
            mk("open", s(&["Ctrl+O"])),
            mk("newwindow", s(&["Ctrl+N"])),
            mk("reloadfile", s(&["F5"])),
            mk("opencontainingfolder", vec![]),
            mk("showfileinfo", s(&["I"])),
            mk("undo", s(&["Ctrl+Z"])),
            mk("copy", s(&["Ctrl+C", "Ctrl+Ins"])),
            mk("paste", s(&["Ctrl+V", "Shift+Ins"])),
            mk("rename", s(&["Ctrl+Return", "F2", "Ctrl+R"])),
            mk("delete", s(&["Ctrl+Backspace", "Del"])),
            mk("deletepermanent", s(&["Shift+Del"])),
            mk("firstfile", s(&["Home"])),
            mk("previousfile", s(&["Left"])),
            mk("nextfile", s(&["Right"])),
            mk("lastfile", s(&["End"])),
            mk("randomfile", s(&["R"])),
            mk("zoomin", s(&["Ctrl++", "Ctrl+="])),
            mk("zoomout", s(&["Ctrl+-"])),
            mk("resetzoom", s(&["Ctrl+0"])),
            mk("originalsize", s(&["O"])),
            mk("rotateright", s(&["Up"])),
            mk("rotateleft", s(&["Down"])),
            mk("mirror", s(&["F"])),
            mk("flip", s(&["Ctrl+F"])),
            mk("fullscreen", s(&["F11"])),
            mk("saveframeas", s(&["Ctrl+S"])),
            mk("pause", s(&["P"])),
            mk("nextframe", s(&["N"])),
            mk("decreasespeed", s(&["["])),
            mk("resetspeed", s(&["\\"])),
            mk("increasespeed", s(&["]"])),
            mk("slideshow", vec![]),
            mk("quit", s(&["Ctrl+Q", "Ctrl+W"])),
        ]
    }

    /// Re-read user shortcuts from the config (ShortcutManager::updateShortcuts)
    pub fn update_shortcuts(&mut self) {
        let kf = Store::load();
        for sc in &mut self.list {
            sc.shortcuts = match kf.string_list("shortcuts", sc.name) {
                Some(l) => l.into_iter().filter(|s| !s.is_empty()).collect(),
                None => sc.default_shortcuts.clone(),
            };
        }
    }

    /// Find the action bound to a key press
    pub fn action_for(&self, combos: &[KeyCombo]) -> Option<&'static str> {
        if combos.is_empty() {
            return None;
        }
        for sc in &self.list {
            for s in &sc.shortcuts {
                if let Some(kc) = KeyCombo::parse(s) {
                    if combos.contains(&kc) {
                        return Some(sc.name);
                    }
                }
            }
        }
        None
    }

    pub fn escape_is_bound(&self) -> bool {
        self.list.iter().any(|s| {
            s.shortcuts
                .iter()
                .any(|k| KeyCombo::parse(k).map(|c| c.key == "Esc" && !c.ctrl && !c.shift && !c.alt && !c.meta).unwrap_or(false))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parse_plus() {
        let k = KeyCombo::parse("Ctrl++").unwrap();
        assert!(k.ctrl && k.key == "+");
        let k = KeyCombo::parse("Ctrl+Shift+O").unwrap();
        assert!(k.ctrl && k.shift && k.key == "O");
        assert_eq!(KeyCombo::parse("\\").unwrap().key, "\\");
        let k = KeyCombo::parse("Shift+Del").unwrap();
        assert!(k.shift && k.key == "Del");
        assert_eq!(KeyCombo::parse("f11").unwrap().key, "F11");
    }

    #[test]
    fn shifted_symbol_matches() {
        let m = Modifiers { ctrl: true, shift: true, ..Default::default() };
        let combos = combos_for_event(Key::Plus, Some(Key::Equals), m);
        assert!(combos.contains(&KeyCombo::parse("Ctrl++").unwrap()));
        let combos = combos_for_event(Key::Minus, Some(Key::Minus), Modifiers { ctrl: true, ..Default::default() });
        assert!(combos.contains(&KeyCombo::parse("Ctrl+-").unwrap()));
    }
}
