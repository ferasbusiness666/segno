//! The audio engine. It lives on its own thread and owns the output device, so music keeps
//! playing (and moves on to the next song) even when the window is hidden and the UI is asleep.
//!
//! Battery notes:
//! - The thread blocks on its command channel; it wakes only for commands or a song ending.
//! - After a pause of 20 s the output device is closed, so Windows can power the audio path down.
//!   Pressing play again reopens it and seeks back to where you were.
//!
//! Volume never jumps: play, pause, skipping, seeking and the volume slider all ramp the sound
//! (see `Fader`), so there are no clicks or pops.

use crate::store::LockExt;
use crate::store::Repeat;
use rodio::source::EmptyCallback;
use rodio::Source;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

static UID: AtomicU64 = AtomicU64::new(1);

/// How a song got into the queue (the queue groups them so you can see what plays next).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Origin {
    /// Part of the list you pressed play on (an album, a playlist, Songs…).
    #[default]
    List,
    /// You added it: Play next, Add to queue, or dragged in.
    Added,
    /// Segno carried on with your library after the queue ran out.
    Library,
}

impl Origin {
    pub fn code(self) -> u8 {
        match self {
            Origin::List => 0,
            Origin::Added => 1,
            Origin::Library => 2,
        }
    }
    pub fn from_code(c: u8) -> Self {
        match c {
            1 => Origin::Added,
            2 => Origin::Library,
            _ => Origin::List,
        }
    }
}

#[derive(Clone, Debug)]
pub struct QItem {
    /// Unique per queue entry, so the same song can be in the queue twice.
    pub uid: u64,
    /// Library track id.
    pub id: u64,
    pub path: PathBuf,
    /// Plays from its site: the file arrives when the app has fetched it (see `Cmd::Fetched`).
    pub stream: bool,
    pub origin: Origin,
}

impl QItem {
    pub fn new(id: u64, path: PathBuf) -> Self {
        Self {
            uid: UID.fetch_add(1, Ordering::Relaxed),
            id,
            path,
            stream: false,
            origin: Origin::List,
        }
    }

    pub fn with_origin(mut self, origin: Origin) -> Self {
        self.origin = origin;
        self
    }

    /// A song that plays from its site. `path` is its fetched file, or a placeholder until then.
    pub fn stream(id: u64, path: PathBuf) -> Self {
        Self { stream: true, ..Self::new(id, path) }
    }
}

pub enum Cmd {
    /// Replace the queue and start playing `start`.
    PlayList {
        items: Vec<QItem>,
        start: usize,
        shuffle: Option<bool>,
    },
    Toggle,
    Play,
    Pause,
    Next,
    Prev,
    Seek(f32),
    SetVolume(f32),
    SetMuted(bool),
    SetShuffle(bool),
    SetRepeat(Repeat),
    PlayNext(Vec<QItem>),
    AddToQueue(Vec<QItem>),
    RemoveUid(u64),
    Move { from: usize, to: usize },
    JumpTo(usize),
    ClearUpcoming,
    Restore {
        queue: Vec<QItem>,
        original: Vec<QItem>,
        index: usize,
        position: f32,
    },
    Stop,
    TrackEnded(u64),
    DeviceError,
    /// Insert songs at a position in the queue (dragged in).
    InsertAt { at: usize, items: Vec<QItem> },
    /// Keep going with songs from the library when the queue runs out.
    SetAutoplay(bool),
    /// A streamed song's file is ready (every queue entry for that song gets it).
    Fetched { id: u64, path: PathBuf },
    /// A streamed song has started downloading: it can play from the part that's there.
    Arriving { id: u64, arrival: crate::growing::Arrival },
    /// A streamed song couldn't be fetched: if it's the one waiting, move on.
    FetchFailed { id: u64 },
    /// Set or turn off the sleep timer.
    SetSleep(Sleep),
}

/// The sleep timer: the music fades out over its last 30 s and pauses at a set time, or it
/// stops when the song that's playing ends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sleep {
    #[default]
    Off,
    At(Instant),
    EndOfSong,
}

/// How long the sleep timer takes to fade the music out.
pub const SLEEP_FADE: f32 = 30.0;
/// How long after Segno hands audio over it's heard: the ~150 ms kept queued ahead of the
/// speakers (see `ensure_sink`). The position counts what was handed over.
pub const HEARD_LATE: f32 = 0.15;
/// How often the volume steps down during that fade (it glides between steps).
const SLEEP_TICK: Duration = Duration::from_millis(200);

/// How much of the volume the sleep timer leaves: all of it until the last 30 s, then less
/// and less (squared, so it sounds even) down to nothing.
fn sleep_share(sleep: Sleep, now: Instant) -> f32 {
    match sleep {
        Sleep::At(t) => {
            let f = (t.saturating_duration_since(now).as_secs_f32() / SLEEP_FADE).min(1.0);
            f * f
        }
        _ => 1.0,
    }
}

/// The sleep timer is in its last 30 s, fading the music out.
fn sleep_fading(sleep: Sleep, now: Instant) -> bool {
    matches!(sleep, Sleep::At(t) if t.saturating_duration_since(now).as_secs_f32() < SLEEP_FADE)
}

/// When the sleep timer next needs the engine: when its fade starts, then often during it.
fn sleep_wake(sleep: Sleep, now: Instant) -> Option<Instant> {
    let Sleep::At(t) = sleep else { return None };
    let fade_from = t.checked_sub(Duration::from_secs_f32(SLEEP_FADE)).unwrap_or(now);
    Some(if now < fade_from { fade_from } else { (now + SLEEP_TICK).min(t) })
}

