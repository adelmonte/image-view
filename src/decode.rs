//! Image decoding (the QImageReader part of QVImageCore::readFile). Runs on worker threads.
//!
//! Output is egui's premultiplied sRGB `ColorImage`, ready to upload as a texture.

use eframe::egui::{Color32, ColorImage};
use rayon::prelude::*;
use std::io::{BufReader, Read, Seek};
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ErrorData {
    pub has_error: bool,
    pub error_num: i32,
    pub error_string: String,
}

impl ErrorData {
    pub fn new(num: i32) -> Self {
        let s = match num {
            1 => "File not found",
            2 => "Device error",
            3 => "Unsupported image format",
            4 => "Unable to read image data",
            _ => "Unknown error",
        };
        ErrorData { has_error: true, error_num: num, error_string: s.to_string() }
    }
}

pub type Image = Arc<ColorImage>;

pub fn width(img: &ColorImage) -> i32 {
    img.size[0] as i32
}

pub fn height(img: &ColorImage) -> i32 {
    img.size[1] as i32
}

fn new_image(w: usize, h: usize, pixels: Vec<Color32>) -> Image {
    Arc::new(ColorImage::new([w, h], pixels))
}

/// Rotate by a multiple of 90 degrees (clockwise).
pub fn rotated(img: &Image, degrees: i32) -> Image {
    let deg = ((degrees % 360) + 360) % 360;
    if deg == 0 {
        return img.clone();
    }
    let [w, h] = img.size;
    let src = &img.pixels;
    let (nw, nh) = if deg == 180 { (w, h) } else { (h, w) };
    let mut out = vec![Color32::TRANSPARENT; w * h];
    out.par_chunks_mut(nw).enumerate().for_each(|(dy, row)| {
        for (dx, d) in row.iter_mut().enumerate() {
            let (x, y) = match deg {
                90 => (dy, h - 1 - dx),
                180 => (w - 1 - dx, h - 1 - dy),
                _ => (w - 1 - dy, dx),
            };
            *d = src[y * w + x];
        }
    });
    new_image(nw, nh, out)
}

#[derive(Clone)]
pub struct Frame {
    pub image: Image,
    pub delay_ms: u32,
}

#[derive(Clone)]
pub struct ReadData {
    pub image: Option<Image>,
    pub frames: Option<Arc<Vec<Frame>>>,
    pub absolute_file_path: String,
    pub file_size: u64,
    pub image_size: (i32, i32),
    pub error_data: ErrorData,
}

impl ReadData {
    pub fn cost_kib(&self) -> usize {
        let px = |i: &Image| i.pixels.len() * 4;
        let mut b = self.image.as_ref().map(px).unwrap_or(0);
        if let Some(f) = &self.frames {
            b += f.iter().map(|f| px(&f.image)).sum::<usize>();
        }
        (b / 1024).max(1)
    }
}

// ---- format support ------------------------------------------------------------------------

/// (extension, mime type) for everything we can decode
const FORMATS: &[(&str, &str)] = &[
    ("bmp", "image/bmp"),
    ("dib", "image/bmp"),
    ("dds", "image/vnd.ms-dds"),
    ("exr", "image/x-exr"),
    ("ff", "image/x-farbfeld"),
    ("gif", "image/gif"),
    ("hdr", "image/vnd.radiance"),
    ("ico", "image/vnd.microsoft.icon"),
    ("cur", "image/x-win-bitmap"),
    ("jpeg", "image/jpeg"),
    ("jpg", "image/jpeg"),
    ("jpe", "image/jpeg"),
    ("jfi", "image/jpeg"),
    ("jfif", "image/jpeg"),
    ("pam", "image/x-portable-arbitrarymap"),
    ("pbm", "image/x-portable-bitmap"),
    ("pgm", "image/x-portable-graymap"),
    ("png", "image/png"),
    ("apng", "image/apng"),
    ("pnm", "image/x-portable-anymap"),
    ("ppm", "image/x-portable-pixmap"),
    ("qoi", "image/qoi"),
    ("svg", "image/svg+xml"),
    ("svgz", "image/svg+xml-compressed"),
    ("tga", "image/x-tga"),
    ("tif", "image/tiff"),
    ("tiff", "image/tiff"),
    ("webp", "image/webp"),
];

