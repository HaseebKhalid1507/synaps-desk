//! Pack loading and validation. A pack is a directory of PNG frames plus JSON:
//! `sprites.json` (the sprite catalog), `poses/poses.json` (docking poses, validated only)
//! and `moods.json` (the pack's behaviour sidecar: mood table, idle activities, click
//! reactions, HUD labels). The code ships no character ids; everything comes from here.

use anyhow::{Context as _, Result, anyhow, bail};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

#[derive(Deserialize, Clone, Debug)]
pub struct Anchor {
    pub name: String,
    pub x: i32,
    pub y: i32,
}

#[derive(Deserialize, Clone, Copy, Debug)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    pub fn right(&self) -> i32 {
        self.x + self.width
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.height
    }
    pub fn offset(&self, dx: i32, dy: i32) -> Rect {
        Rect { x: self.x + dx, y: self.y + dy, ..*self }
    }
    pub fn union(&self, o: &Rect) -> Rect {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect { x, y, width: self.right().max(o.right()) - x, height: self.bottom().max(o.bottom()) - y }
    }
}

#[derive(Deserialize, Clone, Copy, Debug)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Deserialize, Clone, Copy, Debug)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Sprite {
    pub id: String,
    pub file: String,
    #[serde(default)]
    pub kind: String,
    pub anchor: Anchor,
    pub visible_bounds: Rect,
    #[serde(default)]
    pub sha256: Option<String>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ClipFrame {
    pub id: String,
    pub ms: u32,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Clip {
    pub id: String,
    pub frames: Vec<ClipFrame>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Action {
    pub id: String,
    pub animation: String,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Autonomy {
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default = "first_ms")]
    pub first_activity_ms: u64,
    #[serde(default = "gap_ms")]
    pub activity_gap_ms: u64,
}

fn yes() -> bool {
    true
}
fn first_ms() -> u64 {
    18_000
}
fn gap_ms() -> u64 {
    12_000
}

impl Default for Autonomy {
    fn default() -> Self {
        Autonomy { enabled: true, first_activity_ms: first_ms(), activity_gap_ms: gap_ms() }
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub schema_version: u32,
    #[serde(default)]
    pub character: Option<String>,
    pub default_sprite: String,
    pub canvas: Size,
    pub canonical_anchor: Point,
    pub sprites: Vec<Sprite>,
    #[serde(default)]
    pub clips: Vec<Clip>,
    #[serde(default)]
    pub actions: Vec<Action>,
    #[serde(default)]
    pub autonomy: Autonomy,
}

#[derive(Deserialize, Debug)]
pub struct PoseCatalog {
    pub sprites: Vec<Sprite>,
}

/// One step of a scripted sequence: a catalog clip id, or one frame held for `ms`.
#[derive(Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum Step {
    Clip(String),
    Hold { frame: String, ms: u32 },
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MoodSpec {
    /// Frame shown while the mood holds.
    pub base: String,
    /// Clip looped over the base (e.g. blink, speech). Its frames should share the base pose.
    #[serde(default)]
    pub r#loop: Option<String>,
    /// Played once on entering the mood.
    #[serde(default)]
    pub entry: Option<Vec<Step>>,
    /// Played once on leaving the mood (e.g. stand up after sitting).
    #[serde(default)]
    pub exit: Option<Vec<Step>>,
    /// Idle autonomy runs only in moods that allow it.
    #[serde(default)]
    pub autonomy: bool,
    /// Status tag text (upper-case HUD label).
    #[serde(default)]
    pub status: Option<String>,
    /// Key into `tones` for the status colour.
    #[serde(default)]
    pub tone: Option<String>,
    /// Automatically switch to another mood after `afterMs`.
    #[serde(default)]
    pub then: Option<Then>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Then {
    pub mood: String,
    pub after_ms: u64,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExtraFrame {
    pub id: String,
    pub file: String,
    /// Borrow anchor and visible bounds from this catalog sprite (same registration).
    pub like: String,
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ClickSpec {
    #[serde(default)]
    pub head: Vec<Step>,
    #[serde(default)]
    pub body: Vec<Vec<Step>>,
    #[serde(default)]
    pub annoyed: Vec<Step>,
    /// Rapid clicks (each within `streakMs` of the last) before `annoyed` plays.
    #[serde(default = "annoyed_after")]
    pub annoyed_after: u32,
    #[serde(default = "streak_ms")]
    pub streak_ms: u64,
    /// Fraction of the frame's visible height counted as the head, from the top.
    #[serde(default = "head_fraction")]
    pub head_fraction: f32,
}

fn annoyed_after() -> u32 {
    5
}
fn streak_ms() -> u64 {
    1_600
}
fn head_fraction() -> f32 {
    0.3
}

#[derive(Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Behaviour {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub font: Option<String>,
    #[serde(default)]
    pub font_files: Vec<String>,
    #[serde(default)]
    pub moods: BTreeMap<String, MoodSpec>,
    #[serde(default)]
    pub activities: Vec<Vec<Step>>,
    #[serde(default)]
    pub extra_frames: Vec<ExtraFrame>,
    #[serde(default)]
    pub click: ClickSpec,
    #[serde(default)]
    pub tones: BTreeMap<String, String>,
}

/// A frame to decode: catalog sprite or sidecar extra.
#[derive(Clone, Debug)]
pub struct FrameSource {
    pub id: String,
    pub path: PathBuf,
    pub anchor: Anchor,
    pub bounds: Rect,
    pub sha256: Option<String>,
}

pub struct Pack {
    pub dir: PathBuf,
    pub name: String,
    pub catalog: Catalog,
    pub behaviour: Behaviour,
    pub frames: Vec<FrameSource>,
    pub index: HashMap<String, usize>,
    pub poses: Vec<FrameSource>,
}

impl Pack {
    pub fn frame(&self, id: &str) -> Result<usize> {
        self.index.get(id).copied().ok_or_else(|| anyhow!("unknown frame id `{id}`"))
    }

    pub fn clip(&self, id: &str) -> Option<&Clip> {
        let id = self.catalog.actions.iter().find(|a| a.id == id).map_or(id, |a| a.animation.as_str());
        self.catalog.clips.iter().find(|c| c.id == id)
    }

    /// Where a frame is drawn on the canonical canvas: its own anchor moved onto the
    /// canonical ground point. Seat-anchored frames keep their vertical registration
    /// (they are authored to the seat line, not the floor).
    pub fn offset(&self, f: &FrameSource) -> (i32, i32) {
        let c = self.catalog.canonical_anchor;
        let dy = if f.anchor.name == "ground" { c.y - f.anchor.y } else { 0 };
        (c.x - f.anchor.x, dy)
    }

    /// Union of every body frame's visible bounds after anchor registration. Every
    /// frame is cropped to this one rect so they stay aligned.
    pub fn crop_rect(&self) -> Rect {
        let mut r: Option<Rect> = None;
        for f in &self.frames {
            let (dx, dy) = self.offset(f);
            let b = f.bounds.offset(dx, dy);
            r = Some(r.map_or(b, |r| r.union(&b)));
        }
        let r = r.expect("pack has frames");
        let (cw, ch) = (self.catalog.canvas.width as i32, self.catalog.canvas.height as i32);
        let x = (r.x - 2).max(0);
        let y = (r.y - 2).max(0);
        Rect { x, y, width: (r.right() + 2).min(cw) - x, height: (r.bottom() + 2).min(ch) - y }
    }

    pub fn load(dir: &Path) -> Result<Pack> {
        let read = |rel: &str| -> Result<String> {
            let p = dir.join(rel);
            std::fs::read_to_string(&p).with_context(|| format!("pack: cannot read {}", p.display()))
        };
        let catalog: Catalog = serde_json::from_str(&read("sprites.json")?)
            .with_context(|| format!("pack: {}/sprites.json is not a valid sprite catalog", dir.display()))?;
        if catalog.schema_version != 2 {
            bail!("pack: sprites.json schemaVersion {} (expected 2)", catalog.schema_version);
        }
        let behaviour: Behaviour = match std::fs::read_to_string(dir.join("moods.json")) {
            Ok(s) => serde_json::from_str(&s)
                .with_context(|| format!("pack: {}/moods.json is invalid", dir.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Behaviour::default(),
            Err(e) => return Err(e).context("pack: cannot read moods.json"),
        };

        let mut errors = Vec::new();
        let mut frames = Vec::new();
        let mut index = HashMap::new();
        let (cw, ch) = (catalog.canvas.width as i32, catalog.canvas.height as i32);
        for s in &catalog.sprites {
            // Effects (e.g. a heart burst) are overlays, not body frames.
            if s.kind == "effect" {
                continue;
            }
            let path = dir.join(&s.file);
            if !path.is_file() {
                errors.push(format!("sprite `{}`: file {} is missing", s.id, s.file));
            }
            let b = s.visible_bounds;
            if b.x < 0 || b.y < 0 || b.right() > cw || b.bottom() > ch || b.width <= 0 || b.height <= 0 {
                errors.push(format!("sprite `{}`: visibleBounds {:?} outside the {cw}x{ch} canvas", s.id, b));
            }
            if index.insert(s.id.clone(), frames.len()).is_some() {
                errors.push(format!("sprite id `{}` is defined twice", s.id));
            }
            frames.push(FrameSource {
                id: s.id.clone(),
                path,
                anchor: s.anchor.clone(),
                bounds: b,
                sha256: s.sha256.clone(),
            });
        }
        for x in &behaviour.extra_frames {
            let Some(&like) = index.get(&x.like) else {
                errors.push(format!("moods.json extraFrames `{}`: `like` frame `{}` is unknown", x.id, x.like));
                continue;
            };
            let path = dir.join(&x.file);
            if !path.is_file() {
                errors.push(format!("moods.json extraFrames `{}`: file {} is missing", x.id, x.file));
            }
            let base = frames[like].clone();
            index.insert(x.id.clone(), frames.len());
            frames.push(FrameSource { id: x.id.clone(), path, sha256: None, ..base });
        }

        let has = |id: &str| index.contains_key(id);
        if !has(&catalog.default_sprite) {
            errors.push(format!("defaultSprite `{}` is not a sprite id", catalog.default_sprite));
        }
        for c in &catalog.clips {
            for f in &c.frames {
                if !has(&f.id) {
                    errors.push(format!("clip `{}`: frame `{}` does not resolve", c.id, f.id));
                }
            }
        }
        for a in &catalog.actions {
            if !catalog.clips.iter().any(|c| c.id == a.animation) {
                errors.push(format!("action `{}`: clip `{}` does not exist", a.id, a.animation));
            }
        }

        let pack_name = behaviour
            .name
            .clone()
            .or_else(|| catalog.character.clone())
            .unwrap_or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());

        let mut pack = Pack { dir: dir.to_owned(), name: pack_name, catalog, behaviour, frames, index, poses: Vec::new() };

        // Behaviour references.
        let check_steps = |what: &str, steps: &[Step], errors: &mut Vec<String>, pack: &Pack| {
            for s in steps {
                match s {
                    Step::Clip(id) if pack.clip(id).is_none() => {
                        errors.push(format!("moods.json {what}: clip `{id}` does not exist"))
                    }
                    Step::Hold { frame, .. } if !pack.index.contains_key(frame) => {
                        errors.push(format!("moods.json {what}: frame `{frame}` does not resolve"))
                    }
                    _ => {}
                }
            }
        };
        for (name, m) in &pack.behaviour.moods {
            if crate::anim::Mood::parse(name).is_none() {
                errors.push(format!("moods.json: unknown mood `{name}`"));
            }
            if !pack.index.contains_key(&m.base) {
                errors.push(format!("moods.json mood `{name}`: base frame `{}` does not resolve", m.base));
            }
            if let Some(l) = &m.r#loop
                && pack.clip(l).is_none()
            {
                errors.push(format!("moods.json mood `{name}`: loop clip `{l}` does not exist"));
            }
            check_steps(&format!("mood `{name}` entry"), m.entry.as_deref().unwrap_or(&[]), &mut errors, &pack);
            check_steps(&format!("mood `{name}` exit"), m.exit.as_deref().unwrap_or(&[]), &mut errors, &pack);
            if let Some(t) = &m.then
                && crate::anim::Mood::parse(&t.mood).is_none()
            {
                errors.push(format!("moods.json mood `{name}`: then.mood `{}` is unknown", t.mood));
            }
        }
        for (i, a) in pack.behaviour.activities.iter().enumerate() {
            check_steps(&format!("activity #{i}"), a, &mut errors, &pack);
        }
        check_steps("click.head", &pack.behaviour.click.head, &mut errors, &pack);
        check_steps("click.annoyed", &pack.behaviour.click.annoyed, &mut errors, &pack);
        for (i, b) in pack.behaviour.click.body.iter().enumerate() {
            check_steps(&format!("click.body #{i}"), b, &mut errors, &pack);
        }
        for f in &pack.behaviour.font_files {
            if !dir.join(f).is_file() {
                errors.push(format!("moods.json fontFiles: {f} is missing"));
            }
        }

        // Docking poses: phase 3 uses them; validate now so a broken pack fails early.
        match std::fs::read_to_string(dir.join("poses/poses.json")) {
            Ok(s) => match serde_json::from_str::<PoseCatalog>(&s) {
                Ok(p) => {
                    for s in p.sprites {
                        let path = dir.join("poses").join(&s.file);
                        if !path.is_file() {
                            errors.push(format!("pose `{}`: file poses/{} is missing", s.id, s.file));
                        }
                        let (ax, ay) = (s.anchor.x, s.anchor.y);
                        if ax < 0 || ay < 0 || ax > cw || ay > ch {
                            errors.push(format!("pose `{}`: anchor ({ax},{ay}) outside the canvas", s.id));
                        }
                        pack.poses.push(FrameSource {
                            id: s.id,
                            path,
                            anchor: s.anchor,
                            bounds: s.visible_bounds,
                            sha256: s.sha256,
                        });
                    }
                }
                Err(e) => errors.push(format!("poses/poses.json is invalid: {e}")),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => errors.push(format!("poses/poses.json unreadable: {e}")),
        }

        if !errors.is_empty() {
            bail!("pack {} failed validation:\n  - {}", dir.display(), errors.join("\n  - "));
        }
        Ok(pack)
    }

    /// Flatten steps into (frame index, ms) pairs.
    pub fn flatten(&self, steps: &[Step]) -> Vec<(usize, u32)> {
        let mut out = Vec::new();
        for s in steps {
            match s {
                Step::Clip(id) => {
                    if let Some(c) = self.clip(id) {
                        out.extend(c.frames.iter().filter_map(|f| Some((*self.index.get(&f.id)?, f.ms))));
                    }
                }
                Step::Hold { frame, ms } => {
                    if let Some(&i) = self.index.get(frame) {
                        out.push((i, *ms));
                    }
                }
            }
        }
        out
    }

    pub fn tone(&self, key: Option<&str>) -> u32 {
        key.map_or(0x8ef5ed, |k| self.tone_or(k, 0x8ef5ed))
    }

    /// A `#rrggbb` colour from the sidecar's `tones`, or `fallback`.
    pub fn tone_or(&self, key: &str, fallback: u32) -> u32 {
        self.behaviour
            .tones
            .get(key)
            .and_then(|hex| u32::from_str_radix(hex.trim_start_matches('#'), 16).ok())
            .unwrap_or(fallback)
    }
}
