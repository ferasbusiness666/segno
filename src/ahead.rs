//! Songs are decoded a few seconds ahead on their own thread, and the audio thread only copies
//! finished samples out of a ring buffer. Before, songs were decoded inside the audio callback,
//! so when the laptop froze for longer than the output buffer, the sound stuttered.
//!
//! The decoder's output is also converted to the output device's format on that thread, so the
//! audio thread doesn't resample either.
//!
//! Seeking: the audio thread asks (`try_seek` returns at once), the decode thread seeks and
//! leaves a mark saying where in the buffer the new position starts. Until then the audio
//! thread plays silence; then it drops what came before the mark.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rodio::source::{SeekError, UniformSourceIterator};
use rodio::{ChannelCount, Sample, SampleRate, Source};

/// How far ahead the decode thread stays.
const AHEAD_SECS: f32 = 3.0;
/// It tops the buffer up once this much has been played, so it wakes up rarely.
const REFILL_SECS: f32 = 1.0;
/// It decodes in pieces this long, so a seek is noticed quickly.
const PIECE_SECS: f32 = 0.05;
/// `done_at` before the song has ended.
const NOT_DONE: u64 = u64::MAX;

struct Shared {
    /// The newest seek: its number and where to (in µs).
    seek_gen: AtomicU64,
    seek_to: AtomicU64,
    /// How many samples had been decoded in all when the song ended (NOT_DONE until then).
    done_at: AtomicU64,
    /// The audio thread let go: the decode thread stops.
    closed: AtomicBool,
}

/// A song being decoded ahead. Plays like any rodio source.
pub struct Ahead {
    samples: rtrb::Consumer<Sample>,
    /// (seek number, sample count where the new position starts), one per seek.
    marks: rtrb::Consumer<(u64, u64)>,
    shared: Arc<Shared>,
    thread: std::thread::Thread,
    channels: ChannelCount,
    rate: SampleRate,
    duration: Option<Duration>,
    /// Samples taken out of the buffer so far.
    taken: u64,
    /// Where in the frame being played we are (0: at the start of a frame). Silence always
    /// comes in whole frames, so left and right never swap.
    phase: u16,
    /// The frame being played is silence (the decoder is behind, or a seek is on its way).
    quiet: bool,
    /// After a seek: the number of the seek we're waiting to hear back about.
    wait: Option<u64>,
    /// Then: drop samples up to here, where the new position starts.
    skip_to: Option<u64>,
    /// Opened partway into the song: where. The player's first seek there is already done.
    start: Option<Duration>,
    /// The buffer ran dry and the decode thread was woken up (once, not every frame).
    hungry: bool,
}

#[derive(PartialEq)]
enum Frame {
    Music,
    Quiet,
    End,
}

impl Ahead {
    /// Start decoding `src` (already at `start`, if it was opened partway) for an output with
    /// `channels` and `rate`.
    pub fn new<S>(src: S, channels: ChannelCount, rate: SampleRate, start: Option<Duration>) -> Self
    where
        S: Source + Send + 'static,
    {
        let duration = src.total_duration();
        let ch = channels.get() as usize;
        let frames = |secs: f32| ((secs * rate.get() as f32) as usize).max(1) * ch;
        let (tx, samples) = rtrb::RingBuffer::new(frames(AHEAD_SECS));
        let (marks_tx, marks) = rtrb::RingBuffer::new(1024);
        let shared = Arc::new(Shared {
            seek_gen: AtomicU64::new(0),
            seek_to: AtomicU64::new(0),
            done_at: AtomicU64::new(NOT_DONE),
            closed: AtomicBool::new(false),
        });
        let sh = shared.clone();
        let sizes = Sizes { ch, piece: frames(PIECE_SECS), refill: frames(REFILL_SECS), per_sec: frames(1.0) };
        let uniform = UniformSourceIterator::new(OneSpan(src), channels, rate);
        let thread = std::thread::Builder::new()
            .name("decode".into())
            .spawn(move || decode(uniform, tx, marks_tx, sh, sizes))
            .expect("decode thread")
            .thread()
            .clone();
        Ahead {
            samples,
            marks,
            shared,
            thread,
            channels,
            rate,
            duration,
            taken: 0,
            phase: 0,
            quiet: false,
            wait: None,
            skip_to: None,
            start,
            hungry: false,
        }
    }

