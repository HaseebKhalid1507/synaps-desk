//! Frame decode → anchor registration → crop to the shared canvas rect → scale to display
//! size → BGRA render image + hit mask. Only the scaled pixels are kept.

use crate::pack::{FrameSource, Pack, Rect};
use anyhow::{Context as _, Result, bail};
use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer, images::Image};
use gpui_kit::RenderImage;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Alpha at or above this counts as "body" for click-through and hit tests.
pub const HIT_ALPHA: u8 = 96;

pub struct Mask {
    pub w: u32,
    pub h: u32,
    bits: Vec<u64>,
}

impl Mask {
    fn new(w: u32, h: u32) -> Self {
        Mask { w, h, bits: vec![0; (w as usize * h as usize).div_ceil(64)] }
    }
    fn set(&mut self, x: u32, y: u32) {
        let i = y as usize * self.w as usize + x as usize;
        self.bits[i / 64] |= 1 << (i % 64);
    }
    pub fn get(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x as u32 >= self.w || y as u32 >= self.h {
            return false;
        }
        let i = y as usize * self.w as usize + x as usize;
        self.bits[i / 64] & (1 << (i % 64)) != 0
    }
    /// Run-length rows merged into bands of identical rows: the X input/bounding shape.
    pub fn rects(&self) -> Vec<(i16, i16, u16, u16)> {
        let mut out: Vec<(i16, i16, u16, u16)> = Vec::new();
        let mut prev: Vec<(u32, u32)> = Vec::new();
        let mut open: Vec<usize> = Vec::new(); // indices into `out` for prev row's runs
        for y in 0..self.h {
            let mut runs = Vec::new();
            let mut x = 0;
            while x < self.w {
                if self.get(x as i32, y as i32) {
                    let s = x;
                    while x < self.w && self.get(x as i32, y as i32) {
                        x += 1;
                    }
                    runs.push((s, x - s));
                } else {
                    x += 1;
                }
            }
            if runs == prev && !runs.is_empty() {
                for &i in &open {
                    out[i].3 += 1;
                }
            } else {
                open.clear();
                for &(s, w) in &runs {
                    open.push(out.len());
                    out.push((s as i16, y as i16, w as u16, 1));
                }
                prev = runs;
            }
        }
        out
    }
}

pub struct Frame {
    pub id: String,
    pub image: Arc<RenderImage>,
    pub mask: Mask,
    pub rects: Arc<Vec<(i16, i16, u16, u16)>>,
    /// Device-pixel y (inside the avatar rect) above which a click is a head click.
    pub head_line: i32,
    /// Device-px top of the frame's visible bounds inside the avatar rect.
    pub top: i32,
    /// Device-px x of the head: mask centroid over the top 30 % of the visible bounds.
    pub head_x: i32,
    /// Points well inside the upper part of the figure (hair), for glints.
    pub glints: Vec<(i16, i16)>,
    /// Topmost opaque row per column (i16::MAX where the column is empty).
    pub profile: Arc<Vec<i16>>,
    /// Device-px rows of the face (brows to mouth): glitches keep it readable.
    pub face: (i32, i32),
}

/// Topmost opaque row per column.
fn top_profile(mask: &Mask) -> Vec<i16> {
    (0..mask.w as i32)
        .map(|x| (0..mask.h as i32).find(|&y| mask.get(x, y)).map_or(i16::MAX, |y| y as i16))
        .collect()
}

