//! image-view: a minimal image viewer (a Rust + egui port of qView, https://github.com/jurplel/qView).

mod app;
mod core;
mod decode;
mod i18n;
mod settings;
mod shortcuts;
mod trash;
mod util;
mod view;

use eframe::egui;
use settings::Setting;

fn print_help() {
    println!("Usage: image-view [options] file\n");
    println!("Options:");
    println!("  -h, --help     Displays help on commandline options.");
    println!("  -v, --version  Displays version information.\n");
    println!("Arguments:");
    println!("  file           The file or folder to open.");
}

fn icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(include_bytes!("../dist/hicolor/256x256/apps/image-view.png")).ok()?.into_rgba8();
    Some(egui::IconData { width: img.width(), height: img.height(), rgba: img.into_raw() })
}

fn main() {
    let mut file: Option<String> = None;
    let mut only_files = false;
    for a in std::env::args().skip(1) {
        if !only_files {
            match a.as_str() {
                "-h" | "--help" | "-?" => {
                    print_help();
                    return;
                }
                "-v" | "--version" => {
                    println!("image-view {}", env!("CARGO_PKG_VERSION"));
                    return;
                }
                "--" => {
                    only_files = true;
                    continue;
                }
                _ if a.starts_with('-') && a.len() > 1 => continue,
                _ => {}
            }
        }
        if file.is_none() {
            file = Some(a);
        }
    }

    settings::migrate_if_needed();
    i18n::load(&settings::get().string(Setting::Language));

    let geo: Vec<i32> = settings::Store::get("General", "geometry")
        .map(|s| s.split(',').filter_map(|t| t.trim().parse().ok()).collect())
        .unwrap_or_default();
    let (size, maximized) = match geo.as_slice() {
        [_, _, w, h, rest @ ..] if *w > 0 && *h > 0 => (egui::vec2(*w as f32, *h as f32), rest.first() == Some(&1)),
        _ => (egui::vec2(640.0, 480.0), false),
    };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("image-view")
        .with_app_id("image-view")
        .with_inner_size(size)
        .with_min_inner_size(egui::vec2(100.0, 100.0))
        .with_maximized(maximized);
    if let Some(i) = icon() {
        viewport = viewport.with_icon(i);
    }
    let options = eframe::NativeOptions { viewport, clipboard_shortcuts: false, persist_window: false, ..Default::default() };
    if let Err(e) = eframe::run_native("image-view", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, file))))) {
        eprintln!("image-view: {}", e);
        std::process::exit(1);
    }
}