    /// Wait (at most `limit`) until the first moment of the song is decoded, so it doesn't
    /// start with a gap.
    pub fn wait_ready(&self, limit: Duration) {
        let want = (self.rate.get() as f32 * 0.25) as usize * self.channels.get() as usize;
        let until = Instant::now() + limit;
        while self.samples.slots() < want && self.shared.done_at.load(Ordering::Acquire) == NOT_DONE && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// What the next frame is: music from the buffer, silence, or the end of the song.
    fn next_frame(&mut self) -> Frame {
        if let Some(g) = self.wait {
            while let Ok((mark, at)) = self.marks.pop() {
                if mark == g {
                    self.wait = None;
                    self.skip_to = Some(at);
                }
            }
            if self.wait.is_some() {
                return Frame::Quiet;
            }
        }
        if let Some(at) = self.skip_to {
            let n = (at.saturating_sub(self.taken) as usize).min(self.samples.slots());
            if n > 0 {
                if let Ok(c) = self.samples.read_chunk(n) {
                    c.commit_all();
                }
                self.taken += n as u64;
                // Room again: the decode thread fills it from the new position right away.
                self.thread.unpark();
            }
            if self.taken < at {
                return Frame::Quiet;
            }
            self.skip_to = None;
        }
        if self.samples.slots() >= self.channels.get() as usize {
            self.hungry = false;
            return Frame::Music;
        }
        if self.shared.done_at.load(Ordering::Acquire) <= self.taken {
            return Frame::End;
        }
        // The decoder fell behind (the laptop is very busy, or the song is still downloading):
        // a moment of silence instead of stopping, and a nudge in case it's asleep.
        if !self.hungry {
            self.hungry = true;
            self.thread.unpark();
        }
        Frame::Quiet
    }
}

impl Drop for Ahead {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Release);
        self.thread.unpark();
    }
}

impl Iterator for Ahead {
    type Item = Sample;

    #[inline]
    fn next(&mut self) -> Option<Sample> {
        if self.phase == 0 {
            match self.next_frame() {
                Frame::End => return None,
                f => self.quiet = f == Frame::Quiet,
            }
        }
        let v = if self.quiet {
            0.0
        } else {
            match self.samples.pop() {
                Ok(v) => {
                    self.taken += 1;
                    v
                }
                // Can't happen: whole frames are put in at once.
                Err(_) => 0.0,
            }
        };
        self.phase += 1;
        if self.phase == self.channels.get() {
            self.phase = 0;
        }
        Some(v)
    }
}

impl Source for Ahead {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> ChannelCount {
        self.channels
    }
    fn sample_rate(&self) -> SampleRate {
        self.rate
    }
    fn total_duration(&self) -> Option<Duration> {
        self.duration
    }
    /// Runs on the audio thread, so it only leaves a note for the decode thread.
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        // The player's first seek, to where the song was opened: already there.
        let barely_started = self.taken < (self.rate.get() as u64 * self.channels.get() as u64) / 2;
        if self.start.take() == Some(pos) && barely_started {
            return Ok(());
        }
        let g = self.shared.seek_gen.load(Ordering::Relaxed) + 1;
        self.shared.seek_to.store(pos.as_micros() as u64, Ordering::Release);
        self.shared.seek_gen.store(g, Ordering::Release);
        self.wait = Some(g);
        self.skip_to = None;
        self.thread.unpark();
        Ok(())
    }
}

/// Passes a decoder through as one long span. Some decoders say their first span is empty
/// until they've read a packet, which the converter would take as the end of the song. (A song
/// that changes its sample rate partway is played at its first rate; real songs don't.)
struct OneSpan<S>(S);

impl<S: Source> Iterator for OneSpan<S> {
    type Item = Sample;
    #[inline]
    fn next(&mut self) -> Option<Sample> {
        self.0.next()
    }
}