/// Rows of the face. Capri-style eyes are saturated blue-teal disks; their rows give the
/// band (brows a little above, the mouth below). Without them: a fixed slice under the top.
fn face_band(px: &[u8], w: u32, top: i32, height: i32) -> (i32, i32) {
    let mut rows = Vec::new();
    let lim = (top + (height as f32 * 0.65) as i32).min((px.len() / 4 / w as usize) as i32);
    for y in top.max(0)..lim {
        for x in 0..w as i32 {
            let i = (y as usize * w as usize + x as usize) * 4;
            let (r, g, b, a) = (px[i], px[i + 1], px[i + 2], px[i + 3]);
            if a > 200 && b > 190 && r < 100 && g > 50 && g < 200 {
                rows.push(y);
            }
        }
    }
    if rows.len() >= 6 {
        rows.sort_unstable();
        let (y0, y1) = (rows[rows.len() / 10], rows[rows.len() * 9 / 10]);
        let eye = (y1 - y0).max(6);
        return (y0 - eye, y1 + eye * 2);
    }
    (top + (height as f32 * 0.17) as i32, top + (height as f32 * 0.37) as i32)
}

/// A 1 px rim of `rgb` around dark opaque pixels from row `from` down, drawn into
/// transparent neighbours (so behind the art, never over it). RGBA straight alpha.
fn add_rim(px: &mut [u8], w: u32, h: u32, from: i32, rgb: u32) {
    let (w, h) = (w as i32, h as i32);
    let dark = |px: &[u8], x: i32, y: i32| {
        if x < 0 || y < 0 || x >= w || y >= h {
            return false;
        }
        let i = (y * w + x) as usize * 4;
        px[i + 3] >= 160 && (px[i] as u32 * 3 + px[i + 1] as u32 * 6 + px[i + 2] as u32) < 10 * 70
    };
    let mut set = Vec::new();
    for y in from.max(0)..h {
        for x in 0..w {
            let i = (y * w + x) as usize * 4;
            if px[i + 3] >= HIT_ALPHA {
                continue;
            }
            if dark(px, x - 1, y) || dark(px, x + 1, y) || dark(px, x, y - 1) || dark(px, x, y + 1) {
                set.push(i);
            }
        }
    }
    for i in set {
        px[i] = (rgb >> 16) as u8;
        px[i + 1] = (rgb >> 8) as u8;
        px[i + 2] = rgb as u8;
        px[i + 3] = 255;
    }
}

/// Head x and glint candidates from the hit mask (pack-agnostic: no colour rules).
fn head_and_glints(mask: &Mask, top: i32, height: i32) -> (i32, Vec<(i16, i16)>) {
    let band = (height as f32 * 0.3) as i32;
    let (mut sum, mut n) = (0i64, 0i64);
    for y in top.max(0)..(top + band).min(mask.h as i32) {
        for x in 0..mask.w as i32 {
            if mask.get(x, y) {
                sum += x as i64;
                n += 1;
            }
        }
    }
    let head_x = if n > 0 { (sum / n) as i32 } else { mask.w as i32 / 2 };
    // Candidates: pixels in the top 40 % whose 9x9 neighbourhood is all body, on a grid.
    let r = 4;
    let mut cands = Vec::new();
    let y1 = (top + (height as f32 * 0.4) as i32).min(mask.h as i32 - r);
    for y in ((top + r).max(r)..y1).step_by(3) {
        for x in (r..mask.w as i32 - r).step_by(3) {
            if [(-r, -r), (r, -r), (-r, r), (r, r), (0, 0), (0, -r), (0, r), (-r, 0), (r, 0)].iter().all(|&(dx, dy)| mask.get(x + dx, y + dy)) {
                cands.push((x as i16, y as i16));
            }
        }
    }
    // A fixed spread of 48 of them (deterministic LCG).
    let mut seed = 0x2545_f491u32 ^ (cands.len() as u32);
    let mut glints = Vec::new();
    for _ in 0..cands.len().min(48) {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        glints.push(cands[(seed >> 8) as usize % cands.len()]);
    }
    (head_x, glints)
}

/// Display geometry shared by all frames.
#[derive(Clone, Copy, Debug)]
pub struct Geometry {
    /// Canvas rect every frame is cropped to.
    pub crop: Rect,
    /// Canvas px → device px.
    pub scale: f32,
    pub width: u32,
    pub height: u32,
}

