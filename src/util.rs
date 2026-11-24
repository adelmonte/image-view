use std::cmp::Ordering;
use std::path::{Component, Path, PathBuf};

/// Locale-ish natural comparison (QCollator with numeric mode): case-insensitive,
/// digit runs compared by value.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.chars().peekable();
    let mut bi = b.chars().peekable();
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => break,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let mut na = String::new();
                    while let Some(c) = ai.peek().copied().filter(|c| c.is_ascii_digit()) {
                        na.push(c);
                        ai.next();
                    }
                    let mut nb = String::new();
                    while let Some(c) = bi.peek().copied().filter(|c| c.is_ascii_digit()) {
                        nb.push(c);
                        bi.next();
                    }
                    let ta = na.trim_start_matches('0');
                    let tb = nb.trim_start_matches('0');
                    let o = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                    if o != Ordering::Equal {
                        return o;
                    }
                } else {
                    let la: String = ca.to_lowercase().collect();
                    let lb: String = cb.to_lowercase().collect();
                    // punctuation sorts before alphanumerics, like ICU
                    let ka = (ca.is_alphanumeric(), la);
                    let kb = (cb.is_alphanumeric(), lb);
                    let o = ka.cmp(&kb);
                    if o != Ordering::Equal {
                        return o;
                    }
                    ai.next();
                    bi.next();
                }
            }
        }
    }
    // tie-break: lowercase before uppercase (ICU tertiary), then raw
    let lower_first = |s: &str| s.chars().map(|c| if c.is_uppercase() { 1u8 } else { 0 }).collect::<Vec<_>>();
    lower_first(a).cmp(&lower_first(b)).then_with(|| a.cmp(b))
}

/// QLocale::formattedDataSize (IEC, 2 decimals)
pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{} bytes", bytes);
    }
    let units = ["KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    let mut size = bytes as f64 / 1024.0;
    let mut i = 0;
    while size >= 1024.0 && i < units.len() - 1 {
        size /= 1024.0;
        i += 1;
    }
    format!("{:.2} {}", size, units[i])
}

/// 1234567 -> "1,234,567"
pub fn group_digits(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Lexically clean an absolute path (QDir::cleanPath / QFileInfo::absoluteFilePath).
pub fn absolute_clean(p: &str) -> String {
    let path = Path::new(p);
    let abs: PathBuf = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/")).join(path)
    };
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out.to_string_lossy().to_string()
}

/// If the string is a file:// URL, return the local path (QUrl::toLocalFile).
pub fn url_to_local(s: &str) -> Option<String> {
    let rest = s.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let decoded = percent_encoding::percent_decode_str(rest).decode_utf8_lossy().to_string();
    Some(decoded)
}

pub fn file_name(p: &str) -> String {
    Path::new(p).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}

pub fn parent_dir(p: &str) -> String {
    Path::new(p).parent().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}

/// QFileInfo::baseName (up to the first dot)
pub fn base_name(p: &str) -> String {
    let n = file_name(p);
    n.split('.').next().unwrap_or("").to_string()
}

/// "#rgb", "#rrggbb" or "#rrrrggggbbbb" (what QColor::name and qView write)
pub fn parse_color(s: &str) -> Option<(u8, u8, u8)> {
    let hex = s.trim().strip_prefix('#')?;
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let n = match hex.len() {
        3 => 1,
        6 => 2,
        9 => 3,
        12 => 4,
        _ => return None,
    };
    let comp = |i: usize| -> Option<u8> {
        let v = u32::from_str_radix(&hex[i * n..(i + 1) * n], 16).ok()?;
        let max = (1u32 << (4 * n)) - 1;
        Some(((v * 255 + max / 2) / max) as u8)
    };
    Some((comp(0)?, comp(1)?, comp(2)?))
}

/// access(2) with W_OK
pub fn is_writable(path: &str) -> bool {
    let Ok(c) = std::ffi::CString::new(path) else { return false };
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

pub fn perceived_brightness(r: f64, g: f64, b: f64) -> f64 {
    r * 0.299 + g * 0.587 + b * 0.114
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn natural() {
        let mut v = vec!["img10.png", "img2.png", "Img1.png", "img1.png", "a.png"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["a.png", "img1.png", "Img1.png", "img2.png", "img10.png"]);
    }
    #[test]
    fn bytes() {
        assert_eq!(format_bytes(500), "500 bytes");
        assert_eq!(format_bytes(1536), "1.50 KiB");
        assert_eq!(group_digits(1234567), "1,234,567");
    }
    #[test]
    fn colors() {
        assert_eq!(parse_color("#4c566a"), Some((0x4c, 0x56, 0x6a)));
        assert_eq!(parse_color("#fff"), Some((255, 255, 255)));
        assert_eq!(parse_color("red"), None);
    }
}

/// Timing trace, enabled with IMAGE_VIEW_TRACE=1
pub struct Trace(&'static str, std::time::Instant);
impl Trace {
    pub fn new(name: &'static str) -> Trace {
        Trace(name, std::time::Instant::now())
    }
}
impl Drop for Trace {
    fn drop(&mut self) {
        thread_local!(static ON: bool = std::env::var_os("IMAGE_VIEW_TRACE").is_some());
        if ON.with(|o| *o) {
            eprintln!("[trace] {} {:.1}ms", self.0, self.1.elapsed().as_secs_f64() * 1000.0);
        }
    }
}