/// Lowercase extensions with the dot (".png")
pub fn extensions() -> Vec<String> {
    let mut v: Vec<String> = FORMATS.iter().map(|(e, _)| format!(".{}", e)).collect();
    v.sort();
    v.dedup();
    v
}

fn ext_of(path: &str) -> String {
    Path::new(path).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

fn mime_from_magic(head: &[u8]) -> Option<&'static str> {
    use image::ImageFormat as F;
    if looks_like_svg(head) {
        return Some("image/svg+xml");
    }
    Some(match image::guess_format(head).ok()? {
        F::Png => "image/png",
        F::Jpeg => "image/jpeg",
        F::Gif => "image/gif",
        F::WebP => "image/webp",
        F::Pnm => "image/x-portable-anymap",
        F::Tiff => "image/tiff",
        F::Tga => "image/x-tga",
        F::Dds => "image/vnd.ms-dds",
        F::Bmp => "image/bmp",
        F::Ico => "image/vnd.microsoft.icon",
        F::Hdr => "image/vnd.radiance",
        F::OpenExr => "image/x-exr",
        F::Farbfeld => "image/x-farbfeld",
        F::Qoi => "image/qoi",
        F::Avif => "image/avif",
        _ => return None,
    })
}

/// Best-effort MIME type: from the content when `content` is set, else from the extension.
pub fn mime_for_file(path: &str, content: bool) -> String {
    if content {
        let mut head = vec![0u8; 1024];
        if let Ok(n) = std::fs::File::open(path).and_then(|mut f| f.read(&mut head)) {
            head.truncate(n);
            if let Some(m) = mime_from_magic(&head) {
                return m.to_string();
            }
        }
    }
    let ext = ext_of(path);
    FORMATS
        .iter()
        .find(|(e, _)| *e == ext)
        .map(|(_, m)| m.to_string())
        .unwrap_or_else(|| "application/octet-stream".to_string())
}

/// True if the file's content is a format we decode
pub fn content_is_supported(path: &str) -> bool {
    let m = mime_for_file(path, true);
    m != "application/octet-stream" && m != "image/avif" && FORMATS.iter().any(|(_, f)| *f == m)
}

// ---- color management ------------------------------------------------------------------------

/// The embedded profile, if it isn't plain sRGB (e.g. Display P3 from iPhones, Adobe RGB from
/// cameras), i.e. if the colors need converting for a normal screen.
fn needs_conversion(embedded_icc: Option<&[u8]>) -> Option<lcms2::Profile> {
    let p = lcms2::Profile::new_icc(embedded_icc?).ok()?;
    if p.color_space() != lcms2::ColorSpaceSignature::RgbData {
        return None;
    }
    let desc = p.info(lcms2::InfoType::Description, lcms2::Locale::none()).unwrap_or_default();
    if desc.to_ascii_lowercase().contains("srgb") {
        return None;
    }
    Some(p)
}

/// Convert RGBA8 pixels in place from the embedded profile to sRGB. Untagged and
/// sRGB-tagged images are left alone.
fn convert_color_space(rgba: &mut [u8], embedded_icc: Option<&[u8]>) {
    let Some(src) = needs_conversion(embedded_icc) else { return };
    let dst = lcms2::Profile::new_srgb();
    if let Ok(t) = lcms2::Transform::<[u8; 4], [u8; 4]>::new(&src, lcms2::PixelFormat::RGBA_8, &dst, lcms2::PixelFormat::RGBA_8, lcms2::Intent::Perceptual) {
        let px: &mut [[u8; 4]] = unsafe { std::slice::from_raw_parts_mut(rgba.as_mut_ptr() as *mut [u8; 4], rgba.len() / 4) };
        t.transform_in_place(px);
    }
}

const CHUNK: usize = 64 * 1024;

