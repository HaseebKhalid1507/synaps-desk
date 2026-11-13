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
    /// Device-pixel horizontal centre of the frame's visible bounds.
    pub center_x: i32,
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

    let mut mask = Mask::new(geo.width, geo.height);
    for (i, p) in px.chunks_exact_mut(4).enumerate() {
        if p[3] >= HIT_ALPHA {
            mask.set(i as u32 % geo.width, i as u32 / geo.width);
        }
        p.swap(0, 2); // RGBA → BGRA (GPUI's sprite atlas format), straight alpha.
    }
    let rects = Arc::new(mask.rects());
    let vb = f.bounds.offset(dx, dy);
    let head_fraction = pack.behaviour.click.head_fraction;
    let (_, top) = geo.to_device(0, vb.y);
    let head_line = (top + vb.height as f32 * geo.scale * head_fraction).round() as i32;
    let (cx, _) = geo.to_device(vb.x + vb.width / 2, 0);
    let rgba = image::RgbaImage::from_raw(geo.width, geo.height, px).expect("buffer size");
    Ok(Frame {
        id: f.id.clone(),
        image: Arc::new(RenderImage::new(vec![image::Frame::new(rgba)])),
        mask,
        rects,
        head_line,
        center_x: cx.round() as i32,
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
