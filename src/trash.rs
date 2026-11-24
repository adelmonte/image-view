//! Freedesktop.org trash (what QFile::moveToTrash does on Linux).

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

fn home_trash() -> PathBuf {
    crate::settings::user_data_dir().join("Trash")
}

fn uid() -> u32 {
    unsafe { libc::getuid() }
}

fn mount_top(path: &Path) -> Option<PathBuf> {
    let dev = fs::metadata(path).ok()?.dev();
    let mut cur = path.parent()?.to_path_buf();
    loop {
        let Some(parent) = cur.parent() else { return Some(cur) };
        match fs::metadata(parent) {
            Ok(m) if m.dev() == dev => cur = parent.to_path_buf(),
            _ => return Some(cur),
        }
    }
}

fn ensure_dirs(trash: &Path) -> bool {
    fs::create_dir_all(trash.join("files")).is_ok() && fs::create_dir_all(trash.join("info")).is_ok()
}

fn find_trash(path: &Path) -> Option<(PathBuf, bool)> {
    let home = home_trash();
    let _ = fs::create_dir_all(&home);
    let file_dev = fs::symlink_metadata(path).ok()?.dev();
    if let Ok(m) = fs::metadata(&home) {
        if m.dev() == file_dev && ensure_dirs(&home) {
            return Some((home, false));
        }
    }
    let top = mount_top(path)?;
    let shared = top.join(".Trash");
    if let Ok(m) = fs::symlink_metadata(&shared) {
        use std::os::unix::fs::PermissionsExt;
        if m.is_dir() && !m.file_type().is_symlink() && m.permissions().mode() & 0o1000 != 0 {
            let t = shared.join(uid().to_string());
            if ensure_dirs(&t) {
                return Some((t, true));
            }
        }
    }
    let t = top.join(format!(".Trash-{}", uid()));
    if ensure_dirs(&t) {
        return Some((t, true));
    }
    None
}

fn encode_path(p: &str) -> String {
    const SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
        .remove(b'/')
        .remove(b'-')
        .remove(b'_')
        .remove(b'.')
        .remove(b'~');
    percent_encoding::utf8_percent_encode(p, SET).to_string()
}

/// Move a file to the trash. Returns the path inside the trash on success.
pub fn move_to_trash(path: &str) -> Option<String> {
    let p = Path::new(path);
    let (trash, topdir) = find_trash(p)?;
    let name = p.file_name()?.to_string_lossy().to_string();
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()),
        _ => (name.clone(), String::new()),
    };
    let rel = if topdir {
        let top = trash.parent().map(|t| if t.ends_with(".Trash") { t.parent().unwrap_or(t) } else { t }).unwrap_or(&trash);
        p.strip_prefix(top).map(|r| r.to_string_lossy().to_string()).unwrap_or_else(|_| path.to_string())
    } else {
        path.to_string()
    };
    let date = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();
    for i in 0..10000 {
        let candidate = if i == 0 { name.clone() } else { format!("{} ({}){}", stem, i, ext) };
        let info = trash.join("info").join(format!("{}.trashinfo", candidate));
        let f = fs::OpenOptions::new().write(true).create_new(true).open(&info);
        let Ok(mut f) = f else { continue };
        use std::io::Write;
        let _ = write!(f, "[Trash Info]\nPath={}\nDeletionDate={}\n", encode_path(&rel), date);
        drop(f);
        let dest = trash.join("files").join(&candidate);
        if fs::rename(p, &dest).is_ok() {
            return Some(dest.to_string_lossy().to_string());
        }
        let _ = fs::remove_file(&info);
        return None;
    }
    None
}

/// Undo: move back out of the trash and drop the .trashinfo
pub fn restore(path_in_trash: &str, previous_path: &str) -> bool {
    let ok = fs::rename(path_in_trash, previous_path).is_ok();
    if ok {
        let p = Path::new(path_in_trash);
        if let (Some(files), Some(name)) = (p.parent(), p.file_name()) {
            if let Some(trash) = files.parent() {
                let _ = fs::remove_file(trash.join("info").join(format!("{}.trashinfo", name.to_string_lossy())));
            }
        }
    }
    ok
}