#[derive(Clone, Default)]
pub struct State {
    pub queue: Vec<QItem>,
    pub original: Vec<QItem>,
    pub index: usize,
    pub playing: bool,
    pub shuffle: bool,
    pub repeat: Repeat,
    pub volume: f32,
    pub muted: bool,
    /// Duration reported by the decoder (0 if unknown; the UI falls back to the library).
    pub duration: f32,
    /// Position when no output is open (restored or idle).
    pub idle_pos: f32,
    pub error: Option<String>,
    /// Bumped on every change, so the UI and the media overlay know when to refresh.
    pub version: u64,
    pub autoplay: bool,
    /// The current song plays from its site and is still being fetched.
    pub waiting: bool,
    pub sleep: Sleep,
}

impl State {
    pub fn current(&self) -> Option<&QItem> {
        self.queue.get(self.index)
    }
}

#[derive(Clone)]
pub struct PlayerHandle {
    tx: Sender<Cmd>,
    pub state: Arc<Mutex<State>>,
    live: Arc<Mutex<Option<Arc<rodio::Player>>>>,
    /// The library in "Songs" order, used to keep playing after the queue ends.
    pub library: Arc<Mutex<Vec<(u64, PathBuf)>>>,
    /// Asks for a streamed song's file (by track id). The answer comes back as `Cmd::Fetched`
    /// or `Cmd::FetchFailed`, straight to this thread, so it works while the window sleeps.
    pub fetcher: Fetcher,
    /// The equalizer's curve: set from the window, heard at once.
    pub eq: Arc<crate::eq::EqShared>,
}

pub type Fetcher = Arc<Mutex<Option<Arc<dyn Fn(u64) + Send + Sync>>>>;

impl PlayerHandle {
    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
    pub fn snapshot(&self) -> State {
        self.state.lk().clone()
    }
    /// Current position in seconds.
    pub fn position(&self) -> f32 {
        if let Some(p) = self.live.lk().as_ref() {
            return p.get_pos().as_secs_f32();
        }
        self.state.lk().idle_pos
    }
}

pub fn spawn(notify: Arc<dyn Fn() + Send + Sync>) -> PlayerHandle {
    let (tx, rx) = channel();
    let state = Arc::new(Mutex::new(State {
        volume: 0.8,
        autoplay: true,
        ..Default::default()
    }));
    let live = Arc::new(Mutex::new(None));
    let library = Arc::new(Mutex::new(Vec::new()));
    let fetcher: Fetcher = Arc::new(Mutex::new(None));
    let eq = Arc::new(crate::eq::EqShared::default());
    let handle = PlayerHandle {
        tx: tx.clone(),
        state: state.clone(),
        live: live.clone(),
        library: library.clone(),
        fetcher: fetcher.clone(),
        eq: eq.clone(),
    };
    std::thread::Builder::new()
        .name("audio".into())
        .spawn(move || {
            let mut eng = Engine {
                tx,
                state,
                live,
                notify,
                sink: None,
                gen: 0,
                teardown_at: None,
                rng: seed(),
                last_device_error: None,
                library,
                fetcher,
                fade: None,
                arriving: Default::default(),
                growing: None,
                eq,
            };
            eng.run(rx);
        })
        .expect("audio thread");
    handle
}

fn seed() -> u64 {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E3779B97F4A7C15);
    t | 1
}

/// Perceived loudness is roughly logarithmic; a squared curve makes the slider feel even.
fn gain(volume: f32, muted: bool) -> f32 {
    if muted {
        0.0
    } else {
        let v = volume.clamp(0.0, 1.0);
        v * v
    }
}

/// Shared with a `Fader`: where its volume is heading, how fast, and where it is now.
struct FadeCtl {
    target: AtomicU32,
    /// Volume change per second.
    rate: AtomicU32,
    level: AtomicU32,
    /// Jump straight to `snap_to` before the next sample.
    snap: AtomicBool,
    snap_to: AtomicU32,
}

impl FadeCtl {
    fn new(level: f32) -> Arc<Self> {
        Arc::new(Self {
            target: AtomicU32::new(level.to_bits()),
            rate: AtomicU32::new(1.0f32.to_bits()),
            level: AtomicU32::new(level.to_bits()),
            snap: AtomicBool::new(false),
            snap_to: AtomicU32::new(0),
        })
    }

    fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    /// Glide to `target` over about `secs`.
    fn go(&self, target: f32, secs: f32) {
        let rate = ((target - self.level()).abs() / secs.max(0.001)).max(0.05);
        self.rate.store(rate.to_bits(), Ordering::Relaxed);
        self.target.store(target.to_bits(), Ordering::Relaxed);
    }

    /// Set the volume right away (only while no sound is coming out).
    fn set_now(&self, v: f32) {
        self.snap_to.store(v.to_bits(), Ordering::Relaxed);
        self.level.store(v.to_bits(), Ordering::Relaxed);
        self.snap.store(true, Ordering::Release);
    }
}

/// Plays a song through a volume that glides instead of jumping.
struct Fader<S> {
    inner: S,
    ctl: Arc<FadeCtl>,
    gain: f32,
    /// Position inside the current frame (one sample per channel); the volume moves per frame.
    at: u16,
}

impl<S: Source> Iterator for Fader<S> {
    type Item = rodio::Sample;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let v = self.inner.next()?;
        if self.at == 0 {
            if self.ctl.snap.swap(false, Ordering::Acquire) {
                self.gain = f32::from_bits(self.ctl.snap_to.load(Ordering::Relaxed));
            }
            let target = f32::from_bits(self.ctl.target.load(Ordering::Relaxed));
            if self.gain != target {
                let step = f32::from_bits(self.ctl.rate.load(Ordering::Relaxed)) / self.inner.sample_rate().get() as f32;
                let d = target - self.gain;
                self.gain = if d.abs() <= step { target } else { self.gain + step.copysign(d) };
                self.ctl.level.store(self.gain.to_bits(), Ordering::Relaxed);
            }
        }
        self.at += 1;
        if self.at >= self.inner.channels().get() {
            self.at = 0;
        }
        Some(v * self.gain as rodio::Sample)
    }

    #[inline]
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<S: Source> Source for Fader<S> {
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }
    fn channels(&self) -> rodio::ChannelCount {
        self.inner.channels()
    }
    fn sample_rate(&self) -> rodio::SampleRate {
        self.inner.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), rodio::source::SeekError> {
        self.at = 0;
        self.inner.try_seek(pos)
    }
}