/// Straight-alpha RGBA8 -> premultiplied Color32, in parallel
fn from_rgba(w: u32, h: u32, rgba: &[u8]) -> Image {
    let mut out = vec![Color32::TRANSPARENT; (w * h) as usize];
    out.par_chunks_mut(CHUNK).zip(rgba.par_chunks(CHUNK * 4)).for_each(|(o, i)| {
        for (d, p) in o.iter_mut().zip(i.chunks_exact(4)) {
            *d = Color32::from_rgba_unmultiplied(p[0], p[1], p[2], p[3]);
        }
    });
    new_image(w as usize, h as usize, out)
}

fn from_rgb(w: u32, h: u32, rgb: &[u8]) -> Image {
    let mut out = vec![Color32::TRANSPARENT; (w * h) as usize];
    out.par_chunks_mut(CHUNK).zip(rgb.par_chunks(CHUNK * 3)).for_each(|(o, i)| {
        for (d, p) in o.iter_mut().zip(i.chunks_exact(3)) {
            *d = Color32::from_rgb(p[0], p[1], p[2]);
        }
    });
    new_image(w as usize, h as usize, out)
}

/// Premultiplied Color32 -> straight RGBA8 (for saving / clipboard)
pub fn to_rgba(img: &ColorImage) -> Vec<u8> {
    let mut out = vec![0u8; img.pixels.len() * 4];
    out.par_chunks_mut(CHUNK * 4).zip(img.pixels.par_chunks(CHUNK)).for_each(|(o, i)| {
        for (d, p) in o.chunks_exact_mut(4).zip(i) {
            d.copy_from_slice(&p.to_srgba_unmultiplied());
        }
    });
    out
}

fn as_bytes(px: &[Color32]) -> &[u8] {
    // Color32 is a #[repr(C)] [u8; 4]
    unsafe { std::slice::from_raw_parts(px.as_ptr() as *const u8, px.len() * 4) }
}

// ---- decoding -------------------------------------------------------------------------------

fn looks_like_svg(head: &[u8]) -> bool {
    if head.starts_with(&[0x1f, 0x8b]) {
        return false; // gzip; only treated as svgz by extension
    }
    let s = String::from_utf8_lossy(&head[..head.len().min(1024)]).to_ascii_lowercase();
    s.contains("<svg")
}

/// Port of QVImageCore::readFile.
pub fn read_file(path: &str, largest_dimension: i32) -> ReadData {
    let _t = crate::util::Trace::new("decode");
    let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let mut rd = ReadData {
        image: None,
        frames: None,
        absolute_file_path: path.to_string(),
        file_size,
        image_size: (-1, -1),
        error_data: ErrorData::default(),
    };

    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            rd.error_data = ErrorData::new(if e.kind() == std::io::ErrorKind::NotFound { 1 } else { 2 });
            return rd;
        }
    };
    let mut head = vec![0u8; 1024];
    let n = file.read(&mut head).unwrap_or(0);
    head.truncate(n);
    let _ = file.rewind();

    let ext = ext_of(path);
    let result = if ext == "svg" || ext == "svgz" || looks_like_svg(&head) {
        read_svg(path, largest_dimension).map(|(img, size)| (img, None, size))
    } else {
        read_raster(file, path)
    };
    match result {
        Ok((img, frames, size)) => {
            rd.image = Some(img);
            rd.frames = frames.map(Arc::new);
            rd.image_size = size;
        }
        Err(e) => rd.error_data = e,
    }
    rd
}

type RasterResult = Result<(Image, Option<Vec<Frame>>, (i32, i32)), ErrorData>;

fn map_err(e: image::ImageError) -> ErrorData {
    match e {
        image::ImageError::Unsupported(_) => ErrorData::new(3),
        image::ImageError::IoError(io) if io.kind() == std::io::ErrorKind::NotFound => ErrorData::new(1),
        image::ImageError::IoError(_) => ErrorData::new(4),
        image::ImageError::Decoding(_) | image::ImageError::Limits(_) => ErrorData::new(4),
        _ => ErrorData::new(0),
    }
}