impl Geometry {
    /// `body_px`: device-pixel height of the default sprite's visible bounds.
    pub fn new(pack: &Pack, body_px: f32) -> Geometry {
        let crop = pack.crop_rect();
        let reference = pack.frame(&pack.catalog.default_sprite).map(|i| pack.frames[i].bounds.height).unwrap_or(1000);
        let scale = body_px / reference as f32;
        Geometry {
            crop,
            scale,
            width: (crop.width as f32 * scale).round() as u32,
            height: (crop.height as f32 * scale).round() as u32,
        }
    }

    /// Device px of a canonical-canvas point inside the avatar rect.
    pub fn to_device(&self, x: i32, y: i32) -> (f32, f32) {
        ((x - self.crop.x) as f32 * self.scale, (y - self.crop.y) as f32 * self.scale)
    }
}

pub struct Decoded {
    pub frames: Vec<Frame>,
    pub decode_ms: u128,
}

fn sha_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn decode_one(pack: &Pack, f: &FrameSource, geo: &Geometry, verify: bool, resizer: &mut Resizer) -> Result<Frame> {
    let bytes = std::fs::read(&f.path).with_context(|| format!("frame `{}`: cannot read {}", f.id, f.path.display()))?;
    if verify
        && let Some(want) = &f.sha256
    {
        let got = sha_hex(&bytes);
        if !got.eq_ignore_ascii_case(want) {
            bail!("frame `{}`: sha256 mismatch for {} (catalog {want}, file {got})", f.id, f.path.display());
        }
    }
    let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        .with_context(|| format!("frame `{}`: {} is not a valid PNG", f.id, f.path.display()))?
        .into_rgba8();
    let (iw, ih) = img.dimensions();
    let canvas = &pack.catalog.canvas;
    if iw != canvas.width || ih != canvas.height {
        bail!("frame `{}`: {} is {iw}x{ih}, catalog canvas is {}x{}", f.id, f.path.display(), canvas.width, canvas.height);
    }

    // Copy the registered frame into the shared crop rect (transparent padding).
    let (dx, dy) = pack.offset(f);
    let crop = geo.crop;
    let (cw, ch) = (crop.width as usize, crop.height as usize);
    let mut buf = vec![0u8; cw * ch * 4];
    let src = img.as_raw();
    for row in 0..ch {
        let sy = crop.y + row as i32 - dy;
        if sy < 0 || sy >= ih as i32 {
            continue;
        }
        let sx0 = crop.x - dx;
        let x_from = (-sx0).max(0) as usize;
        let x_to = (iw as i32 - sx0).min(cw as i32).max(0) as usize;
        if x_from >= x_to {
            continue;
        }
        let s = (sy as usize * iw as usize + (sx0 + x_from as i32) as usize) * 4;
        let d = (row * cw + x_from) * 4;
        let n = (x_to - x_from) * 4;
        buf[d..d + n].copy_from_slice(&src[s..s + n]);
    }
    drop(img);

    let src = Image::from_vec_u8(cw as u32, ch as u32, buf, PixelType::U8x4)?;
    let mut dst = Image::new(geo.width, geo.height, PixelType::U8x4);
    // Lanczos3 on premultiplied alpha (fast_image_resize premultiplies for U8x4).
    resizer.resize(&src, &mut dst, &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3)))?;
    let mut px = dst.into_vec();
    if f.anchor.name == "seat"
        && let Some(rim) = pack.behaviour.perch.as_ref().and_then(|p| u32::from_str_radix(p.rim.trim_start_matches('#'), 16).ok())
    {
        // The limbs over the window: from just above the seat line down.
        let (_, seat) = geo.to_device(0, f.anchor.y + dy);
        add_rim(&mut px, geo.width, geo.height, seat.round() as i32 - 8, rim);
    }
    let vb = f.bounds.offset(dx, dy);
    let (_, top) = geo.to_device(0, vb.y);
    let face = face_band(&px, geo.width, top.round() as i32, (vb.height as f32 * geo.scale) as i32);

    let mut mask = Mask::new(geo.width, geo.height);
    for (i, p) in px.chunks_exact_mut(4).enumerate() {
        if p[3] >= HIT_ALPHA {
            mask.set(i as u32 % geo.width, i as u32 / geo.width);
        }
        p.swap(0, 2); // RGBA → BGRA (GPUI's sprite atlas format), straight alpha.
    }
    let rects = Arc::new(mask.rects());
    let head_fraction = pack.behaviour.click.head_fraction;
    let head_line = (top + vb.height as f32 * geo.scale * head_fraction).round() as i32;
    let rgba = image::RgbaImage::from_raw(geo.width, geo.height, px).expect("buffer size");
    let (head_x, glints) = head_and_glints(&mask, top.round() as i32, (vb.height as f32 * geo.scale) as i32);
    let profile = Arc::new(top_profile(&mask));
    Ok(Frame {
        profile,
        face,
        id: f.id.clone(),
        image: Arc::new(RenderImage::new(vec![image::Frame::new(rgba)])),
        mask,
        rects,
        head_line,
        top: top.round() as i32,
        head_x,
        glints,
    })
}

