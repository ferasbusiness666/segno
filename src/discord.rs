//! "Listening to Segno" on Discord: the song, artist, cover and a time bar, for your friends to
//! see. Segno talks to the Discord app on this computer through its local pipe; nothing goes
//! over the internet from Segno. With no Discord app open nothing happens, and Segno looks for
//! it again now and then, so opening Discord later just works.
//!
//! It all runs on its own thread: the window only hands it what's playing.

use std::io::{self, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// Segno's app on Discord (its name there is what shows after "Listening to").
const APP_ID: &str = "1566442057151545384";
const LOGO: &str = "https://segnomusic.pages.dev/icon-512.png";
const SITE: &str = "https://segnomusic.pages.dev";
/// Discord takes about 5 updates every 20 s; one every 4 s at most stays under that.
const GAP: Duration = Duration::from_secs(4);
/// How often to look for the Discord app while it isn't open.
const LOOK_AGAIN: Duration = Duration::from_secs(20);
/// Wait this long after a change before telling Discord, so a few quick skips (or a song
/// that's still loading) only send the song that ends up playing.
const SETTLE: Duration = Duration::from_millis(600);

/// What's playing, as Discord shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Activity {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// A link to the cover (songs from YouTube or Spotify). None: the Segno logo.
    pub cover: Option<String>,
    /// When the song started and when it ends (Unix time in ms), for Discord's time bar.
    pub start_ms: i64,
    pub end_ms: Option<i64>,
}

impl Activity {
    /// The same song at (about) the same place: no need to tell Discord again. Times are
    /// worked out from the position each time something changes, so they wobble a little.
    fn same_as(&self, o: &Activity) -> bool {
        let near = |a: i64, b: i64| (a - b).abs() < 1500;
        self.title == o.title
            && self.artist == o.artist
            && self.album == o.album
            && self.cover == o.cover
            && near(self.start_ms, o.start_ms)
            && match (self.end_ms, o.end_ms) {
                (Some(a), Some(b)) => near(a, b),
                (a, b) => a == b,
            }
    }

    fn to_json(&self) -> Value {
        let mut assets = json!({ "large_image": self.cover.as_deref().unwrap_or(LOGO) });
        if !self.album.is_empty() {
            assets["large_text"] = fit(&self.album).into();
        }
        if self.cover.is_some() {
            // The cover takes the big picture; the logo sits on its corner.
            assets["small_image"] = LOGO.into();
            assets["small_text"] = "Segno".into();
        }
        let mut times = json!({ "start": self.start_ms });
        if let Some(end) = self.end_ms {
            times["end"] = end.into();
        }
        let mut a = json!({
            "type": 2,
            "details": fit(&self.title),
            "timestamps": times,
            "assets": assets,
            "buttons": [{ "label": "Get Segno", "url": SITE }],
            "instance": false,
        });
        if !self.artist.is_empty() {
            a["state"] = fit(&self.artist).into();
        }
        a
    }
}

/// Discord wants 2 to 128 characters in each line.
fn fit(s: &str) -> String {
    let s = s.trim();
    let n = s.chars().count();
    if n > 128 {
        let mut t: String = s.chars().take(127).collect();
        t.push('…');
        t
    } else if n < 2 {
        // A blank that Discord doesn't trim away.
        format!("{s}\u{2800}")
    } else {
        s.to_string()
    }
}

/// The link Discord can show as the cover, if there is one (it takes up to 300 characters).
pub fn cover_link(url: Option<String>) -> Option<String> {
    url.filter(|u| u.starts_with("https://") && u.len() <= 300)
}

pub struct Presence {
    tx: Sender<Option<Activity>>,
    connected: Arc<AtomicBool>,
}

impl Presence {
    pub fn start() -> Self {
        let (tx, rx) = channel();
        let connected = Arc::new(AtomicBool::new(false));
        let c = connected.clone();
        let _ = std::thread::Builder::new().name("discord".into()).spawn(move || run(rx, c, connect));
        Self { tx, connected }
    }

