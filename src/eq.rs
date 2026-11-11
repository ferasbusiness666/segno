//! The equalizer: ten bands, from the deep bass (31 Hz) to the air at the top (16 kHz).
//!
//! It runs on the audio thread, after the decode-ahead buffer, so a change is heard at once.
//! Each band is a peaking filter (Robert Bristow-Johnson's "Audio EQ Cookbook"), worked out in
//! f64 so the low bands stay clean. Neighbouring bands overlap, so each filter's own gain is
//! worked out so the sound at every band's centre is exactly what its slider says (a small
//! 10×10 system solved, then refined), and the curve between them stays smooth. The music is
//! first turned down by the curve's biggest boost, so a boost never clips. Changes glide over about 60 ms, so moving a slider never
//! clicks, and a flat curve (or the equalizer off) leaves the music exactly as it was.

use std::f64::consts::PI;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rodio::{ChannelCount, Sample, SampleRate, Source};

/// The bands' centres in Hz, an octave apart.
pub const BANDS: [f64; 10] = [31.25, 62.5, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];
pub const LABELS: [&str; 10] = ["31", "62", "125", "250", "500", "1k", "2k", "4k", "8k", "16k"];
/// Furthest a band goes up or down, in dB.
pub const MAX_DB: f32 = 12.0;
pub type Gains = [f32; 10];
pub const FLAT: Gains = [0.0; 10];

/// The presets, in the order they're shown.
pub const PRESETS: [(&str, Gains); 9] = [
    ("Flat", FLAT),
    ("Bass boost", [6.0, 5.0, 4.0, 2.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Bass reducer", [-6.0, -5.0, -4.0, -2.0, -0.5, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Treble boost", [0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 1.5, 3.0, 4.5, 5.5]),
    ("Vocal", [-2.0, -2.0, -1.0, 0.0, 2.0, 3.5, 3.5, 2.5, 1.0, 0.0]),
    ("Rock", [4.5, 3.5, 2.0, 0.0, -1.0, -0.5, 1.0, 2.5, 3.5, 4.0]),
    ("Pop", [-1.0, 0.0, 1.5, 3.0, 3.5, 2.0, 0.5, -0.5, -0.5, -1.0]),
    ("Electronic", [4.5, 4.0, 1.5, 0.0, -1.5, 0.0, 1.0, 2.0, 3.5, 4.5]),
    ("Hip-hop", [5.0, 4.5, 2.0, 2.0, -1.0, -1.0, 1.0, 0.0, 1.0, 2.0]),
];

/// How wide each band is: a little over an octave, so neighbours blend into one smooth curve.
const Q: f64 = 1.0;
/// Furthest a single filter goes to make the curve match the sliders (only very jagged
/// curves need this much).
const MAX_INNER_DB: f64 = 24.0;
/// Filters are worked out again every this many frames while a change glides.
const BLOCK: u32 = 32;
const GLIDE: f64 = 0.06;

/// The equalizer's settings, shared by the window (which changes them) and the audio thread
/// (which reads them without ever waiting).
#[derive(Default)]
pub struct EqShared {
    version: AtomicU64,
    on: AtomicBool,
    gains: [AtomicU32; 10],
}

impl EqShared {
    pub fn set(&self, on: bool, gains: &Gains) {
        self.on.store(on, Ordering::Relaxed);
        for (a, g) in self.gains.iter().zip(gains) {
            a.store(g.clamp(-MAX_DB, MAX_DB).to_bits(), Ordering::Relaxed);
        }
        self.version.fetch_add(1, Ordering::Release);
    }

    /// The curve to play (flat while off), if it changed since `seen`.
    fn changed(&self, seen: u64) -> Option<(u64, Gains)> {
        let v = self.version.load(Ordering::Acquire);
        if v == seen {
            return None;
        }
        let on = self.on.load(Ordering::Relaxed);
        let mut g = FLAT;
        if on {
            for (g, a) in g.iter_mut().zip(&self.gains) {
                *g = f32::from_bits(a.load(Ordering::Relaxed));
            }
        }
        // Changed again while being read: take it next time round.
        (self.version.load(Ordering::Acquire) == v).then_some((v, g))
    }
}

/// The name of the preset a curve is, if it's one.
pub fn preset_of(g: &Gains) -> Option<&'static str> {
    PRESETS.iter().find(|(_, p)| p.iter().zip(g).all(|(a, b)| (a - b).abs() < 0.05)).map(|(n, _)| *n)
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Coefs {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Coefs {
    const IDENTITY: Coefs = Coefs { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };

    fn peaking(freq: f64, db: f64, rate: f64) -> Coefs {
        // A band too close to the top of what the rate can carry is left out.
        if db.abs() < 1e-4 || freq >= rate * 0.45 {
            return Coefs::IDENTITY;
        }
        let a = 10f64.powf(db / 40.0);
        let (s, c) = (2.0 * PI * freq / rate).sin_cos();
        let alpha = s / (2.0 * Q);
        let a0 = 1.0 + alpha / a;
        Coefs { b0: (1.0 + alpha * a) / a0, b1: -2.0 * c / a0, b2: (1.0 - alpha * a) / a0, a1: -2.0 * c / a0, a2: (1.0 - alpha / a) / a0 }
    }

    /// How much it changes the level of the frequency at `w` (radians per sample), as a factor.
    fn magnitude(&self, w: f64) -> f64 {
        // H(z) at z = e^{jw}: (b0 + b1 z^-1 + b2 z^-2) / (1 + a1 z^-1 + a2 z^-2).
        let (c1, s1) = (w.cos(), -w.sin());
        let (c2, s2) = ((2.0 * w).cos(), -(2.0 * w).sin());
        let (nr, ni) = (self.b0 + self.b1 * c1 + self.b2 * c2, self.b1 * s1 + self.b2 * s2);
        let (dr, di) = (1.0 + self.a1 * c1 + self.a2 * c2, self.a1 * s1 + self.a2 * s2);
        ((nr * nr + ni * ni) / (dr * dr + di * di)).sqrt()
    }
}

/// Each filter's own gain in dB (not the sliders': see `shaped`).
type Inner = [f64; 10];

fn coefs_for(g: &Inner, rate: f64) -> [Coefs; 10] {
    std::array::from_fn(|i| Coefs::peaking(BANDS[i], g[i], rate))
}

fn db_at(c: &[Coefs; 10], freq: f64, rate: f64) -> f64 {
    let w = 2.0 * PI * freq / rate;
    20.0 * c.iter().map(|c| c.magnitude(w)).product::<f64>().log10()
}

/// How much each band reaches into the others: row i, column j is band j's effect at band i's
/// centre, per dB of band j.
fn reach(rate: f64) -> [[f64; 10]; 10] {
    std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            let mut g = [0.0; 10];
            g[j] = 12.0;
            db_at(&coefs_for(&g, rate), BANDS[i], rate) / 12.0
        })
    })
}

