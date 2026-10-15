//! The audio engine. It lives on its own thread and owns the output device, so music keeps
//! playing (and moves on to the next song) even when the window is hidden and the UI is asleep.
//!
//! Battery notes:
//! - The thread blocks on its command channel; it wakes only for commands or a song ending.
//! - After a pause of 20 s the output device is closed, so Windows can power the audio path down.
//!   Pressing play again reopens it and seeks back to where you were.

use crate::store::LockExt;
use crate::store::Repeat;
use rodio::source::EmptyCallback;
use rodio::Source;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

static UID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub struct QItem {
    /// Unique per queue entry, so the same song can be in the queue twice.
    pub uid: u64,
    /// Library track id.
    pub id: u64,
    pub path: PathBuf,
    /// Plays from its site: the file arrives when the app has fetched it (see `Cmd::Fetched`).
    pub stream: bool,
}

impl QItem {
    pub fn new(id: u64, path: PathBuf) -> Self {
        Self {
            uid: UID.fetch_add(1, Ordering::Relaxed),
            id,
            path,
            stream: false,
        }
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
    /// A streamed song couldn't be fetched: if it's the one waiting, move on.
    FetchFailed { id: u64 },
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
    let handle = PlayerHandle {
        tx: tx.clone(),
        state: state.clone(),
        live: live.clone(),
        library: library.clone(),
        fetcher: fetcher.clone(),
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
}

impl Engine {
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
            let cmd = match self.teardown_at {
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
            match cmd {
                Some(c) => {
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.handle(c)));
                    if r.is_err() {
                        // Something went badly wrong decoding; drop the output and carry on.
                        if let Ok(mut l) = self.live.lock() {
                            *l = None;
                        }
                        self.sink = None;
                        if let Ok(mut s) = self.state.lock() {
                            s.playing = false;
                            s.error = Some("Playback hit a problem. Press play to try again.".into());
                        }
                    }
                }
                None => {
                    // Paused long enough: release the audio device.
                    self.teardown_at = None;
                    self.close_output();
                }
            }
            self.state.lk().version += 1;
            (self.notify)();
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

    /// Load the current queue item into a fresh player and start at `at` seconds.
    fn load_current(&mut self, at: f32, autoplay: bool) -> bool {
        self.teardown_at = None;
        if let Some(p) = self.live.lk().take() {
            p.stop();
        }
        self.gen += 1;
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
        if item.stream && !item.path.is_file() {
            // Not here yet: wait for the app to fetch it, then `Cmd::Fetched` starts it.
            let mut s = self.state.lk();
            s.waiting = true;
            s.playing = autoplay;
            s.duration = 0.0;
            s.idle_pos = 0.0;
            s.error = None;
            drop(s);
            self.want(item.id);
            return true;
        }
        self.state.lk().waiting = false;
        if !self.ensure_sink() {
            self.state.lk().playing = false;
            return false;
        }
        let decoder = File::open(&item.path).ok().and_then(|f| {
            let len = f.metadata().map(|m| m.len()).ok();
            let ext = item
                .path
                .extension()
                .map(|e| e.to_string_lossy().to_lowercase())
                .unwrap_or_default();
            let mut b = rodio::Decoder::builder()
                .with_data(BufReader::new(f))
                .with_seekable(true)
                .with_gapless(true);
            if let Some(l) = len {
                b = b.with_byte_len(l);
            }
            if !ext.is_empty() {
                b = b.with_hint(&ext);
            }
            b.build().ok()
        });
        let Some(decoder) = decoder else {
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
        let sink = self.sink.as_ref().unwrap();
        let player = rodio::Player::connect_new(sink.mixer());
        let (vol, muted) = {
            let s = self.state.lk();
            (s.volume, s.muted)
        };
        player.set_volume(gain(vol, muted));
        if !autoplay {
            player.pause();
        }
        player.append(decoder);
        let gen = self.gen;
        let tx = self.tx.clone();
        player.append(EmptyCallback::new(Box::new(move || {
            let _ = tx.send(Cmd::TrackEnded(gen));
        })));
        if at > 0.5 {
            let _ = player.try_seek(Duration::from_secs_f32(at));
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
                        p.play();
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
                if let Some(p) = self.player() {
                    p.pause();
                    self.state.lk().idle_pos = p.get_pos().as_secs_f32();
                }
                self.state.lk().playing = false;
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
                if let Some(p) = self.player() {
                    p.set_volume(gain(v, muted));
                }
            }
            Cmd::SetMuted(m) => {
                let v = {
                    let mut s = self.state.lk();
                    s.muted = m;
                    s.volume
                };
                if let Some(p) = self.player() {
                    p.set_volume(gain(v, m));
                }
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
                let mut s = self.state.lk();
                s.queue.extend(items.iter().cloned());
                s.original.extend(items);
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
                let it = s.queue.remove(from);
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
                if let Some(p) = self.live.lk().take() {
                    p.stop();
                }
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
                let (start, intent) = {
                    let mut guard = self.state.lk();
                    let s = &mut *guard;
                    for q in s.queue.iter_mut().chain(s.original.iter_mut()) {
                        if q.id == id && q.stream {
                            q.path = path.clone();
                        }
                    }
                    let cur = s.queue.get(s.index).map(|q| q.id);
                    (s.waiting && cur == Some(id), s.playing)
                };
                if start {
                    self.load_current(0.0, intent);
                }
            }
            Cmd::FetchFailed { id } => {
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
                    self.advance(true);
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

    fn seek(&mut self, secs: f32) {
        let secs = secs.max(0.0);
        match self.player() {
            Some(p) if !p.empty() => {
                let _ = p.try_seek(Duration::from_secs_f32(secs));
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
                out.push(QItem::new(*id, path.clone()));
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
            // End of the queue: stop and rewind the last song, like other players do.
            if let Some(p) = self.live.lk().take() {
                p.stop();
            }
            self.gen += 1;
            let mut s = self.state.lk();
            s.playing = false;
            s.idle_pos = 0.0;
            drop(s);
            self.teardown_at = Some(Instant::now() + Duration::from_secs(5));
        }
    }
}