// How long the volume takes to glide (seconds).
const FADE_PAUSE: f32 = 0.2;
const FADE_PLAY: f32 = 0.3;
const FADE_SKIP: f32 = 0.08;
const FADE_SEEK: f32 = 0.06;
const FADE_VOLUME: f32 = 0.08;
/// A song starting from the top only gets a tiny ramp, so its first beat isn't softened.
const FADE_START: f32 = 0.012;

struct Engine {
    tx: Sender<Cmd>,
    state: Arc<Mutex<State>>,
    live: Arc<Mutex<Option<Arc<rodio::Player>>>>,
    notify: Arc<dyn Fn() + Send + Sync>,
    sink: Option<rodio::MixerDeviceSink>,
    gen: u64,
    teardown_at: Option<Instant>,
    rng: u64,
    last_device_error: Option<Instant>,
    library: Arc<Mutex<Vec<(u64, PathBuf)>>>,
    fetcher: Fetcher,
    /// The volume ramp of the song that's loaded.
    fade: Option<Arc<FadeCtl>>,
    /// Streamed songs that are on their way, by track id (until they're all here).
    arriving: std::collections::HashMap<u64, crate::growing::Arrival>,
    /// The song that's loaded plays from a file that was still downloading when it started
    /// (it can't seek until the whole file is here).
    growing: Option<Arc<crate::growing::Download>>,
    eq: Arc<crate::eq::EqShared>,
}

impl Engine {
    /// The volume the slider asks for (0 when muted), lowered while the sleep timer fades out.
    fn want_gain(&self) -> f32 {
        let s = self.state.lk();
        gain(s.volume, s.muted) * sleep_share(s.sleep, Instant::now())
    }

    /// Someone pressed something while the sleep timer was fading the music out: they're
    /// awake, so the timer stops (the command itself brings the volume back).
    fn awake(&self, c: &Cmd) {
        let pressed = matches!(
            c,
            Cmd::Toggle
                | Cmd::Play
                | Cmd::Pause
                | Cmd::Next
                | Cmd::Prev
                | Cmd::Seek(_)
                | Cmd::SetVolume(_)
                | Cmd::SetMuted(_)
                | Cmd::JumpTo(_)
                | Cmd::PlayList { .. }
        );
        let mut s = self.state.lk();
        if pressed && sleep_fading(s.sleep, Instant::now()) {
            s.sleep = Sleep::Off;
        }
    }

    /// The sleep timer's part of the loop: step the volume down during its fade, and pause
    /// when it's up. Returns true when it just paused the music.
    fn sleep_tick(&mut self) -> bool {
        let now = Instant::now();
        let (sleep, playing) = {
            let s = self.state.lk();
            (s.sleep, s.playing)
        };
        let Sleep::At(t) = sleep else { return false };
        if now >= t {
            self.state.lk().sleep = Sleep::Off;
            if playing {
                self.handle(Cmd::Pause);
            }
            return true;
        }
        if playing && sleep_fading(sleep, now) {
            self.fade_to(self.want_gain(), SLEEP_TICK.as_secs_f32() * 1.25);
        }
        false
    }

    /// Glide the loaded song's volume (doesn't wait).
    fn fade_to(&self, target: f32, secs: f32) {
        if let Some(f) = &self.fade {
            f.go(target, secs);
        }
    }