/// Solves m·x = b (Gaussian elimination with partial pivoting; m is small and well-behaved).
fn solve(mut m: [[f64; 10]; 10], mut b: [f64; 10]) -> [f64; 10] {
    for col in 0..10 {
        let pivot = (col..10).max_by(|&a, &c| m[a][col].abs().total_cmp(&m[c][col].abs())).unwrap_or(col);
        m.swap(col, pivot);
        b.swap(col, pivot);
        if m[col][col].abs() < 1e-12 {
            continue;
        }
        for row in col + 1..10 {
            let k = m[row][col] / m[col][col];
            let pivot_row = m[col];
            for (v, p) in m[row].iter_mut().zip(pivot_row).skip(col) {
                *v -= k * p;
            }
            b[row] -= k * b[col];
        }
    }
    let mut x = [0.0; 10];
    for row in (0..10).rev() {
        let rest: f64 = (row + 1..10).map(|c| m[row][c] * x[c]).sum();
        x[row] = if m[row][row].abs() < 1e-12 { 0.0 } else { (b[row] - rest) / m[row][row] };
    }
    x
}

/// The filters' own gains that make the sound at each band's centre what its slider says.
fn shaped(g: &Gains, rate: f64, m: &[[f64; 10]; 10]) -> Inner {
    let want: Inner = std::array::from_fn(|i| g[i] as f64);
    if want.iter().all(|&v| v == 0.0) {
        return want;
    }
    let mut x = solve(*m, want);
    // The bands don't add up exactly in dB, so check the real curve and correct, a few times.
    for _ in 0..3 {
        x = x.map(|v| v.clamp(-MAX_INNER_DB, MAX_INNER_DB));
        let c = coefs_for(&x, rate);
        let off: [f64; 10] = std::array::from_fn(|i| want[i] - db_at(&c, BANDS[i], rate));
        let fix = solve(*m, off);
        for i in 0..10 {
            x[i] += fix[i];
        }
    }
    x.map(|v| v.clamp(-MAX_INNER_DB, MAX_INNER_DB))
}