/// Decode the frames at `which` (indices into `pack.frames`) on up to `threads` threads.
pub fn decode(pack: &Pack, geo: &Geometry, which: &[usize], verify: bool, threads: usize) -> Result<Decoded> {
    let t0 = std::time::Instant::now();
    let next = AtomicUsize::new(0);
    let slots: std::sync::Mutex<Vec<Option<Result<Frame>>>> =
        std::sync::Mutex::new((0..which.len()).map(|_| None).collect());
    std::thread::scope(|s| {
        for _ in 0..threads.max(1).min(which.len().max(1)) {
            s.spawn(|| {
                let mut resizer = Resizer::new();
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= which.len() {
                        break;
                    }
                    let r = decode_one(pack, &pack.frames[which[i]], geo, verify, &mut resizer);
                    slots.lock().unwrap()[i] = Some(r);
                }
            });
        }
    });
    let mut frames = Vec::with_capacity(which.len());
    for r in slots.into_inner().unwrap() {
        frames.push(r.expect("every slot decoded")?);
    }
    Ok(Decoded { frames, decode_ms: t0.elapsed().as_millis() })
}

/// One pass over the docking poses' hashes (they are not decoded in phase 1).
pub fn verify_poses(pack: &Pack) -> Result<()> {
    for p in &pack.poses {
        if let Some(want) = &p.sha256 {
            let bytes = std::fs::read(&p.path).with_context(|| format!("pose `{}`: cannot read", p.id))?;
            let got = sha_hex(&bytes);
            if !got.eq_ignore_ascii_case(want) {
                bail!("pose `{}`: sha256 mismatch for {}", p.id, p.path.display());
            }
        }
    }
    Ok(())
}

/// A music note as two 1-bit masks (dark outline, colour fill) per rotation: shown as
/// two shaped solid-colour X windows, like the hearts, so moving notes never expose stale
/// pixels of the avatar window.
pub struct NoteMasks {
    pub size: u32,
    pub fill_rgb: u32,
    pub outline_rgb: u32,
    /// Per `NOTE_ANGLES` entry: (outline, fill).
    pub variants: Vec<(Mask, Mask)>,
}

pub const NOTE_ANGLES: [f32; 7] = [-18.0, -12.0, -6.0, 0.0, 6.0, 12.0, 18.0];