    /// Fade to silence and wait until it's done, so the next step (pause, stop, seek, another
    /// song) doesn't cut the sound off with a click. Only waits while sound is coming out.
    fn fade_out(&self, secs: f32) {
        let (Some(p), Some(f)) = (self.player(), &self.fade) else { return };
        if p.is_paused() || p.empty() || f.level() <= 0.002 {
            return;
        }
        f.go(0.0, secs);
        let until = Instant::now() + Duration::from_secs_f32(secs + 0.35);
        while f.level() > 0.002 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(4));
        }
    }

    /// Tell the window right away (before a fade that takes a moment).
    fn tell_ui(&self) {
        self.state.lk().version += 1;
        (self.notify)();
    }

    /// Ask the app to fetch a streamed song (it answers with `Cmd::Fetched`).
    fn want(&self, id: u64) {
        let f = self.fetcher.lk().clone();
        if let Some(f) = f {
            f(id);
        }
    }

    /// While a song plays, get the next one ready if it plays from its site.
    fn prefetch_next(&self) {
        let next = {
            let s = self.state.lk();
            s.queue.get(s.index + 1).filter(|q| q.stream && !q.path.is_file()).map(|q| q.id)
        };
        if let Some(id) = next {
            self.want(id);
        }
    }

    fn run(&mut self, rx: Receiver<Cmd>) {
        loop {
            let sleep = sleep_wake(self.state.lk().sleep, Instant::now());
            let cmd = match [self.teardown_at, sleep].into_iter().flatten().min() {
                Some(at) => {
                    let wait = at.saturating_duration_since(Instant::now());
                    match rx.recv_timeout(wait) {
                        Ok(c) => Some(c),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
                None => match rx.recv() {
                    Ok(c) => Some(c),
                    Err(_) => return,
                },
            };
            let mut changed = true;
            match cmd {
                Some(c) => {
                    self.awake(&c);
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.handle(c)));
                    if r.is_err() {
                        // Something went badly wrong decoding; drop the output and carry on.
                        if let Ok(mut l) = self.live.lock() {
                            *l = None;
                        }
                        self.sink = None;
                        self.fade = None;
                        if let Ok(mut s) = self.state.lock() {
                            s.playing = false;
                            s.error = Some("Playback hit a problem. Press play to try again.".into());
                        }
                    }
                }
                None => match self.teardown_at {
                    Some(at) if Instant::now() >= at => {
                        // Paused long enough: release the audio device.
                        self.teardown_at = None;
                        self.close_output();
                    }
                    // Only the sleep timer's fade stepping down: nothing for the window.
                    _ => changed = false,
                },
            }
            if self.sleep_tick() {
                changed = true;
            }
            if changed {
                self.state.lk().version += 1;
                (self.notify)();
            }
        }
    }

    fn player(&self) -> Option<Arc<rodio::Player>> {
        self.live.lk().clone()
    }

    fn position(&self) -> f32 {
        match self.player() {
            Some(p) => p.get_pos().as_secs_f32(),
            None => self.state.lk().idle_pos,
        }
    }

    fn close_output(&mut self) {
        let pos = self.position();
        if let Some(p) = self.live.lk().take() {
            p.stop();
        }
        self.fade = None;
        self.gen += 1;
        self.sink = None;
        self.state.lk().idle_pos = pos;
    }

    fn ensure_sink(&mut self) -> bool {
        if self.sink.is_some() {
            return true;
        }
        // Keep ~150 ms of audio queued ahead of the speakers (rodio's default is 50 ms). On a
        // slow laptop a busy moment, like the desktop-switch gesture, can stall everything for
        // longer than 50 ms, and the music skips. Pause and volume react a touch later, which
        // you can't really hear.
        let frames = {
            use rodio::cpal::traits::{DeviceTrait, HostTrait};
            let rate = rodio::cpal::default_host()
                .default_output_device()
                .and_then(|d| d.default_output_config().ok())
                .map(|c| c.sample_rate() as u32)
                .unwrap_or(48_000);
            (rate * 3 / 20) & !1
        };
        let open = |fixed: bool| {
            let tx = self.tx.clone();
            rodio::DeviceSinkBuilder::from_default_device().and_then(|b| {
                let b = if fixed { b.with_buffer_size(rodio::cpal::BufferSize::Fixed(frames)) } else { b };
                b.with_error_callback(move |_e| {
                    let _ = tx.send(Cmd::DeviceError);
                })
                .open_stream()
            })
        };
        // Some drivers refuse a custom buffer size; fall back to the default then.
        let opened = open(true).or_else(|_| open(false));
        match opened {
            Ok(mut s) => {
                s.log_on_drop(false);
                self.sink = Some(s);
                true
            }
            Err(_) => {
                self.state.lk().error =
                    Some("No sound output found. Plug in headphones or speakers, then press play.".into());
                false
            }
        }
    }

    /// A streamed song that isn't here yet: wait for it (`Cmd::Arriving` or `Cmd::Fetched`
    /// starts it), at `at` seconds.
    fn wait_for(&mut self, id: u64, at: f32, autoplay: bool) {
        let mut s = self.state.lk();
        s.waiting = true;
        s.playing = autoplay;
        s.duration = 0.0;
        s.idle_pos = at;
        s.error = None;
        drop(s);
        self.want(id);
    }

    /// Load the current queue item into a fresh player and start at `at` seconds.
    fn load_current(&mut self, at: f32, autoplay: bool) -> bool {
        self.teardown_at = None;
        // Changing songs mid-song: a quick fade instead of a cut.
        self.fade_out(FADE_SKIP);
        if let Some(p) = self.live.lk().take() {
            p.stop();
        }
        self.fade = None;
        self.gen += 1;
        self.growing = None;
        let item = {
            let s = self.state.lk();
            s.queue.get(s.index).cloned()
        };
        let Some(item) = item else {
            let mut s = self.state.lk();
            s.playing = false;
            s.idle_pos = 0.0;
            s.waiting = false;
            return false;
        };
        let mut arrival = None;
        if item.stream && !item.path.is_file() {
            // On its way: from the top, it can play from the part that's there already.
            arrival = self.arriving.get(&item.id).filter(|_| at < 0.5).cloned();
            if arrival.is_none() {
                self.wait_for(item.id, at, autoplay);
                return true;
            }
        }
        self.state.lk().waiting = false;
        if !self.ensure_sink() {
            self.state.lk().playing = false;
            return false;
        }
        let decoder = match &arrival {
            Some(a) => match open_growing(a) {
                Some(d) => {
                    self.growing = Some(a.download.clone());
                    Some(d)
                }
                None => {
                    // It can't be read while it's coming in: wait for the whole file.
                    self.wait_for(item.id, at, autoplay);
                    return true;
                }
            },
            None => open_file(&item.path),
        };
        let Some(mut decoder) = decoder else {
            let name = item
                .path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let mut s = self.state.lk();
            s.error = Some(format!("Couldn't play “{name}”. The file may be damaged or moved."));
            s.playing = false;
            return false;
        };
        let duration = decoder.total_duration().map(|d| d.as_secs_f32()).unwrap_or(0.0);
        let start = (at > 0.5).then(|| Duration::from_secs_f32(at));
        if let Some(t) = start {
            let _ = decoder.try_seek(t);
        }
        let sink = self.sink.as_ref().unwrap();
        // Decoded a few seconds ahead on its own thread; the audio thread only copies samples.
        let cfg = sink.config();
        let ahead = crate::ahead::Ahead::new(decoder, cfg.channel_count(), cfg.sample_rate(), start);
        ahead.wait_ready(Duration::from_millis(400));
        let player = rodio::Player::connect_new(sink.mixer());
        // The volume lives in the fader, which starts silent and glides up: quickly for a song
        // from the top, slower when picking up in the middle.
        let fade = FadeCtl::new(0.0);
        if autoplay {
            fade.go(self.want_gain(), if at > 0.5 { FADE_PLAY } else { FADE_START });
        }
        if !autoplay {
            player.pause();
        }
        // Through the equalizer on the audio thread, after the decoding, so changes are instant.
        let eq = crate::eq::Eq::new(ahead, self.eq.clone());
        player.append(Fader { inner: eq, ctl: fade.clone(), gain: 0.0, at: 0 });
        self.fade = Some(fade);
        let gen = self.gen;
        let tx = self.tx.clone();
        player.append(EmptyCallback::new(Box::new(move || {
            let _ = tx.send(Cmd::TrackEnded(gen));
        })));
        if let Some(t) = start {
            // Already there (see above); this tells the player where it is.
            let _ = player.try_seek(t);
        }
        let player = Arc::new(player);
        *self.live.lk() = Some(player);
        let mut s = self.state.lk();
        s.duration = duration;
        s.playing = autoplay;
        s.idle_pos = at;
        s.error = None;
        drop(s);
        if !autoplay {
            self.teardown_at = Some(Instant::now() + Duration::from_secs(20));
        }
        self.prefetch_next();
        true
    }

    /// Try the current item, and if it can't be played, move forward until one works.
    fn start_current(&mut self, at: f32) {
        let len = self.state.lk().queue.len();
        for _ in 0..len.max(1) {
            if self.load_current(at, true) {
                return;
            }
            let mut s = self.state.lk();
            if s.queue.is_empty() || s.index + 1 >= s.queue.len() {
                s.playing = false;
                return;
            }
            s.index += 1;
        }
    }

    fn shuffle_from_current(&mut self) {
        let mut s = self.state.lk();
        let cur = s.queue.get(s.index).cloned();
        let mut rest: Vec<QItem> = s
            .original
            .iter()
            .filter(|q| Some(q.uid) != cur.as_ref().map(|c| c.uid))
            .cloned()
            .collect();
        // Fisher–Yates with xorshift.
        for i in (1..rest.len()).rev() {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            let j = (self.rng % (i as u64 + 1)) as usize;
            rest.swap(i, j);
        }
        let mut q = Vec::with_capacity(rest.len() + 1);
        if let Some(c) = cur {
            q.push(c);
        }
        q.extend(rest);
        s.queue = q;
        s.index = 0;
    }

    fn unshuffle(&mut self) {
        let mut s = self.state.lk();
        let cur_uid = s.queue.get(s.index).map(|q| q.uid);
        s.queue = s.original.clone();
        s.index = cur_uid
            .and_then(|u| s.queue.iter().position(|q| q.uid == u))
            .unwrap_or(0);
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::PlayList {
                items,
                start,
                shuffle,
            } => {
                {
                    let mut s = self.state.lk();
                    if let Some(sh) = shuffle {
                        s.shuffle = sh;
                    }
                    s.original = items.clone();
                    s.queue = items;
                    s.index = start.min(s.queue.len().saturating_sub(1));
                }
                let shuffled = self.state.lk().shuffle;
                if shuffled {
                    if shuffle == Some(true) {
                        // "Shuffle" button: start from a random song too.
                        let n = self.state.lk().queue.len().max(1);
                        self.rng ^= self.rng << 13;
                        self.rng ^= self.rng >> 7;
                        self.rng ^= self.rng << 17;
                        self.state.lk().index = (self.rng % n as u64) as usize;
                    }
                    self.shuffle_from_current();
                }
                self.start_current(0.0);
            }
            Cmd::Toggle => {
                let playing = self.state.lk().playing;
                self.handle(if playing { Cmd::Pause } else { Cmd::Play });
            }
            Cmd::Play => {
                let has = self.state.lk().current().is_some();
                if !has {
                    return;
                }
                match self.player() {
                    Some(p) if !p.empty() => {
                        if let Some(f) = &self.fade {
                            f.set_now(0.0);
                        }
                        p.play();
                        self.fade_to(self.want_gain(), FADE_PLAY);
                        self.teardown_at = None;
                        self.state.lk().playing = true;
                    }
                    _ => {
                        let at = self.state.lk().idle_pos;
                        self.start_current(at);
                    }
                }
            }
            Cmd::Pause => {
                self.state.lk().playing = false;
                self.tell_ui();
                if let Some(p) = self.player() {
                    self.fade_out(FADE_PAUSE);
                    p.pause();
                    self.state.lk().idle_pos = p.get_pos().as_secs_f32();
                }
                self.teardown_at = Some(Instant::now() + Duration::from_secs(20));
            }
            Cmd::Next => self.advance(false),
            Cmd::Prev => {
                let pos = self.position();
                let (index, repeat, len) = {
                    let s = self.state.lk();
                    (s.index, s.repeat, s.queue.len())
                };
                if len == 0 {
                    return;
                }
                if pos > 3.0 || (index == 0 && repeat != Repeat::All) {
                    self.seek(0.0);
                    let playing = self.state.lk().playing;
                    if !playing {
                        self.handle(Cmd::Play);
                    }
                    return;
                }
                {
                    let mut s = self.state.lk();
                    s.index = if index == 0 { len - 1 } else { index - 1 };
                }
                self.start_current(0.0);
            }
            Cmd::Seek(secs) => self.seek(secs),
            Cmd::SetVolume(v) => {
                let muted = {
                    let mut s = self.state.lk();
                    s.volume = v.clamp(0.0, 1.0);
                    if v > 0.0 {
                        s.muted = false;
                    }
                    s.muted
                };
                self.volume_changed(gain(v, muted));
            }
            Cmd::SetMuted(m) => {
                let v = {
                    let mut s = self.state.lk();
                    s.muted = m;
                    s.volume
                };
                self.volume_changed(gain(v, m));
            }
            Cmd::SetShuffle(on) => {
                let was = self.state.lk().shuffle;
                if was == on {
                    return;
                }
                if on {
                    // Keep `original` in sync with the current order first.
                    {
                        let mut s = self.state.lk();
                        s.original = s.queue.clone();
                        s.shuffle = true;
                    }
                    self.shuffle_from_current();
                } else {
                    self.state.lk().shuffle = false;
                    self.unshuffle();
                }
            }
            Cmd::SetRepeat(r) => self.state.lk().repeat = r,
            Cmd::PlayNext(items) => {
                let mut s = self.state.lk();
                let at = (s.index + 1).min(s.queue.len());
                let cur_uid = s.queue.get(s.index).map(|q| q.uid);
                for (i, it) in items.iter().enumerate() {
                    s.queue.insert(at + i, it.clone());
                }
                let oat = cur_uid
                    .and_then(|u| s.original.iter().position(|q| q.uid == u))
                    .map(|p| p + 1)
                    .unwrap_or(s.original.len());
                for (i, it) in items.into_iter().enumerate() {
                    s.original.insert(oat + i, it);
                }
            }
            Cmd::AddToQueue(items) => {
                // Songs you add go after the current song and the others you added, before the
                // rest of the list (so they don't wait behind a whole album).
                let mut guard = self.state.lk();
                let s = &mut *guard;
                if s.queue.is_empty() {
                    s.queue.extend(items.iter().cloned());
                    s.original.extend(items);
                    return;
                }
                let after_added = |q: &[QItem], from: usize| {
                    let mut at = from.min(q.len());
                    while at < q.len() && q[at].origin == Origin::Added {
                        at += 1;
                    }
                    at
                };
                let at = after_added(&s.queue, s.index + 1);
                let cur_uid = s.queue.get(s.index).map(|q| q.uid);
                for (i, it) in items.iter().enumerate() {
                    s.queue.insert(at + i, it.clone());
                }
                let oat = match cur_uid.and_then(|u| s.original.iter().position(|q| q.uid == u)) {
                    Some(p) => after_added(&s.original, p + 1),
                    None => s.original.len(),
                };
                for (i, it) in items.into_iter().enumerate() {
                    s.original.insert(oat + i, it);
                }
            }
            Cmd::RemoveUid(uid) => {
                let (was_current, empty) = {
                    let mut s = self.state.lk();
                    s.original.retain(|q| q.uid != uid);
                    let Some(i) = s.queue.iter().position(|q| q.uid == uid) else {
                        return;
                    };
                    s.queue.remove(i);
                    let was_current = i == s.index;
                    if i < s.index {
                        s.index -= 1;
                    }
                    if s.index >= s.queue.len() && !s.queue.is_empty() {
                        s.index = s.queue.len() - 1;
                    }
                    (was_current, s.queue.is_empty())
                };
                if was_current {
                    if empty {
                        self.handle(Cmd::Stop);
                    } else {
                        let playing = self.state.lk().playing;
                        if playing {
                            self.start_current(0.0);
                        } else {
                            self.load_current(0.0, false);
                        }
                    }
                }
            }
            Cmd::Move { from, to } => {
                let mut s = self.state.lk();
                if from >= s.queue.len() || to >= s.queue.len() || from == to {
                    return;
                }
                let cur_uid = s.queue.get(s.index).map(|q| q.uid);
                let mut it = s.queue.remove(from);
                // Moved in among songs you added: it counts as one of them now.
                let cur = cur_uid.and_then(|u| s.queue.iter().position(|q| q.uid == u));
                let above = match cur {
                    Some(c) if to > c + 1 => s.queue.get(to - 1).map(|q| q.origin),
                    _ => None,
                };
                let below = s.queue.get(to).map(|q| q.origin);
                let added = Some(Origin::Added);
                let among = match (above, below) {
                    (None, b) => b == added,
                    (a, None) => a == added,
                    (a, b) => a == added && b == added,
                };
                if among {
                    it.origin = Origin::Added;
                }
                s.queue.insert(to, it);
                if let Some(u) = cur_uid {
                    s.index = s.queue.iter().position(|q| q.uid == u).unwrap_or(0);
                }
                if !s.shuffle {
                    s.original = s.queue.clone();
                }
            }
            Cmd::JumpTo(i) => {
                {
                    let mut s = self.state.lk();
                    if i >= s.queue.len() {
                        return;
                    }
                    s.index = i;
                }
                self.start_current(0.0);
            }
            Cmd::ClearUpcoming => {
                let mut s = self.state.lk();
                let keep = (s.index + 1).min(s.queue.len());
                s.queue.truncate(keep);
                let cur_uid = s.queue.get(s.index).map(|q| q.uid);
                s.original.retain(|q| Some(q.uid) == cur_uid);
                if s.original.is_empty() {
                    s.original = s.queue.clone();
                }
            }
            Cmd::Restore {
                queue,
                original,
                index,
                position,
            } => {
                let mut s = self.state.lk();
                s.index = index.min(queue.len().saturating_sub(1));
                s.original = if original.is_empty() { queue.clone() } else { original };
                s.queue = queue;
                s.idle_pos = position;
                s.playing = false;
            }
            Cmd::Stop => {
                self.fade_out(FADE_SKIP);
                if let Some(p) = self.live.lk().take() {
                    p.stop();
                }
                self.fade = None;
                self.gen += 1;
                let mut s = self.state.lk();
                s.playing = false;
                s.idle_pos = 0.0;
                self.teardown_at = Some(Instant::now() + Duration::from_secs(5));
            }
            Cmd::InsertAt { at, items } => {
                let mut s = self.state.lk();
                let empty = s.queue.is_empty();
                let at = at.clamp(if empty { 0 } else { s.index + 1 }, s.queue.len());
                // Keep the pre-shuffle order roughly in step: put them after the song they follow.
                let before_uid = if at > 0 { s.queue.get(at - 1).map(|q| q.uid) } else { None };
                for (i, it) in items.iter().enumerate() {
                    s.queue.insert(at + i, it.clone());
                }
                let oat = before_uid
                    .and_then(|u| s.original.iter().position(|q| q.uid == u))
                    .map(|p| p + 1)
                    .unwrap_or(s.original.len());
                let oat = oat.min(s.original.len());
                for (i, it) in items.into_iter().enumerate() {
                    s.original.insert(oat + i, it);
                }
                if !s.shuffle {
                    s.original = s.queue.clone();
                }
            }
            Cmd::SetAutoplay(on) => self.state.lk().autoplay = on,
            Cmd::Fetched { id, path } => {
                self.arriving.remove(&id);
                let (start, intent, at) = {
                    let mut guard = self.state.lk();
                    let s = &mut *guard;
                    for q in s.queue.iter_mut().chain(s.original.iter_mut()) {
                        if q.id == id && q.stream {
                            q.path = path.clone();
                        }
                    }
                    let cur = s.queue.get(s.index).map(|q| q.id);
                    (s.waiting && cur == Some(id), s.playing, s.idle_pos)
                };
                if start {
                    self.load_current(at, intent);
                }
            }
            Cmd::Arriving { id, arrival } => {
                let (start, intent) = {
                    let s = self.state.lk();
                    let cur = s.queue.get(s.index).map(|q| q.id);
                    (s.waiting && cur == Some(id) && s.idle_pos < 0.5, s.playing)
                };
                self.arriving.insert(id, arrival);
                if start {
                    self.load_current(0.0, intent);
                }
            }
            Cmd::FetchFailed { id } => {
                self.arriving.remove(&id);
                let skip = {
                    let mut s = self.state.lk();
                    let cur = s.queue.get(s.index).map(|q| q.id);
                    let skip = s.waiting && cur == Some(id);
                    if skip {
                        s.waiting = false;
                    }
                    skip
                };
                if skip {
                    // Not `natural`: with repeat-one it would just try the same song again.
                    self.advance(false);
                }
            }
            Cmd::TrackEnded(gen) => {
                if gen == self.gen {
                    let stop = self.state.lk().sleep == Sleep::EndOfSong;
                    if stop {
                        self.state.lk().sleep = Sleep::Off;
                        self.stop_after_song();
                    } else {
                        self.advance(true);
                    }
                }
            }
            Cmd::SetSleep(sleep) => {
                self.state.lk().sleep = sleep;
                // Turned off or pushed back during the fade: the volume comes back up.
                if self.state.lk().playing {
                    self.fade_to(self.want_gain(), FADE_PLAY);
                }
            }
            Cmd::DeviceError => {
                // Headphones unplugged, default device changed, etc. Reopen on the new default.
                let now = Instant::now();
                if self
                    .last_device_error
                    .map(|t| now.duration_since(t) < Duration::from_secs(2))
                    .unwrap_or(false)
                {
                    return;
                }
                self.last_device_error = Some(now);
                let playing = self.state.lk().playing;
                self.close_output();
                if playing {
                    let at = self.state.lk().idle_pos;
                    std::thread::sleep(Duration::from_millis(300));
                    self.start_current(at);
                }
            }
        }
    }

    /// The volume slider or mute: glide there while playing (paused, it applies on play).
    fn volume_changed(&self, g: f32) {
        if let Some(p) = self.player() {
            if !p.is_paused() {
                self.fade_to(g, FADE_VOLUME);
            }
        }
    }

    fn seek(&mut self, secs: f32) {
        let secs = secs.max(0.0);
        if let Some(dl) = self.growing.clone() {
            if self.player().is_some() {
                // Playing from a file that was still downloading: it can't seek. Once the
                // whole file is here, open it again at the new spot; until then, wait there.
                let playing = self.state.lk().playing;
                if dl.done() || secs < 0.5 {
                    // The whole file, or back to the start (which it can read as it comes in).
                    self.load_current(secs, playing);
                } else {
                    self.fade_out(FADE_SEEK);
                    if let Some(p) = self.live.lk().take() {
                        p.stop();
                    }
                    self.fade = None;
                    self.growing = None;
                    self.gen += 1;
                    let id = self.state.lk().current().map(|q| q.id);
                    if let Some(id) = id {
                        self.wait_for(id, secs, playing);
                    }
                }
                return;
            }
        }
        match self.player() {
            Some(p) if !p.empty() => {
                let audible = !p.is_paused();
                if audible {
                    self.fade_out(FADE_SEEK);
                }
                let _ = p.try_seek(Duration::from_secs_f32(secs));
                if audible {
                    self.fade_to(self.want_gain(), FADE_SEEK * 2.0);
                }
                self.state.lk().idle_pos = secs;
            }
            _ => self.state.lk().idle_pos = secs,
        }
    }

    /// Add up to 50 songs that follow the current one in the library (wrapping around).
    fn extend_from_library(&mut self) -> bool {
        let lib = self.library.lk().clone();
        if lib.is_empty() {
            return false;
        }
        let (cur, queued): (Option<u64>, std::collections::HashSet<u64>) = {
            let s = self.state.lk();
            (s.current().map(|q| q.id), s.queue.iter().map(|q| q.id).collect())
        };
        let start = cur
            .and_then(|c| lib.iter().position(|(id, _)| *id == c))
            .map(|p| p + 1)
            .unwrap_or(0);
        let pick = |skip_queued: bool| -> Vec<QItem> {
            let mut out = Vec::new();
            for k in 0..lib.len() {
                let (id, path) = &lib[(start + k) % lib.len()];
                if Some(*id) == cur || (skip_queued && queued.contains(id)) {
                    continue;
                }
                out.push(QItem::new(*id, path.clone()).with_origin(Origin::Library));
                if out.len() >= 50 {
                    break;
                }
            }
            out
        };
        let mut batch = pick(true);
        if batch.is_empty() {
            // Everything was already played: go around again.
            batch = pick(false);
        }
        if batch.is_empty() {
            return false;
        }
        if self.state.lk().shuffle {
            for i in (1..batch.len()).rev() {
                self.rng ^= self.rng << 13;
                self.rng ^= self.rng >> 7;
                self.rng ^= self.rng << 17;
                let j = (self.rng % (i as u64 + 1)) as usize;
                batch.swap(i, j);
            }
        }
        let mut s = self.state.lk();
        s.queue.extend(batch.iter().cloned());
        s.original.extend(batch);
        true
    }

    /// Move to the next song. `natural` = the song ended by itself.
    fn advance(&mut self, natural: bool) {
        let (index, len, repeat) = {
            let s = self.state.lk();
            (s.index, s.queue.len(), s.repeat)
        };
        if len == 0 {
            return;
        }
        if natural && repeat == Repeat::One {
            self.start_current(0.0);
            return;
        }
        if index + 1 < len {
            self.state.lk().index = index + 1;
            let playing = self.state.lk().playing;
            if natural || playing {
                self.start_current(0.0);
            } else {
                self.load_current(0.0, false);
            }
        } else if repeat == Repeat::All {
            self.state.lk().index = 0;
            self.start_current(0.0);
        } else if self.state.lk().autoplay && self.extend_from_library() {
            // Queue ran out: keep going with what comes next in the library.
            self.state.lk().index = index + 1;
            let playing = self.state.lk().playing;
            if natural || playing {
                self.start_current(0.0);
            } else {
                self.load_current(0.0, false);
            }
        } else {
            self.stop_and_rewind();
        }
    }

    /// End of the queue: stop and rewind the last song, like other players do.
    fn stop_and_rewind(&mut self) {
        self.fade_out(FADE_SKIP);
        if let Some(p) = self.live.lk().take() {
            p.stop();
        }
        self.fade = None;
        self.gen += 1;
        let mut s = self.state.lk();
        s.playing = false;
        s.idle_pos = 0.0;
        drop(s);
        self.teardown_at = Some(Instant::now() + Duration::from_secs(5));
    }

    /// The sleep timer's "End of song": the song finished, so stop, with the next one ready
    /// to go when you press play.
    fn stop_after_song(&mut self) {
        let (index, len, repeat) = {
            let mut s = self.state.lk();
            s.playing = false;
            (s.index, s.queue.len(), s.repeat)
        };
        let next = match repeat {
            Repeat::One => Some(index),
            _ if index + 1 < len => Some(index + 1),
            Repeat::All if len > 0 => Some(0),
            _ => None,
        };
        match next {
            Some(i) => {
                self.state.lk().index = i;
                self.load_current(0.0, false);
            }
            None => self.stop_and_rewind(),
        }
    }
}