/// A curve as it's heard, for drawing it.
pub struct Curve([Coefs; 10]);

impl Curve {
    pub fn new(g: &Gains) -> Curve {
        static REACH: std::sync::OnceLock<[[f64; 10]; 10]> = std::sync::OnceLock::new();
        let rate = 48_000.0;
        let m = REACH.get_or_init(|| reach(rate));
        Curve(coefs_for(&shaped(g, rate, m), rate))
    }

    /// What it does at `freq` Hz, in dB.
    pub fn db_at(&self, freq: f64) -> f64 {
        db_at(&self.0, freq, 48_000.0)
    }
}

/// The curve's biggest boost anywhere from 20 Hz up, in dB (0 when it only cuts): the music is
/// turned down by this much first, so nothing clips.
fn peak_db(c: &[Coefs; 10], rate: f64) -> f64 {
    let top = (rate * 0.49).min(20_000.0);
    let mut f = 20.0;
    let mut best = 0.0f64;
    while f <= top {
        best = best.max(db_at(c, f, rate));
        f *= 2f64.powf(1.0 / 24.0);
    }
    best
}

/// One filter's memory, for one channel (transposed direct form II).
#[derive(Clone, Copy, Default)]
struct Mem {
    z1: f64,
    z2: f64,
}

#[inline]
fn run(c: &Coefs, m: &mut Mem, x: f64) -> f64 {
    let y = c.b0 * x + m.z1;
    m.z1 = c.b1 * x - c.a1 * y + m.z2;
    m.z2 = c.b2 * x - c.a2 * y;
    y
}

/// A song played through the equalizer.
pub struct Eq<S> {
    inner: S,
    shared: Arc<EqShared>,
    seen: u64,
    rate: f64,
    reach: [[f64; 10]; 10],
    channels: usize,
    /// Position inside the current frame.
    ch: usize,
    until_update: u32,
    /// Where a change glides from and to (the filters' gains in dB, and the turn-down before
    /// them).
    from: (Inner, f64),
    to: (Inner, f64),
    /// How far along the glide is, 0 to 1.
    t: f64,
    now: Inner,
    coefs: [Coefs; 10],
    /// Bands that do something (the others are skipped).
    used: [bool; 10],
    pre: f64,
    mem: Vec<[Mem; 10]>,
    /// Anything to do at all (else samples pass straight through).
    active: bool,
}

impl<S: Source> Eq<S> {
    pub fn new(inner: S, shared: Arc<EqShared>) -> Self {
        let rate = inner.sample_rate().get() as f64;
        let channels = inner.channels().get() as usize;
        let mut eq = Eq {
            inner,
            shared,
            seen: u64::MAX,
            rate,
            reach: reach(rate),
            channels,
            ch: 0,
            until_update: 0,
            from: ([0.0; 10], 0.0),
            to: ([0.0; 10], 0.0),
            t: 1.0,
            now: [0.0; 10],
            coefs: [Coefs::IDENTITY; 10],
            used: [false; 10],
            pre: 1.0,
            mem: vec![[Mem::default(); 10]; channels],
            active: false,
        };
        // A song starts with the curve already in place, not gliding to it.
        if let Some((v, g)) = eq.shared.changed(u64::MAX) {
            eq.seen = v;
            let g = shaped(&g, rate, &eq.reach);
            let pre = -peak_db(&coefs_for(&g, rate), rate);
            eq.from = (g, pre);
            eq.to = (g, pre);
            eq.apply(g, pre);
        }
        eq
    }

    /// Between frames: pick up a change, and move a glide along.
    fn update(&mut self) {
        if let Some((v, g)) = self.shared.changed(self.seen) {
            self.seen = v;
            let g = shaped(&g, self.rate, &self.reach);
            let pre = -peak_db(&coefs_for(&g, self.rate), self.rate);
            self.from = (self.now, 20.0 * self.pre.log10());
            self.to = (g, pre);
            self.t = 0.0;
        }
        if self.t < 1.0 {
            self.t = (self.t + BLOCK as f64 / (GLIDE * self.rate)).min(1.0);
            let k = self.t;
            let g: Inner = std::array::from_fn(|i| self.from.0[i] + (self.to.0[i] - self.from.0[i]) * k);
            let pre = self.from.1 + (self.to.1 - self.from.1) * k;
            self.apply(g, pre);
        }
        // Long silences would leave tiny numbers behind that slow some processors down.
        for m in self.mem.iter_mut().flatten() {
            if m.z1.abs() < 1e-20 {
                m.z1 = 0.0;
            }
            if m.z2.abs() < 1e-20 {
                m.z2 = 0.0;
            }
        }
    }