/// Rotate a square RGBA image about its centre (bilinear, premultiplied while sampling).
fn rotate_rgba(src: &[u8], n: u32, deg: f32) -> Vec<u8> {
    let (s, c) = deg.to_radians().sin_cos();
    let n_i = n as i32;
    let mid = (n as f32 - 1.0) / 2.0;
    let mut out = vec![0u8; src.len()];
    let at = |x: i32, y: i32| -> [f32; 4] {
        if x < 0 || y < 0 || x >= n_i || y >= n_i {
            return [0.0; 4];
        }
        let i = (y as usize * n as usize + x as usize) * 4;
        let a = src[i + 3] as f32 / 255.0;
        [src[i] as f32 * a, src[i + 1] as f32 * a, src[i + 2] as f32 * a, src[i + 3] as f32]
    };
    for y in 0..n_i {
        for x in 0..n_i {
            let (dx, dy) = (x as f32 - mid, y as f32 - mid);
            let sx = c * dx + s * dy + mid;
            let sy = -s * dx + c * dy + mid;
            let (x0, y0) = (sx.floor() as i32, sy.floor() as i32);
            let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
            let (p00, p10, p01, p11) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
            let mut v = [0.0f32; 4];
            for k in 0..4 {
                v[k] = (p00[k] * (1.0 - fx) + p10[k] * fx) * (1.0 - fy) + (p01[k] * (1.0 - fx) + p11[k] * fx) * fy;
            }
            let i = (y as usize * n as usize + x as usize) * 4;
            if v[3] > 0.5 {
                let a = v[3] / 255.0;
                out[i] = (v[0] / a).round().min(255.0) as u8;
                out[i + 1] = (v[1] / a).round().min(255.0) as u8;
                out[i + 2] = (v[2] / a).round().min(255.0) as u8;
                out[i + 3] = v[3].round().min(255.0) as u8;
            }
        }
    }
    out
}

/// Decode note images to `size` device px squares, rotated at every `NOTE_ANGLES` step.
pub fn decode_notes(pack: &Pack, files: &[String], size: u32) -> Result<Vec<NoteMasks>> {
    let mut resizer = Resizer::new();
    let mut out = Vec::new();
    let is_dark = |p: &[u8]| (p[0] as u32 * 3 + p[1] as u32 * 6 + p[2] as u32) < 10 * 60;
    for rel in files {
        let path = pack.dir.join(rel);
        let bytes = std::fs::read(&path).with_context(|| format!("note {}: cannot read", path.display()))?;
        let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
            .with_context(|| format!("note {}: not a PNG", path.display()))?
            .into_rgba8();
        let (w, h) = img.dimensions();
        if w != h {
            bail!("note {}: {w}x{h} is not square", path.display());
        }
        // Fill colour: the mean of the bright opaque pixels.
        let (mut sum, mut n) = ([0u64; 3], 0u64);
        for p in img.as_raw().chunks_exact(4).filter(|p| p[3] > 200 && !is_dark(p)) {
            for k in 0..3 {
                sum[k] += p[k] as u64;
            }
            n += 1;
        }
        let n = n.max(1);
        let fill_rgb = ((sum[0] / n) << 16 | (sum[1] / n) << 8 | (sum[2] / n)) as u32;
        let mut variants = Vec::new();
        for &deg in &NOTE_ANGLES {
            let rot = if deg == 0.0 { img.as_raw().clone() } else { rotate_rgba(img.as_raw(), w, deg) };
            let src = Image::from_vec_u8(w, w, rot, PixelType::U8x4)?;
            let mut dst = Image::new(size, size, PixelType::U8x4);
            resizer.resize(&src, &mut dst, &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3)))?;
            let px = dst.into_vec();
            let (mut outline, mut fill) = (Mask::new(size, size), Mask::new(size, size));
            for (i, p) in px.chunks_exact(4).enumerate() {
                let (x, y) = (i as u32 % size, i as u32 / size);
                if p[3] >= 110 {
                    outline.set(x, y);
                    if p[3] >= 200 && !is_dark(p) {
                        fill.set(x, y);
                    }
                }
            }
            variants.push((outline, fill));
        }
        out.push(NoteMasks { size, fill_rgb, outline_rgb: 0x0b0420, variants });
    }
    Ok(out)
}