fn read_raster(file: std::fs::File, path: &str) -> RasterResult {
    use image::{AnimationDecoder, ImageDecoder, ImageFormat};
    let reader = image::ImageReader::new(BufReader::new(file)).with_guessed_format().map_err(|_| ErrorData::new(4))?;
    let format = reader.format().or_else(|| ImageFormat::from_path(path).ok()).ok_or_else(|| ErrorData::new(3))?;

    let frames_from = |frames: image::Frames| -> Result<Vec<Frame>, ErrorData> {
        let mut out = vec![];
        let mut total: usize = 0;
        for f in frames {
            let f = f.map_err(map_err)?;
            let (n, d) = f.delay().numer_denom_ms();
            let delay = if d == 0 { 0 } else { n / d };
            let buf = f.into_buffer();
            let (w, h) = buf.dimensions();
            total += (w as usize) * (h as usize) * 4;
            if total > 2 * 1024 * 1024 * 1024 {
                break; // keep memory sane for gigantic animations
            }
            out.push(Frame { image: from_rgba(w, h, buf.as_raw()), delay_ms: delay });
        }
        Ok(out)
    };

    let open = || std::fs::File::open(path).map(BufReader::new).map_err(|_| ErrorData::new(2));
    match format {
        ImageFormat::Gif => {
            let dec = image::codecs::gif::GifDecoder::new(open()?).map_err(map_err)?;
            let (w, h) = dec.dimensions();
            let frames = frames_from(dec.into_frames())?;
            return finish_animation(frames, (w as i32, h as i32));
        }
        ImageFormat::WebP => {
            let dec = image::codecs::webp::WebPDecoder::new(open()?).map_err(map_err)?;
            if dec.has_animation() {
                let (w, h) = dec.dimensions();
                let frames = frames_from(dec.into_frames())?;
                return finish_animation(frames, (w as i32, h as i32));
            }
        }
        ImageFormat::Png => {
            let dec = image::codecs::png::PngDecoder::new(open()?).map_err(map_err)?;
            if dec.is_apng().unwrap_or(false) {
                let (w, h) = dec.dimensions();
                let frames = frames_from(dec.apng().map_err(map_err)?.into_frames())?;
                return finish_animation(frames, (w as i32, h as i32));
            }
        }
        _ => {}
    }

    // Static image with orientation + ICC
    let mut dec = reader.into_decoder().map_err(map_err)?;
    let (w, h) = dec.dimensions();
    let icc = dec.icc_profile().ok().flatten();
    let orientation = dec.orientation().ok();
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(8 * 1024 * 1024 * 1024);
    dec.set_limits(limits).map_err(map_err)?;
    let mut img = image::DynamicImage::from_decoder(dec).map_err(map_err)?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    // Fast path: opaque 8-bit RGB that needs no color conversion (most photos)
    let needs_color = needs_conversion(icc.as_deref()).is_some();
    if let (image::DynamicImage::ImageRgb8(rgb), false) = (&img, needs_color) {
        let (iw, ih) = rgb.dimensions();
        return Ok((from_rgb(iw, ih, rgb.as_raw()), None, (w as i32, h as i32)));
    }
    let rgba = img.into_rgba8();
    let (iw, ih) = rgba.dimensions();
    let mut raw = rgba.into_raw();
    convert_color_space(&mut raw, icc.as_deref());
    Ok((from_rgba(iw, ih, &raw), None, (w as i32, h as i32)))
}

fn finish_animation(frames: Vec<Frame>, size: (i32, i32)) -> RasterResult {
    if frames.is_empty() {
        return Err(ErrorData::new(4));
    }
    let first = frames[0].image.clone();
    if frames.len() == 1 {
        return Ok((first, None, size));
    }
    Ok((first, Some(frames), size))
}

fn read_svg(path: &str, largest: i32) -> Result<(Image, (i32, i32)), ErrorData> {
    let data = std::fs::read(path).map_err(|_| ErrorData::new(1))?;
    let mut opt = resvg::usvg::Options { resources_dir: Path::new(path).parent().map(|p| p.to_path_buf()), ..Default::default() };
    opt.fontdb_mut().load_system_fonts();
    let tree = resvg::usvg::Tree::from_data(&data, &opt).map_err(|_| ErrorData::new(4))?;
    let size = tree.size();
    let (sw, sh) = (size.width(), size.height());
    // QIcon::pixmap(largestDimension): fit inside an L x L square, keeping aspect ratio
    let l = largest.max(1) as f32;
    let scale = (l / sw).min(l / sh);
    let w = (sw * scale).round().max(1.0) as u32;
    let h = (sh * scale).round().max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).ok_or_else(|| ErrorData::new(4))?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(w as f32 / sw, h as f32 / sh), &mut pixmap.as_mut());
    // tiny-skia is premultiplied RGBA, same as Color32
    let px: Vec<Color32> = pixmap.data().chunks_exact(4).map(|p| Color32::from_rgba_premultiplied(p[0], p[1], p[2], p[3])).collect();
    Ok((new_image(w as usize, h as usize, px), (sw.round() as i32, sh.round() as i32)))
}