    fn apply(&mut self, g: Inner, pre_db: f64) {
        self.now = g;
        self.coefs = coefs_for(&g, self.rate);
        for i in 0..10 {
            self.used[i] = self.coefs[i] != Coefs::IDENTITY;
            if !self.used[i] {
                self.mem.iter_mut().for_each(|m| m[i] = Mem::default());
            }
        }
        self.pre = 10f64.powf(pre_db / 20.0);
        self.active = self.used.iter().any(|&u| u) || pre_db.abs() > 1e-6;
    }
}

impl<S: Source> Iterator for Eq<S> {
    type Item = Sample;

    #[inline]
    fn next(&mut self) -> Option<Sample> {
        if self.ch == 0 {
            if self.until_update == 0 {
                self.update();
                self.until_update = BLOCK;
            }
            self.until_update -= 1;
        }
        let x = self.inner.next()?;
        let ch = self.ch;
        self.ch = if ch + 1 >= self.channels { 0 } else { ch + 1 };
        if !self.active {
            return Some(x);
        }
        let mut y = x as f64 * self.pre;
        for ((c, m), &used) in self.coefs.iter().zip(self.mem[ch].iter_mut()).zip(&self.used) {
            if used {
                y = run(c, m, y);
            }
        }
        Some(y as Sample)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<S: Source> Source for Eq<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }
    fn channels(&self) -> ChannelCount {
        self.inner.channels()
    }
    fn sample_rate(&self) -> SampleRate {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        // What the filters remember belongs to the old spot.
        self.mem.iter_mut().for_each(|m| *m = [Mem::default(); 10]);
        self.ch = 0;
        self.inner.try_seek(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rodio::buffer::SamplesBuffer;
    use std::num::NonZero;

    const RATE: u32 = 48_000;

    /// `secs` of a sine at `freq` Hz, the same in both channels.
    fn sine(freq: f64, amp: f32, secs: f64) -> Vec<f32> {
        let n = (secs * RATE as f64) as usize;
        (0..n).flat_map(|i| {
            let v = amp * (2.0 * PI * freq * i as f64 / RATE as f64).sin() as f32;
            [v, v]
        })
        .collect()
    }

    fn through(samples: Vec<f32>, on: bool, g: Gains) -> (Vec<f32>, Arc<EqShared>) {
        let shared = Arc::new(EqShared::default());
        shared.set(on, &g);
        let src = SamplesBuffer::new(NonZero::new(2).unwrap(), NonZero::new(RATE).unwrap(), samples);
        (Eq::new(src, shared.clone()).collect(), shared)
    }

    fn rms(s: &[f32]) -> f64 {
        (s.iter().map(|&x| (x as f64).powi(2)).sum::<f64>() / s.len() as f64).sqrt()
    }

    fn db(a: f64, b: f64) -> f64 {
        20.0 * (a / b).log10()
    }

    /// The level change at `freq` with curve `g`, measured after the filters settle.
    fn change_at(freq: f64, g: Gains) -> f64 {
        let input = sine(freq, 0.5, 1.0);
        let (out, _) = through(input.clone(), true, g);
        let tail = input.len() / 2;
        db(rms(&out[tail..]), rms(&input[tail..]))
    }

    #[test]
    fn off_or_flat_leaves_the_music_exactly_as_it_was() {
        let input = sine(440.0, 0.8, 0.2);
        assert_eq!(through(input.clone(), false, PRESETS[1].1).0, input);
        assert_eq!(through(input.clone(), true, FLAT).0, input);
    }

    #[test]
    fn a_band_moves_its_own_frequency() {
        let mut g = FLAT;
        g[5] = -6.0;
        // A cut: 1 kHz goes 6 dB below the bass. (Its neighbours rise a fraction of a dB to stay
        // where their sliders are, so everything is turned down that fraction.)
        let (mid, bass) = (change_at(1000.0, g), change_at(62.5, g));
        assert!((mid - bass + 6.0).abs() < 0.2, "{mid} vs {bass}");
        assert!(bass <= 0.0 && bass > -1.0, "{bass}");
        // A boost: the music is turned down by 6 dB first, so 1 kHz ends up where it was and
        // everything else is 6 dB lower.
        g[5] = 6.0;
        assert!(change_at(1000.0, g).abs() < 0.2, "{}", change_at(1000.0, g));
        assert!((change_at(62.5, g) + 6.0).abs() < 0.3);
        // The lowest and highest bands work too.
        let mut g = FLAT;
        g[0] = -9.0;
        g[9] = -9.0;
        let mid = change_at(1000.0, g);
        assert!((change_at(31.25, g) - mid + 9.0).abs() < 0.3);
        assert!((change_at(16000.0, g) - mid + 9.0).abs() < 0.5);
    }

    #[test]
    fn boosts_never_clip() {
        let all_up = [MAX_DB; 10];
        for freq in [40.0, 125.0, 700.0, 3000.0, 12000.0] {
            let (out, _) = through(sine(freq, 1.0, 0.5), true, all_up);
            let peak = out.iter().fold(0f32, |m, &x| m.max(x.abs()));
            assert!(peak <= 1.01, "{freq} Hz peaks at {peak}");
        }
    }

    #[test]
    fn changes_glide_instead_of_jumping() {
        let shared = Arc::new(EqShared::default());
        shared.set(true, &FLAT);
        let src = SamplesBuffer::new(NonZero::new(2).unwrap(), NonZero::new(RATE).unwrap(), sine(1000.0, 0.5, 1.0));
        let mut eq = Eq::new(src, shared.clone());
        let before: Vec<f32> = eq.by_ref().take(RATE as usize / 5 * 2).collect();
        let mut g = FLAT;
        g[5] = -12.0;
        shared.set(true, &g);
        let after: Vec<f32> = eq.collect();
        // 2.5 ms windows: the level steps down a little at a time, never all at once.
        let win = (RATE as usize / 400) * 2;
        let levels: Vec<f64> = std::iter::once(rms(&before[before.len() - win..])).chain(after.chunks(win).map(rms)).collect();
        let worst = levels.windows(2).map(|w| db(w[1], w[0]).abs()).fold(0.0, f64::max);
        assert!(worst < 2.0, "a step of {worst:.2} dB");
        let total = db(*levels.last().unwrap(), levels[0]);
        assert!(total < -12.0 && total > -14.0, "{total}");
    }

    #[test]
    fn channels_stay_apart() {
        let input: Vec<f32> = sine(250.0, 0.7, 0.3).chunks(2).flat_map(|f| [f[0], 0.0]).collect();
        let (out, _) = through(input, true, PRESETS[5].1);
        assert!(out.chunks(2).all(|f| f[1].abs() < 1e-9));
        assert!(rms(&out) > 0.1);
    }

    #[test]
    fn presets_are_named() {
        assert_eq!(preset_of(&FLAT), Some("Flat"));
        assert_eq!(preset_of(&PRESETS[4].1), Some("Vocal"));
        let mut g = PRESETS[4].1;
        g[0] += 1.0;
        assert_eq!(preset_of(&g), None);
    }

    #[test]
    fn the_sound_at_each_band_is_what_its_slider_says() {
        let mut one = FLAT;
        one[5] = 9.0;
        for g in [PRESETS[1].1, PRESETS[4].1, PRESETS[5].1, PRESETS[8].1, one, [MAX_DB; 10]] {
            let c = Curve::new(&g);
            for i in 0..10 {
                let at = c.db_at(BANDS[i]);
                assert!((at - g[i] as f64).abs() < 0.1, "{g:?}: band {i} is {at:.2}");
            }
        }
        // And smooth between them: Bass boost never wobbles more than a fraction of a dB.
        let c = Curve::new(&PRESETS[1].1);
        let mut f = BANDS[0];
        while f < BANDS[4] {
            let between = PRESETS[1].1;
            let k = (f / BANDS[0]).log2();
            let (lo, hi) = (k.floor() as usize, (k.floor() as usize + 1).min(9));
            let straight = between[lo] as f64 + (between[hi] - between[lo]) as f64 * (k - k.floor());
            assert!((c.db_at(f) - straight).abs() < 0.5, "{f} Hz");
            f *= 1.05;
        }
    }
}