/// A song file, ready to decode (None if it can't be read).
fn open_file(path: &std::path::Path) -> Option<Box<dyn Source + Send>> {
    let f = File::open(path).ok()?;
    let len = f.metadata().map(|m| m.len()).ok();
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut b = rodio::Decoder::builder().with_data(BufReader::new(f)).with_seekable(true).with_gapless(true);
    if let Some(l) = len {
        b = b.with_byte_len(l);
    }
    if !ext.is_empty() {
        b = b.with_hint(&ext);
    }
    Some(Box::new(b.build().ok()?))
}

/// A song that's still downloading, read from the start as it comes in. It can't seek, so
/// the decoder reads it straight through.
fn open_growing(a: &crate::growing::Arrival) -> Option<Box<dyn Source + Send>> {
    let (file, patient) = crate::growing::Growing::open(a).ok()?;
    let d = rodio::Decoder::builder()
        .with_data(BufReader::new(file))
        .with_seekable(false)
        .with_gapless(true)
        .with_hint(&a.ext)
        .build()
        .ok()?;
    patient.store(true, Ordering::Relaxed);
    Some(Box::new(d))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleep_timer_fades_in_its_last_30_seconds() {
        let now = Instant::now();
        let at = |secs: u64| Sleep::At(now + Duration::from_secs(secs));
        // Far off: full volume, and the engine sleeps until the fade starts.
        assert_eq!(sleep_share(at(600), now), 1.0);
        assert!(!sleep_fading(at(600), now));
        assert_eq!(sleep_wake(at(600), now), Some(now + Duration::from_secs(570)));
        // Halfway through the fade: a quarter of the volume (squared), and it wakes up often.
        assert!((sleep_share(at(15), now) - 0.25).abs() < 1e-3);
        assert!(sleep_fading(at(15), now));
        assert_eq!(sleep_wake(at(15), now), Some(now + SLEEP_TICK));
        // The last moment: it wakes exactly when the time is up.
        assert_eq!(sleep_wake(Sleep::At(now + Duration::from_millis(50)), now), Some(now + Duration::from_millis(50)));
        // Up: silent.
        assert_eq!(sleep_share(Sleep::At(now), now), 0.0);
        // Off or "End of song": nothing to do with the volume or the clock.
        for s in [Sleep::Off, Sleep::EndOfSong] {
            assert_eq!(sleep_share(s, now), 1.0);
            assert!(!sleep_fading(s, now));
            assert_eq!(sleep_wake(s, now), None);
        }
    }

    #[test]
    fn wav_files_play() {
        // 16-bit stereo PCM, the plainest WAV there is: 0.1 s of a quiet tone.
        let (rate, frames) = (44_100u32, 4_410u32);
        let data: Vec<u8> = (0..frames)
            .flat_map(|i| {
                let v = ((i as f32 * 0.06).sin() * 8000.0) as i16;
                [v.to_le_bytes(), v.to_le_bytes()].concat()
            })
            .collect();
        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * 4).to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
        wav.extend_from_slice(&data);
        let path = std::env::temp_dir().join(format!("segno-wav-{}.wav", std::process::id()));
        std::fs::write(&path, wav).unwrap();
        // It needs the PCM decoder (rodio's "symphonia-pcm"), which WAV and AIFF both use.
        let song = open_file(&path).expect("a WAV file opens");
        assert_eq!(song.count(), (frames * 2) as usize);
        let _ = std::fs::remove_file(path);
    }
}