/// Smooth scale, like QPixmap::scaled(..., Qt::SmoothTransformation): area averaging when
/// shrinking, bilinear when enlarging. SIMD + multithreaded; data is already premultiplied.
pub fn scale_smooth(src: &ColorImage, w: i32, h: i32) -> Option<ColorImage> {
    use fast_image_resize as fr;
    let (w, h) = (w.max(1) as u32, h.max(1) as u32);
    let (sw, sh) = (src.size[0] as u32, src.size[1] as u32);
    let src_img = fr::images::ImageRef::new(sw, sh, as_bytes(&src.pixels), fr::PixelType::U8x4).ok()?;
    let mut dst = fr::images::Image::new(w, h, fr::PixelType::U8x4);
    let filter = if w < sw || h < sh { fr::FilterType::Box } else { fr::FilterType::Bilinear };
    let opts = fr::ResizeOptions::new().resize_alg(fr::ResizeAlg::Convolution(filter)).use_alpha(false);
    fr::Resizer::new().resize(&src_img, &mut dst, &opts).ok()?;
    let bytes = dst.into_vec();
    let mut px = vec![Color32::TRANSPARENT; (w * h) as usize];
    px.par_chunks_mut(CHUNK).zip(bytes.par_chunks(CHUNK * 4)).for_each(|(o, i)| {
        for (d, p) in o.iter_mut().zip(i.chunks_exact(4)) {
            *d = Color32::from_rgba_premultiplied(p[0], p[1], p[2], p[3]);
        }
    });
    Some(ColorImage::new([w as usize, h as usize], px))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p3_icc() -> Vec<u8> {
        let white = lcms2::CIExyY { x: 0.3127, y: 0.3290, Y: 1.0 };
        let prim = lcms2::CIExyYTRIPLE {
            Red: lcms2::CIExyY { x: 0.680, y: 0.320, Y: 1.0 },
            Green: lcms2::CIExyY { x: 0.265, y: 0.690, Y: 1.0 },
            Blue: lcms2::CIExyY { x: 0.150, y: 0.060, Y: 1.0 },
        };
        let curve = lcms2::ToneCurve::new_parametric(4, &[2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045]).unwrap();
        let p = lcms2::Profile::new_rgb(&white, &prim, &[&curve, &curve, &curve]).unwrap();
        p.icc().unwrap()
    }

    #[test]
    fn p3_is_converted_srgb_is_not() {
        let mut px = vec![200u8, 100, 50, 255];
        convert_color_space(&mut px, Some(&p3_icc()));
        assert!(px[0] > 200, "P3 orange is more saturated than sRGB: {:?}", px);

        let srgb = lcms2::Profile::new_srgb().icc().unwrap();
        let mut px = vec![200u8, 100, 50, 255];
        convert_color_space(&mut px, Some(&srgb));
        assert_eq!(px, vec![200, 100, 50, 255]);
    }

    #[test]
    fn rotation() {
        // 2x1: [a b] -> 90cw -> 1x2: [a; b]
        let a = Color32::from_rgb(1, 0, 0);
        let b = Color32::from_rgb(2, 0, 0);
        let img = new_image(2, 1, vec![a, b]);
        let r = rotated(&img, 90);
        assert_eq!(r.size, [1, 2]);
        assert_eq!(r.pixels, vec![a, b]);
        let r = rotated(&img, -90);
        assert_eq!(r.pixels, vec![b, a]);
        let r = rotated(&img, 180);
        assert_eq!(r.pixels, vec![b, a]);
    }
}