    /// What's playing now (None: nothing, or paused).
    pub fn set(&self, a: Option<Activity>) {
        let _ = self.tx.send(a);
    }

    /// The Discord app has been found.
    pub fn connected(&self) -> bool {
        self.connected.load(Ordering::Relaxed)
    }
}

/// A connection to the Discord app.
trait Pipe: Read + Write + Send {}
impl<T: Read + Write + Send> Pipe for T {}
type Conn = Box<dyn Pipe>;

/// The thread: keep Discord showing what the window last handed over, at Discord's pace.
/// Stops (clearing the status) when the window lets go of it.
fn run(rx: Receiver<Option<Activity>>, connected: Arc<AtomicBool>, connect: fn() -> Option<Conn>) {
    let mut want: Option<Activity> = None;
    let mut conn: Option<Conn> = None;
    // What Discord shows: None until something was sent on this connection.
    let mut shown: Option<Option<Activity>> = None;
    let mut last_send: Option<Instant> = None;
    let mut settled = Instant::now();
    let mut next_look = Instant::now();
    let mut nonce = 0u64;
    loop {
        let now = Instant::now();
        let behind = match (&shown, &want) {
            (Some(Some(a)), Some(b)) => !a.same_as(b),
            (Some(a), b) => a.is_some() != b.is_some(),
            (None, b) => b.is_some(),
        };
        let wake = match &conn {
            None if want.is_some() => Some(next_look),
            Some(_) if behind => Some(last_send.map_or(now, |t| t + GAP).max(settled).max(now)),
            _ => None,
        };
        let got = match wake {
            Some(at) => rx.recv_timeout(at.saturating_duration_since(now)),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match got {
            Ok(mut a) => {
                // Only the newest counts.
                while let Ok(newer) = rx.try_recv() {
                    a = newer;
                }
                let same = match (&a, &want) {
                    (Some(x), Some(y)) => x.same_as(y),
                    (x, y) => x.is_none() && y.is_none(),
                };
                if !same {
                    settled = Instant::now() + SETTLE;
                }
                want = a;
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => {
                if let (Some(c), Some(Some(_))) = (conn.as_mut(), &shown) {
                    nonce += 1;
                    let _ = set_activity(c, None, nonce);
                }
                connected.store(false, Ordering::Relaxed);
                return;
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        let now = Instant::now();
        if conn.is_none() {
            if want.is_some() && now >= next_look {
                conn = connect();
                shown = None;
                last_send = None;
                connected.store(conn.is_some(), Ordering::Relaxed);
                next_look = now + LOOK_AGAIN;
            }
            continue;
        }
        if behind && now >= settled && last_send.is_none_or(|t| now >= t + GAP) {
            nonce += 1;
            let c = conn.as_mut().expect("connected");
            match set_activity(c, want.as_ref(), nonce) {
                Ok(()) => {
                    shown = Some(want.clone());
                    last_send = Some(now);
                }
                Err(_) => {
                    // Discord closed: look for it again in a moment.
                    conn = None;
                    connected.store(false, Ordering::Relaxed);
                    next_look = now + Duration::from_secs(5);
                }
            }
        }
    }
}

// Discord's local protocol: each message is a 4-byte opcode, a 4-byte length (both little
// endian), then that many bytes of JSON.
const HANDSHAKE: u32 = 0;
const FRAME: u32 = 1;
const CLOSE: u32 = 2;
const PING: u32 = 3;
const PONG: u32 = 4;

fn write_frame(w: &mut dyn Pipe, op: u32, body: &Value) -> io::Result<()> {
    let data = serde_json::to_vec(body)?;
    let mut buf = Vec::with_capacity(8 + data.len());
    buf.extend_from_slice(&op.to_le_bytes());
    buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
    buf.extend_from_slice(&data);
    w.write_all(&buf)?;
    w.flush()
}

fn read_frame(r: &mut dyn Pipe) -> io::Result<(u32, Value)> {
    let mut head = [0u8; 8];
    r.read_exact(&mut head)?;
    let op = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
    let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
    if len > 1 << 20 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "message too big"));
    }
    let mut data = vec![0u8; len];
    r.read_exact(&mut data)?;
    Ok((op, serde_json::from_slice(&data).unwrap_or(Value::Null)))
}

/// Say hello; Discord answers with READY when it accepts Segno's app id.
fn handshake(c: &mut dyn Pipe) -> io::Result<()> {
    write_frame(c, HANDSHAKE, &json!({ "v": 1, "client_id": APP_ID }))?;
    match read_frame(c)? {
        (FRAME, v) if v["evt"] == "READY" => Ok(()),
        _ => Err(io::Error::new(io::ErrorKind::ConnectionRefused, "Discord said no")),
    }
}

/// Show `a` (None clears it), then wait for Discord's answer (answering pings on the way).
fn set_activity(c: &mut dyn Pipe, a: Option<&Activity>, nonce: u64) -> io::Result<()> {
    let nonce = nonce.to_string();
    let body = json!({
        "cmd": "SET_ACTIVITY",
        "args": { "pid": std::process::id(), "activity": a.map(Activity::to_json) },
        "nonce": nonce,
    });
    write_frame(c, FRAME, &body)?;
    for _ in 0..8 {
        match read_frame(c)? {
            (FRAME, v) if v["nonce"] == nonce.as_str() => {
                if v["evt"] == "ERROR" {
                    // Discord didn't take it (a bad picture link, say); nothing to retry.
                    eprintln!("Discord status: {}", v["data"]["message"]);
                }
                return Ok(());
            }
            (CLOSE, _) => return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "Discord closed")),
            (PING, v) => write_frame(c, PONG, &v)?,
            _ => {}
        }
    }
    Ok(())
}

