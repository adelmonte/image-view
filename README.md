# Image View

A minimal, keyboard-driven image viewer for Linux: a Rust + [egui](https://github.com/emilk/egui)
port of [qView](https://github.com/jurplel/qView) (7.0). No GTK, no Qt.

## At a glance

- Install: `./install.sh` (user) or `sudo PREFIX=/usr/local ./install.sh`
- Run: `image-view [file-or-folder]`
- Config: `~/.config/image-view/image-view.conf` (on first run, an existing
  `~/.config/g-view/g-view.conf` is copied over)

## Building

Needs Rust and `lcms2`. Rendering is OpenGL (via egui's glow backend), on Wayland.

```
cargo build --release
```

## Usage

There is no menu: everything is a keyboard shortcut, set in the `[shortcuts]` group of the config
file (qView key names, `;`-separated, e.g. `zoomin=Ctrl++;Ctrl+=;`). The `options` action
(unbound by default) opens the config file in your text editor; changes apply the next time the
window gets focus.

Mouse: drag to pan, scroll to zoom (see `scrollzoom`), double-click for full screen, middle click
to reset zoom, back/forward buttons for previous/next file. Pinch to zoom on touchpads.

Zoom speed: `scalefactor` is the zoom per mouse wheel notch (percent); `touchpadzoomspeed`
(percent, default 100) scales two-finger scroll zooming on top of that.

| Action | Default |
|---|---|
| Open | Ctrl+O |
| New window | Ctrl+N |
| Reload | F5 |
| File info | I |
| Copy image / paste files | Ctrl+C / Ctrl+V |
| Rename | Ctrl+Return, F2, Ctrl+R |
| Move to Trash / delete permanently / restore | Del / Shift+Del / Ctrl+Z |
| First / previous / next / last / random file | Home / Left / Right / End / R |
| Zoom in / out / reset / original size | Ctrl++ / Ctrl+- / Ctrl+0 / O |
| Rotate right / left, mirror, flip | Up / Down, F, Ctrl+F |
| Full screen | F11 |
| Animations: save frame, pause, next frame, speed −/reset/+ | Ctrl+S, P, N, [ \ ] |
| Slideshow, open containing folder, settings | unbound |
| Quit | Ctrl+Q, Ctrl+W |

## Differences from qView

- No menus, context menu or settings dialog; settings live in the config file.
- No drag-and-drop, Open With, recent files, welcome/about screens or update checker.
- Each window is its own process (New Window, or opening several files at once).
- Images are drawn on the GPU (mipmapped), with a CPU-scaled exact-size copy swapped in after
  zooming for maximum sharpness, like qView's "Image scaling".
- Formats: PNG/APNG, JPEG, GIF, WebP, TIFF, BMP, ICO, PNM, TGA, QOI, HDR, EXR, DDS, farbfeld,
  SVG/SVGZ. HEIC, AVIF and JXL are not supported.
- Photos tagged with a non-sRGB color profile (e.g. iPhone Display P3) are converted to sRGB.
- Wayland: the compositor places windows, so only window size (not position) is restored.

Set `IMAGE_VIEW_TRACE=1` to print decode/scale/upload timings.

## License

GPLv3, same as qView. Original work © 2018-2025 jurplel and qView contributors.