impl<S: Source> Source for OneSpan<S> {
    fn current_span_len(&self) -> Option<usize> {
        None
    }
    fn channels(&self) -> ChannelCount {
        self.0.channels()
    }
    fn sample_rate(&self) -> SampleRate {
        self.0.sample_rate()
    }
    fn total_duration(&self) -> Option<Duration> {
        self.0.total_duration()
    }
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.0.try_seek(pos)
    }
}

#[derive(Clone, Copy)]
struct Sizes {
    ch: usize,
    piece: usize,
    refill: usize,
    per_sec: usize,
}

/// The decode thread: keep the buffer a few seconds ahead, seek when asked, stop when the
/// audio thread lets go.
fn decode<S: Source>(mut src: S, mut tx: rtrb::Producer<Sample>, mut marks: rtrb::Producer<(u64, u64)>, sh: Arc<Shared>, z: Sizes) {
    crate::platform::raise_thread_priority();
    let mut gen = 0;
    let mut pushed: u64 = 0;
    let mut ended = false;
    let mut filling = true;
    let mut mark: Option<(u64, u64)> = None;
    loop {
        if sh.closed.load(Ordering::Acquire) || tx.is_abandoned() {
            return;
        }
        let g = sh.seek_gen.load(Ordering::Acquire);
        if g != gen {
            gen = g;
            let to = Duration::from_micros(sh.seek_to.load(Ordering::Acquire));
            let _ = src.try_seek(to);
            ended = false;
            filling = true;
            sh.done_at.store(NOT_DONE, Ordering::Release);
            // Everything decoded from here on is from the new position.
            mark = Some((gen, pushed));
        }
        if let Some(m) = mark {
            if marks.push(m).is_ok() {
                mark = None;
            }
        }
        let free = tx.slots();
        if ended || free < z.piece || (!filling && free < z.refill) {
            filling = false;
            // Asleep until there's room to top up (a seek or the end wakes it sooner).
            let room_in = if ended { 1.0 } else { (z.refill.saturating_sub(free)) as f32 / z.per_sec as f32 };
            std::thread::park_timeout(Duration::from_secs_f32(room_in.clamp(0.01, 1.0)));
            continue;
        }
        filling = true;
        let n = z.piece.min(free / z.ch * z.ch);
        let Ok(chunk) = tx.write_chunk_uninit(n) else { continue };
        let k = chunk.fill_from_iter(&mut src);
        pushed += k as u64;
        if k < n {
            // The end of the song: finish the last frame with silence so the channels stay in
            // step, then say where it ends.
            for _ in 0..(z.ch - k % z.ch) % z.ch {
                if tx.push(0.0).is_ok() {
                    pushed += 1;
                }
            }
            ended = true;
            sh.done_at.store(pushed, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::num::NonZero;

    /// A pretend song: stereo, 1000 frames a second, where each sample is its frame number
    /// (positive on the left, negative on the right), so order and channels can be checked.
    struct Song {
        frame: u64,
        len: u64,
        side: u16,
        /// Hold up the decoder for this long on every frame (a busy laptop).
        slow: Duration,
    }

    impl Iterator for Song {
        type Item = Sample;
        fn next(&mut self) -> Option<Sample> {
            if self.frame >= self.len {
                return None;
            }
            let v = if self.side == 0 { self.frame as f32 } else { -(self.frame as f32) };
            self.side += 1;
            if self.side == 2 {
                self.side = 0;
                self.frame += 1;
                if !self.slow.is_zero() {
                    std::thread::sleep(self.slow);
                }
            }
            Some(v)
        }
    }

    impl Source for Song {
        fn current_span_len(&self) -> Option<usize> {
            // Like rodio's MP3 decoder before its first packet.
            Some(if self.frame == 0 && self.side == 0 { 0 } else { 2 })
        }
        fn channels(&self) -> ChannelCount {
            NonZero::new(2).unwrap()
        }
        fn sample_rate(&self) -> SampleRate {
            NonZero::new(1000).unwrap()
        }
        fn total_duration(&self) -> Option<Duration> {
            Some(Duration::from_millis(self.len))
        }
        fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
            self.frame = pos.as_millis() as u64;
            self.side = 0;
            Ok(())
        }
    }

    fn song(len: u64) -> Ahead {
        let a = Ahead::new(Song { frame: 0, len, side: 0, slow: Duration::ZERO }, NonZero::new(2).unwrap(), NonZero::new(1000).unwrap(), None);
        a.wait_ready(Duration::from_secs(2));
        a
    }

    /// The next `n` frames that aren't silence, as their frame numbers (checking left/right).
    fn frames(a: &mut Ahead, n: usize) -> Vec<i64> {
        let mut out = Vec::new();
        let until = Instant::now() + Duration::from_secs(5);
        while out.len() < n && Instant::now() < until {
            let (Some(l), Some(r)) = (a.next(), a.next()) else { break };
            if l == 0.0 && r == 0.0 {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
            assert_eq!(l, -r, "left and right swapped or out of step");
            out.push(l as i64);
        }
        out
    }

    #[test]
    fn plays_everything_in_order_then_ends() {
        let mut a = song(5000);
        let got = frames(&mut a, 6000);
        // Frame 0 is silence by itself (0.0 on both sides), so it counts from 1.
        assert_eq!(got, (1..5000).collect::<Vec<i64>>());
        assert_eq!(a.next(), None);
    }

    #[test]
    fn seeks_land_where_asked() {
        let mut a = song(10_000);
        assert_eq!(frames(&mut a, 10), (1..=10).collect::<Vec<i64>>());
        let t = Instant::now();
        a.try_seek(Duration::from_millis(7000)).unwrap();
        assert_eq!(frames(&mut a, 3), vec![7000, 7001, 7002]);
        assert!(t.elapsed() < Duration::from_millis(100), "a seek took {:?}", t.elapsed());
        // Back, and twice in a row: only the last one counts.
        a.try_seek(Duration::from_millis(100)).unwrap();
        a.try_seek(Duration::from_millis(2500)).unwrap();
        assert_eq!(frames(&mut a, 2), vec![2500, 2501]);
        // Near the end: the last frame, then the end.
        a.try_seek(Duration::from_millis(9999)).unwrap();
        assert_eq!(frames(&mut a, 5), vec![9999]);
        assert_eq!(a.next(), None);
    }

    #[test]
    fn opened_partway_the_first_seek_there_is_free() {
        let mut src = Song { frame: 0, len: 10_000, side: 0, slow: Duration::ZERO };
        src.try_seek(Duration::from_millis(4000)).unwrap();
        let mut a = Ahead::new(src, NonZero::new(2).unwrap(), NonZero::new(1000).unwrap(), Some(Duration::from_millis(4000)));
        a.wait_ready(Duration::from_secs(2));
        a.try_seek(Duration::from_millis(4000)).unwrap();
        // No gap: the very first frame is music.
        let (l, r) = (a.next().unwrap(), a.next().unwrap());
        assert_eq!((l, r), (4000.0, -4000.0));
    }

    #[test]
    fn a_slow_decoder_means_silence_not_a_stop_and_channels_stay_in_step() {
        // Decodes slower than it plays: the player gets silence in whole frames, then music.
        let src = Song { frame: 0, len: 300, side: 0, slow: Duration::from_micros(1500) };
        let mut a = Ahead::new(src, NonZero::new(2).unwrap(), NonZero::new(1000).unwrap(), None);
        let mut got = Vec::new();
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            // Odd-sized reads, like a real output buffer.
            let mut pair = [0.0; 2];
            for p in &mut pair {
                match a.next() {
                    Some(v) => *p = v,
                    None => return assert_eq!(got, (1..300).collect::<Vec<i64>>()),
                }
            }
            assert_eq!(pair[0], -pair[1], "out of step");
            if pair[0] != 0.0 {
                got.push(pair[0] as i64);
            }
        }
        panic!("never ended");
    }

    #[test]
    fn the_decode_thread_stops_when_the_song_is_dropped() {
        let a = song(1_000_000);
        let sh = a.shared.clone();
        drop(a);
        std::thread::sleep(Duration::from_millis(100));
        // Only the test holds it now: the thread let go of its copy.
        assert_eq!(Arc::strong_count(&sh), 1);
    }
}
