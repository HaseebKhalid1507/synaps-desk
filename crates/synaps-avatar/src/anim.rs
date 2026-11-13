//! Animation: moods (the phase-2 interface), clip playback and timing. Pure logic over
//! frame indices and Instants; the view drives it with one timer at the next deadline.

use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mood {
    Idle,
    Thinking,
    /// A tool is running.
    Working,
    /// Assistant text is streaming.
    Talking,
    /// A permission prompt is waiting on the user.
    NeedsYou,
    /// The turn finished.
    Done,
    /// Aborted or errored.
    Annoyed,
    /// Long idle or compaction.
    Sleepy,
    /// Session ended or detached.
    Away,
}

impl Mood {
    pub const ALL: [Mood; 9] = [
        Mood::Idle,
        Mood::Thinking,
        Mood::Working,
        Mood::Talking,
        Mood::NeedsYou,
        Mood::Done,
        Mood::Annoyed,
        Mood::Sleepy,
        Mood::Away,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Mood::Idle => "idle",
            Mood::Thinking => "thinking",
            Mood::Working => "working",
            Mood::Talking => "talking",
            Mood::NeedsYou => "needs_you",
            Mood::Done => "done",
            Mood::Annoyed => "annoyed",
            Mood::Sleepy => "sleepy",
            Mood::Away => "away",
        }
    }

    pub fn parse(s: &str) -> Option<Mood> {
        let k: String = s.trim().to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        Mood::ALL.into_iter().find(|m| m.key().replace('_', "") == k)
    }

    /// Fallback HUD label when the pack's mood table has none.
    pub fn default_status(self) -> &'static str {
        match self {
            Mood::Idle => "ONLINE",
            Mood::Thinking => "THINKING",
            Mood::Working => "WORKING",
            Mood::Talking => "SPEAKING",
            Mood::NeedsYou => "NEEDS YOU",
            Mood::Done => "DONE",
            Mood::Annoyed => "ERROR",
            Mood::Sleepy => "SLEEPING",
            Mood::Away => "OFFLINE",
        }
    }
}

/// (frame index, duration ms)
pub type Seq = Vec<(usize, u32)>;

/// Small xorshift; jitter and random picks don't need more.
pub struct Rng(u64);

impl Rng {
    pub fn seeded() -> Rng {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        Rng(t.as_nanos() as u64 | 1)
    }
    pub fn new(seed: u64) -> Rng {
        Rng(seed | 1)
    }
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    /// Uniform in [lo, hi).
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (self.next() >> 40) as f32 / (1u64 << 24) as f32 * (hi - lo)
    }
    pub fn pick(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

pub struct Animator {
    base: usize,
    looping: Option<Seq>,
    loop_pos: usize,
    oneshot: Option<Seq>,
    one_pos: usize,
    due: Option<Instant>,
    rng: Rng,
}

impl Animator {
    pub fn new(base: usize) -> Animator {
        Animator { base, looping: None, loop_pos: 0, oneshot: None, one_pos: 0, due: None, rng: Rng::seeded() }
    }

    pub fn frame(&self) -> usize {
        if let Some(s) = &self.oneshot {
            return s[self.one_pos].0;
        }
        if let Some(s) = &self.looping {
            return s[self.loop_pos].0;
        }
        self.base
    }

    /// The mood layer's resting frame (stable while loops and one-shots play).
    pub fn base(&self) -> usize {
        self.base
    }

    pub fn busy(&self) -> bool {
        self.oneshot.is_some()
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.due
    }

    /// Looped clips breathe: long holds vary so a blink loop doesn't tick like a metronome.
    fn loop_ms(&mut self, ms: u32) -> u64 {
        if ms >= 400 { (ms as f32 * self.rng.range(0.6, 1.7)) as u64 } else { ms as u64 }
    }

    fn start_loop(&mut self, now: Instant) {
        self.loop_pos = 0;
        self.due = match &self.looping {
            Some(s) => {
                let ms = s[0].1;
                Some(now + Duration::from_millis(self.loop_ms(ms)))
            }
            None => None,
        };
    }

    /// Mood layer: base frame + optional loop. A playing one-shot finishes first.
    pub fn set_layer(&mut self, base: usize, looping: Option<Seq>, now: Instant) {
        self.base = base;
        self.looping = looping.filter(|s| !s.is_empty());
        if self.oneshot.is_none() {
            self.start_loop(now);
        }
    }

    /// Play a sequence once, replacing any one-shot in progress.
    pub fn play(&mut self, seq: Seq, now: Instant) {
        if seq.is_empty() {
            return;
        }
        self.due = Some(now + Duration::from_millis(seq[0].1 as u64));
        self.oneshot = Some(seq);
        self.one_pos = 0;
    }

    pub fn stop_oneshot(&mut self, now: Instant) {
        if self.oneshot.take().is_some() {
            self.start_loop(now);
        }
    }

    /// Advance past every step that ended by `now`. True if the shown frame changed.
    pub fn tick(&mut self, now: Instant) -> bool {
        let before = self.frame();
        let mut guard = 0;
        while let Some(due) = self.due
            && due <= now
            && guard < 64
        {
            guard += 1;
            // After a stall (suspend, debugger) restart timing from now instead of replaying.
            let from = if now.duration_since(due) > Duration::from_secs(2) { now } else { due };
            if let Some(s) = &self.oneshot {
                self.one_pos += 1;
                if self.one_pos < s.len() {
                    self.due = Some(from + Duration::from_millis(s[self.one_pos].1 as u64));
                } else {
                    self.oneshot = None;
                    self.start_loop(from);
                }
            } else if let Some(s) = &self.looping {
                self.loop_pos = (self.loop_pos + 1) % s.len();
                let ms = s[self.loop_pos].1;
                let ms = self.loop_ms(ms);
                self.due = Some(from + Duration::from_millis(ms));
            } else {
                self.due = None;
            }
        }
        self.frame() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mood_parse() {
        assert_eq!(Mood::parse("needs_you"), Some(Mood::NeedsYou));
        assert_eq!(Mood::parse("NeedsYou"), Some(Mood::NeedsYou));
        assert_eq!(Mood::parse("idle"), Some(Mood::Idle));
        assert_eq!(Mood::parse("nope"), None);
    }

    #[test]
    fn oneshot_returns_to_loop() {
        let t = Instant::now();
        let mut a = Animator::new(0);
        a.set_layer(0, Some(vec![(0, 100), (1, 50)]), t);
        a.play(vec![(5, 100), (6, 100)], t);
        assert_eq!(a.frame(), 5);
        a.tick(t + Duration::from_millis(100));
        assert_eq!(a.frame(), 6);
        a.tick(t + Duration::from_millis(200));
        assert_eq!(a.frame(), 0);
        assert!(!a.busy());
    }
}