/// Find the Discord app: it listens on discord-ipc-0, or the next number up if that's taken.
fn connect() -> Option<Conn> {
    (0..10).find_map(|i| {
        let mut c = open(i)?;
        handshake(&mut *c).ok()?;
        Some(c)
    })
}

#[cfg(windows)]
fn open(i: u32) -> Option<Conn> {
    let f = std::fs::OpenOptions::new().read(true).write(true).open(format!(r"\\.\pipe\discord-ipc-{i}")).ok()?;
    Some(Box::new(f))
}

#[cfg(unix)]
fn open(i: u32) -> Option<Conn> {
    let name = format!("discord-ipc-{i}");
    let base: Vec<std::path::PathBuf> = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
        .iter()
        .filter_map(std::env::var_os)
        .map(Into::into)
        .chain([std::path::PathBuf::from("/tmp")])
        .collect();
    // Discord installed as a Flatpak or Snap keeps its pipe one folder down.
    base.iter()
        .flat_map(|b| [b.join(&name), b.join("app/com.discordapp.Discord").join(&name), b.join("snap.discord").join(&name)])
        .find_map(|p| std::os::unix::net::UnixStream::connect(p).ok())
        .map(|s| Box::new(s) as Conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn song(title: &str, start: i64) -> Activity {
        Activity {
            title: title.into(),
            artist: "KSI".into(),
            album: String::new(),
            cover: Some("https://i.ytimg.com/vi/abcdefghijk/mqdefault.jpg".into()),
            start_ms: start,
            end_ms: Some(start + 180_000),
        }
    }

    #[test]
    fn activity_reads_like_a_song() {
        let v = song("Thick Of It", 1_000).to_json();
        assert_eq!(v["type"], 2);
        assert_eq!(v["details"], "Thick Of It");
        assert_eq!(v["state"], "KSI");
        assert_eq!(v["timestamps"]["end"], 181_000);
        assert_eq!(v["assets"]["small_image"], LOGO);
        assert_eq!(v["buttons"][0]["url"], SITE);
        // A downloaded file with no cover link: the logo, and no empty lines.
        let mut a = song("X", 0);
        a.cover = None;
        a.artist.clear();
        let v = a.to_json();
        assert_eq!(v["assets"]["large_image"], LOGO);
        assert!(v.get("state").is_none());
        assert_eq!(v["details"].as_str().unwrap().chars().count(), 2);
        assert_eq!(fit(&"a".repeat(300)).chars().count(), 128);
        // The same song a moment later isn't news; a seek is.
        assert!(song("A", 10_000).same_as(&song("A", 10_400)));
        assert!(!song("A", 10_000).same_as(&song("A", 40_000)));
        assert!(!song("A", 10_000).same_as(&song("B", 10_000)));
        assert_eq!(cover_link(Some("http://x".into())), None);
    }

    /// A pretend Discord app: answers the handshake and every command, and keeps what Segno sent.
    struct Fake {
        input: Vec<u8>,
        sent: Arc<Mutex<Vec<(u32, Value)>>>,
        replies: std::collections::VecDeque<u8>,
    }

    impl Read for Fake {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = buf.len().min(self.replies.len());
            for b in buf.iter_mut().take(n) {
                *b = self.replies.pop_front().unwrap();
            }
            Ok(n)
        }
    }

    impl Write for Fake {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.input.extend_from_slice(buf);
            while self.input.len() >= 8 {
                let len = u32::from_le_bytes(self.input[4..8].try_into().unwrap()) as usize;
                if self.input.len() < 8 + len {
                    break;
                }
                let op = u32::from_le_bytes(self.input[0..4].try_into().unwrap());
                let v: Value = serde_json::from_slice(&self.input[8..8 + len]).unwrap();
                self.input.drain(..8 + len);
                let reply = if op == HANDSHAKE { json!({ "evt": "READY" }) } else { json!({ "nonce": v["nonce"] }) };
                let data = serde_json::to_vec(&reply).unwrap();
                self.replies.extend(FRAME.to_le_bytes());
                self.replies.extend((data.len() as u32).to_le_bytes());
                self.replies.extend(data);
                self.sent.lock().unwrap().push((op, v));
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    static SENT: Mutex<Option<Arc<Mutex<Vec<(u32, Value)>>>>> = Mutex::new(None);

    fn fake_connect() -> Option<Conn> {
        let sent = SENT.lock().unwrap().clone()?;
        let mut c: Conn = Box::new(Fake { input: Vec::new(), sent, replies: Default::default() });
        handshake(&mut *c).ok()?;
        Some(c)
    }

    #[test]
    fn shows_the_newest_song_then_clears_when_done() {
        let sent = Arc::new(Mutex::new(Vec::new()));
        *SENT.lock().unwrap() = Some(sent.clone());
        let (tx, rx) = channel();
        let connected = Arc::new(AtomicBool::new(false));
        let c = connected.clone();
        let t = std::thread::spawn(move || run(rx, c, fake_connect));
        // Two songs at once: only the newest is shown.
        tx.send(Some(song("First", 0))).unwrap();
        tx.send(Some(song("Second", 0))).unwrap();
        let until = Instant::now() + Duration::from_secs(3);
        while sent.lock().unwrap().len() < 2 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(connected.load(Ordering::Relaxed));
        // The same song a moment later: nothing new to send.
        tx.send(Some(song("Second", 300))).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        drop(tx);
        t.join().unwrap();
        let sent = sent.lock().unwrap();
        let ops: Vec<u32> = sent.iter().map(|(op, _)| *op).collect();
        assert_eq!(ops, [HANDSHAKE, FRAME, FRAME], "{sent:?}");
        assert_eq!(sent[0].1["client_id"], APP_ID);
        assert_eq!(sent[1].1["args"]["activity"]["details"], "Second");
        // Let go of: the status is cleared.
        assert!(sent[2].1["args"]["activity"].is_null());
        assert!(!connected.load(Ordering::Relaxed));
    }
}
