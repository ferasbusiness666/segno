//! Lyrics: found on LRCLIB (free, no account) and kept on disk. A `.lrc` file next to a song
//! always wins. Timed lyrics when there are any, plain lyrics when that's all there is.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde::Deserialize;

use crate::{net, store};

/// How long "nothing found" is remembered before asking again.
const MISS_DAYS: u64 = 7;
/// Saved lyrics not touched for this long are deleted.
const KEEP_DAYS: u64 = 120;
/// A cut of the song more than this many seconds longer or shorter would drift out of sync.
const MAX_DRIFT: f32 = 4.0;
/// Most requests one song may cost (LRCLIB allows about 90 a minute).
const MAX_REQUESTS: usize = 6;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lyrics {
    /// (start second, line), sorted by time. An empty line is a pause (or a gap between
    /// verses in plain lyrics).
    pub lines: Vec<(f32, String)>,
    /// Timed to the song. Plain lyrics have every line at 0.
    pub synced: bool,
}

impl Lyrics {
    /// The line being sung at `t` seconds (None before the first one, and for plain lyrics).
    pub fn line_at(&self, t: f32) -> Option<usize> {
        if !self.synced {
            return None;
        }
        self.lines.partition_point(|(s, _)| *s <= t).checked_sub(1)
    }

    /// When the line after `idx` starts (None after the last one, and for plain lyrics).
    pub fn next_start(&self, idx: Option<usize>) -> Option<f32> {
        if !self.synced {
            return None;
        }
        self.lines.get(idx.map_or(0, |i| i + 1)).map(|(s, _)| *s)
    }

    /// Plain lyrics: one line per line, a single gap between verses.
    pub fn plain(text: &str) -> Lyrics {
        let mut lines: Vec<(f32, String)> = Vec::new();
        for raw in text.lines() {
            let line = raw.split_whitespace().collect::<Vec<_>>().join(" ");
            if !line.is_empty() || lines.last().is_some_and(|l| !l.1.is_empty()) {
                lines.push((0.0, line));
            }
        }
        while lines.last().is_some_and(|l| l.1.is_empty()) {
            lines.pop();
        }
        Lyrics { lines, synced: false }
    }

    /// Back to LRC text, to keep on disk.
    fn to_lrc(&self) -> String {
        self.lines
            .iter()
            .map(|(t, w)| {
                let cs = (t * 100.0).round() as u64;
                format!("[{:02}:{:02}.{:02}]{w}\n", cs / 6000, cs % 6000 / 100, cs % 100)
            })
            .collect()
    }
}

/// What's known about the lyrics of the song playing.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Loading,
    Found(Arc<Lyrics>),
    /// No lyrics for this song.
    Missing,
    /// Couldn't ask (no internet, the site is busy). Not remembered, so it's asked again.
    Failed,
}

/// A song to find lyrics for.
pub struct Query {
    pub id: u64,
    /// The file, for a `.lrc` beside it (None for songs playing online).
    pub song: Option<PathBuf>,
    /// For a song playing from a video site: the video's own title and channel, which say
    /// more than the names cleaned from them. Otherwise the song's title and artist tags.
    pub title: String,
    pub artist: String,
    pub from_video: bool,
    pub duration: f32,
    /// Ask again even if it wasn't found last time ("Try again").
    pub force: bool,
}

/// Start the lyrics worker. Songs are looked up one at a time and only the newest waiting one
/// counts, so skipping through songs doesn't flood the site. `reply` gets each answer.
pub fn worker(reply: impl Fn(u64, State) + Send + 'static) -> Sender<Query> {
    let (tx, rx) = channel::<Query>();
    let spawned = std::thread::Builder::new().name("lyrics".into()).spawn(move || {
        tidy();
        while let Ok(mut q) = rx.recv() {
            while let Ok(newer) = rx.try_recv() {
                q = newer;
            }
            reply(q.id, load(&q));
        }
    });
    if spawned.is_err() {
        eprintln!("lyrics: couldn't start the worker thread");
    }
    tx
}

// -------------------------------------------------------------------------------------------
// LRC

/// Parse LRC text: `[01:23.45] words`. Several stamps may share a line, tags such as
/// `[ar:…]` are skipped, `[offset:±ms]` is honoured and word stamps (`<00:01.20>`) dropped.
pub fn parse_lrc(text: &str) -> Lyrics {
    let mut offset = 0.0f32;
    let mut lines: Vec<(f32, String)> = Vec::new();
    for raw in text.lines() {
        let mut rest = raw.trim_start_matches('\u{feff}').trim();
        let mut stamps: Vec<f32> = Vec::new();
        while let Some(r) = rest.strip_prefix('[') {
            let Some(end) = r.find(']') else { break };
            let tag = &r[..end];
            if let Some(t) = parse_stamp(tag) {
                stamps.push(t);
            } else if let Some(v) = tag.strip_prefix("offset:") {
                offset = v.trim().parse::<f32>().unwrap_or(0.0) / 1000.0;
            } else if !tag.contains(':') {
                // "[Chorus]" and the like are part of the words.
                break;
            }
            rest = r[end + 1..].trim_start();
        }
        if stamps.is_empty() {
            continue;
        }
        let words = strip_word_stamps(rest);
        for s in stamps {
            lines.push(((s - offset).max(0.0), words.clone()));
        }
    }
    lines.sort_by(|a, b| a.0.total_cmp(&b.0));
    // A pause right at the start or end adds nothing.
    let first = lines.iter().position(|l| !l.1.is_empty()).unwrap_or(lines.len());
    lines.drain(..first);
    while lines.last().is_some_and(|l| l.1.is_empty()) {
        lines.pop();
    }
    Lyrics { lines, synced: true }
}

fn parse_stamp(tag: &str) -> Option<f32> {
    let (m, s) = tag.split_once(':')?;
    let digits = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());
    if m.len() > 3 || !digits(m) {
        return None;
    }
    let (whole, frac) = s.split_once('.').unwrap_or((s, "0"));
    if !digits(whole) || !digits(frac) {
        return None;
    }
    Some(m.parse::<f32>().ok()? * 60.0 + s.parse::<f32>().ok()?)
}

fn strip_word_stamps(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('<') {
        let after = &rest[i + 1..];
        match after.find('>') {
            Some(j) if parse_stamp(&after[..j]).is_some() => {
                out.push_str(&rest[..i]);
                rest = &after[j + 1..];
            }
            _ => {
                out.push_str(&rest[..=i]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Break `text` into rows no wider than `max_w`, between words (`width` measures a piece of
/// text). A word too long for a row on its own (or a line with no spaces, as in Chinese or
/// Japanese) is broken between letters.
pub fn wrap_words(text: &str, max_w: f32, width: impl Fn(&str) -> f32) -> Vec<String> {
    let space = width(" ");
    let mut rows: Vec<String> = Vec::new();
    let (mut cur, mut cur_w) = (String::new(), 0.0f32);
    for word in text.split_whitespace() {
        let w = width(word);
        if !cur.is_empty() && cur_w + space + w > max_w {
            rows.push(std::mem::take(&mut cur));
            cur_w = 0.0;
        }
        if w > max_w {
            // On its own rows, a piece at a time.
            if !cur.is_empty() {
                rows.push(std::mem::take(&mut cur));
            }
            let mut piece = String::new();
            for ch in word.chars() {
                piece.push(ch);
                if piece.chars().count() > 1 && width(&piece) > max_w {
                    piece.pop();
                    rows.push(std::mem::replace(&mut piece, ch.to_string()));
                }
            }
            cur_w = width(&piece);
            cur = piece;
            continue;
        }
        if !cur.is_empty() {
            cur.push(' ');
            cur_w += space;
        }
        cur.push_str(word);
        cur_w += w;
    }
    if !cur.is_empty() || rows.is_empty() {
        rows.push(cur);
    }
    rows
}

// -------------------------------------------------------------------------------------------
// Names: video titles are messy, LRCLIB wants the plain song and artist

/// Speed edits: their lyrics are the original's, stretched.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Speed {
    Slower,
    Faster,
}

fn speed_of(title: &str) -> Option<Speed> {
    let t = title.to_lowercase();
    if t.contains("slowed") || t.contains("daycore") {
        Some(Speed::Slower)
    } else if t.contains("sped up") || t.contains("speed up") || t.contains("nightcore") {
        Some(Speed::Faster)
    } else {
        None
    }
}

/// Words that mark a version of a song rather than its name.
fn is_version(part: &str) -> bool {
    let t = part.to_lowercase();
    speed_of(&t).is_some() || t.contains("reverb") || t.contains("remaster") || t.contains("bass boost") || t.split_whitespace().any(|w| w == "8d")
}

/// "Song (Official Video) [Remastered 2011]" → "Song". Speed edits ("(Slowed)") stay: they
/// have lyrics of their own on LRCLIB.
fn clean_title(title: &str) -> String {
    let t = crate::ytdlp::remove_junk(title);
    strip_versions(&t, |p| p.to_lowercase().contains("remaster"))
}

/// The song's name without any version marks: "addiction (Slowed + Reverb)" → "addiction".
fn without_version(title: &str) -> String {
    strip_versions(title, is_version)
}

/// Drop brackets and " - …" endings that `drop` picks out.
fn strip_versions(title: &str, drop: impl Fn(&str) -> bool) -> String {
    let mut out = String::with_capacity(title.len());
    let mut open: Option<(char, char)> = None;
    let mut group = String::new();
    for ch in title.chars() {
        match open {
            None if ch == '(' || ch == '[' => open = Some((ch, if ch == '(' { ')' } else { ']' })),
            None => out.push(ch),
            Some((o, c)) if ch == c => {
                if !drop(&group) {
                    out.push(o);
                    out.push_str(&group);
                    out.push(c);
                }
                group.clear();
                open = None;
            }
            Some(_) => group.push(ch),
        }
    }
    if let Some((o, _)) = open {
        // A bracket that never closed.
        if !drop(&group) {
            out.push(o);
            out.push_str(&group);
        }
    }
    let mut out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some((a, b)) = out.rsplit_once(" - ") {
        if drop(b) {
            out.truncate(a.len());
        }
    }
    out
}

/// "Artist - Topic", "ArtistVEVO" → the artist.
fn clean_artist(artist: &str) -> String {
    let a = artist.trim();
    let a = a.strip_suffix(" - Topic").unwrap_or(a);
    let cut = a.len().saturating_sub(4);
    match a.get(cut..) {
        Some(end) if cut > 0 && end.eq_ignore_ascii_case("vevo") => a[..cut].trim().to_string(),
        _ => a.to_string(),
    }
}

/// What goes between two artists' names: "A x B", "A, B", "A feat. B", "A و B" (Arabic "and").
const ARTIST_SEPS: [&str; 13] = [" x ", " × ", ", ", "; ", " & ", " feat. ", " feat ", " ft. ", " ft ", " featuring ", " vs. ", " / ", " و "];

fn sep_at(b: &[u8], i: usize) -> bool {
    ARTIST_SEPS.iter().any(|s| b.len() - i >= s.len() && b[i..i + s.len()].eq_ignore_ascii_case(s.as_bytes()))
}

/// The first of several artists ("MARWAN PABLO x LEGE-CY" → "MARWAN PABLO").
fn primary_artist(a: &str) -> &str {
    let b = a.as_bytes();
    let cut = (0..b.len()).find(|&i| sep_at(b, i)).unwrap_or(b.len());
    a[..cut].trim()
}

/// "Song feat. X" → "Song".
fn without_guests(s: &str) -> &str {
    const FEAT: [&str; 5] = [" feat. ", " feat ", " ft. ", " ft ", " featuring "];
    let b = s.as_bytes();
    let cut = (0..b.len())
        .find(|&i| FEAT.iter().any(|f| b.len() - i >= f.len() && b[i..i + f.len()].eq_ignore_ascii_case(f.as_bytes())))
        .unwrap_or(b.len());
    s[..cut].trim()
}

/// Looks like a list of artists rather than a song's name.
fn looks_like_artists(s: &str) -> bool {
    let b = s.as_bytes();
    (0..b.len()).any(|i| sep_at(b, i))
}

/// Lowercase letters and digits only: "Lege-Cy" and "LEGE CY" are the same name.
fn key(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Two names for the same artist ("HatemBas" and "HATEM BAS", or one of several).
fn same_artist(found: &str, ours: &str) -> bool {
    let (f, o) = (key(found), key(primary_artist(ours)));
    let f1 = key(primary_artist(found));
    !o.is_empty() && !f1.is_empty() && (f.contains(&o) || o.contains(&f1))
}

/// Two names for the same song, whatever the version or guests: "addiction - Slowed",
/// "addiction (feat. X)" and "Addiction" all match.
fn same_song(found: &str, ours: &str) -> bool {
    let base = |s: &str| {
        let s = without_version(s);
        let s = s.split(['(', '[']).next().unwrap_or("");
        let s = s.split(" - ").next().unwrap_or("");
        key(without_guests(s))
    };
    let (f, o) = (base(found), base(ours));
    !o.is_empty() && f == o
}

/// "Artist - Song" (or "Song - Artist"): the two sides.
fn split_dash(s: &str) -> Option<(&str, &str)> {
    let (a, b) = [" - ", " – ", " — "].iter().find_map(|d| s.split_once(d))?;
    let (a, b) = (a.trim(), b.trim());
    (!a.is_empty() && !b.is_empty()).then_some((a, b))
}

/// The (song, artist) pairs worth asking about, best guess first.
///
/// Video titles come in every shape: "Artist - Song", "Song - Artist" (common in Arabic),
/// "Artist x Artist - Song | the same in Arabic". Each half of a "|" title is tried, the side
/// with several names in it ("A x B", "A و B") is taken as the artist, and the other way round
/// is tried after.
fn name_pairs(title: &str, artist: &str, from_video: bool) -> Vec<(String, String)> {
    let title = clean_title(title);
    let artist = clean_artist(artist);
    let title = title.replace(" ｜ ", " | ");
    let halves: Vec<&str> = title.split(" | ").map(str::trim).filter(|h| !h.is_empty()).collect();
    let artist_like = |s: &str| looks_like_artists(s) || (!artist.is_empty() && key(s) == key(&artist));
    let (mut first, mut then) = (Vec::new(), Vec::new());
    for h in &halves {
        let (p, q) = match split_dash(h) {
            // "Artist - Song" unless the right side is the one that looks like artists.
            Some((l, r)) => {
                if artist_like(r) && !artist_like(l) {
                    (l, r)
                } else {
                    (r, l)
                }
            }
            None if halves.len() > 1 && !artist.is_empty() => {
                if artist_like(h) {
                    (artist.as_str(), *h)
                } else {
                    (*h, artist.as_str())
                }
            }
            None => continue,
        };
        first.push((p.to_string(), q.to_string()));
        then.push((q.to_string(), p.to_string()));
    }
    let given = (title.to_string(), artist.clone());
    // A video's channel is often not the artist, and a "|" title is never one song name; a
    // song's own tags are usually right.
    let trust_given = !from_video && !artist.is_empty() && halves.len() == 1 && first.iter().all(|(s, a)| key(s) != key(&artist) && key(a) != key(&artist));
    let ordered = if trust_given {
        std::iter::once(given).chain(first).chain(then).collect::<Vec<_>>()
    } else {
        first.into_iter().chain(std::iter::once(given)).chain(then).collect()
    };
    let mut out: Vec<(String, String)> = Vec::new();
    for (s, a) in ordered {
        let s = s.trim_matches(|c: char| c == '-' || c == '|' || c.is_whitespace()).to_string();
        if s.is_empty() || out.iter().any(|(s2, a2)| key(s2) == key(&s) && key(a2) == key(&a)) {
            continue;
        }
        out.push((s, a));
    }
    out
}

// -------------------------------------------------------------------------------------------
// LRCLIB (https://lrclib.net/docs)

#[derive(Deserialize, Debug, Default)]
struct Entry {
    #[serde(default, rename = "trackName")]
    track: String,
    #[serde(default, rename = "artistName")]
    artist: String,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    instrumental: bool,
    #[serde(default, rename = "syncedLyrics")]
    synced: Option<String>,
    #[serde(default, rename = "plainLyrics")]
    plain: Option<String>,
}

impl Entry {
    fn synced(&self) -> Option<&str> {
        self.synced.as_deref().filter(|s| !s.trim().is_empty())
    }

    fn plain(&self) -> Option<&str> {
        self.plain.as_deref().filter(|s| !s.trim().is_empty())
    }

    /// How far its length is from `dur` (unknown lengths count as a near miss).
    fn off(&self, dur: f32) -> f32 {
        match self.duration {
            Some(d) if dur > 0.0 => (d as f32 - dur).abs(),
            _ => MAX_DRIFT / 2.0,
        }
    }
}

fn enc(s: &str) -> String {
    let mut o = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

/// The lyrics picked from what LRCLIB sent back.
#[derive(Debug, PartialEq)]
enum Pick {
    /// Timed to this song.
    Synced(String),
    /// The original's timing, stretched by this much for a slowed or sped-up version.
    Stretched(String, f32),
    Plain(String),
}

/// Pick the best lyrics among `entries` for a song of `dur` seconds (0: unknown) that goes
/// by any of `pairs`.
///
/// Timed lyrics of the same length win. A slowed or sped-up song takes the original's timing,
/// stretched to its length (taken from the length most entries agree on). Otherwise plain
/// lyrics of the closest length, but only when the artist matched too: a song name alone
/// ("Addiction") is shared by too many songs.
fn choose(entries: &[Entry], pairs: &[(String, String)], dur: f32, speed: Option<Speed>) -> Option<Pick> {
    let fits = |e: &&Entry| {
        !e.instrumental && pairs.iter().any(|(s, a)| same_song(&e.track, s) && (a.is_empty() || same_artist(&e.artist, a)))
    };
    let artist_known = |e: &&Entry| pairs.iter().any(|(s, a)| !a.is_empty() && same_song(&e.track, s) && same_artist(&e.artist, a));
    let ok: Vec<&Entry> = entries.iter().filter(fits).collect();

    let synced = ok
        .iter()
        .filter_map(|e| Some((e.off(dur), e.synced()?)))
        .filter(|(off, _)| *off <= MAX_DRIFT)
        .min_by(|a, b| a.0.total_cmp(&b.0));
    if let Some((_, text)) = synced {
        return Some(Pick::Synced(text.to_string()));
    }

    if let (Some(speed), true) = (speed, dur > 0.0) {
        // The original's length: the one most entries agree on.
        let originals: Vec<(u32, &str)> = ok
            .iter()
            .filter(|e| speed_of(&e.track).is_none())
            .filter_map(|e| Some((e.duration? as u32, e.synced()?)))
            .collect();
        let mut best: Option<(usize, u32, &str)> = None;
        for &(d, text) in &originals {
            let n = originals.iter().filter(|(d2, _)| d2.abs_diff(d) <= 1).count();
            if best.is_none_or(|(bn, _, _)| n > bn) {
                best = Some((n, d, text));
            }
        }
        if let Some((_, d, text)) = best.filter(|b| b.1 > 0) {
            let ratio = dur / d as f32;
            let fits = match speed {
                Speed::Slower => (1.02..=1.6).contains(&ratio),
                Speed::Faster => (0.6..=0.98).contains(&ratio),
            };
            if fits {
                return Some(Pick::Stretched(text.to_string(), ratio));
            }
        }
    }

    ok.iter()
        .filter(|e| artist_known(e))
        .filter_map(|e| {
            let text = match (e.plain(), e.synced()) {
                (Some(p), _) => p.to_string(),
                (None, Some(s)) => parse_lrc(s).lines.into_iter().map(|(_, w)| w).collect::<Vec<_>>().join("\n"),
                _ => return None,
            };
            Some((e.off(dur), text))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, text)| Pick::Plain(text))
}

/// Ask LRCLIB, most likely names first, and stop at the first timed match. A failed request
/// only counts as an error if nothing was found at all.
fn from_lrclib(q: &Query) -> Result<Option<Lyrics>, String> {
    let pairs = name_pairs(&q.title, &q.artist, q.from_video);
    let Some((song, artist)) = pairs.first().cloned() else { return Ok(None) };
    let dur = q.duration;
    let speed = speed_of(&q.title);
    let mut entries: Vec<Entry> = Vec::new();
    let mut error = None;

    // The exact song: LRCLIB matches the names and a length within 2 seconds.
    let mut urls = Vec::new();
    if !artist.is_empty() {
        let mut u = format!("https://lrclib.net/api/get?track_name={}&artist_name={}", enc(&song), enc(&artist));
        if (1.0..=3600.0).contains(&dur) {
            u.push_str(&format!("&duration={}", dur.round() as u32));
        }
        urls.push((u, false));
    }
    // Then searches: forgiving about the song's name, and only the first artist.
    for (s, a) in &pairs {
        let a = primary_artist(a);
        let mut u = format!("https://lrclib.net/api/search?track_name={}", enc(s));
        if !a.is_empty() {
            u.push_str(&format!("&artist_name={}", enc(a)));
        }
        urls.push((u, true));
    }
    // A slowed or sped-up song: the original too, to stretch its timing.
    if speed.is_some() {
        let base = without_version(&song);
        if !base.is_empty() && base != song {
            let a = primary_artist(&artist);
            urls.insert(2.min(urls.len()), (format!("https://lrclib.net/api/search?track_name={}&artist_name={}", enc(&base), enc(a)), true));
        }
    }

    for (url, list) in urls.into_iter().take(MAX_REQUESTS) {
        match net::get_text_opt(&url) {
            Ok(Some(body)) if list => entries.extend(serde_json::from_str::<Vec<Entry>>(&body).unwrap_or_default()),
            Ok(Some(body)) => entries.extend(serde_json::from_str::<Entry>(&body).ok()),
            Ok(None) => {}
            Err(e) => {
                // Busy (429) or offline: stop asking.
                error = Some(e);
                break;
            }
        }
        if let Some(Pick::Synced(text)) = choose(&entries, &pairs, dur, None) {
            return Ok(Some(parse_lrc(&text)));
        }
    }
    let found = match choose(&entries, &pairs, dur, speed) {
        Some(Pick::Synced(text)) => Some(parse_lrc(&text)),
        Some(Pick::Stretched(text, ratio)) => {
            let mut l = parse_lrc(&text);
            for line in &mut l.lines {
                line.0 *= ratio;
            }
            Some(l)
        }
        Some(Pick::Plain(text)) => Some(Lyrics::plain(&text)),
        None => None,
    };
    match (found, error) {
        (Some(l), _) if !l.lines.is_empty() => Ok(Some(l)),
        (_, Some(e)) => Err(e),
        _ => Ok(None),
    }
}

// -------------------------------------------------------------------------------------------
// Disk

fn dir() -> PathBuf {
    let d = store::cache_dir().join("lyrics");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn cache_name(title: &str, artist: &str, dur: f32) -> String {
    let id = format!("{}\n{}\n{}", key(&clean_title(title)), key(&clean_artist(artist)), dur.round() as i64);
    format!("{:016x}", store::hash_bytes(id.as_bytes()))
}

fn younger_than(p: &Path, days: u64) -> bool {
    p.metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < Duration::from_secs(days * 86400))
}

fn kept(l: Lyrics) -> Option<Arc<Lyrics>> {
    (!l.lines.is_empty()).then(|| Arc::new(l))
}

/// The `.lrc` next to a song, if there is one.
fn sidecar(song: &Path) -> Option<Arc<Lyrics>> {
    let bytes = std::fs::read(song.with_extension("lrc")).ok()?;
    kept(parse_lrc(&String::from_utf8_lossy(&bytes)))
}

/// Lyrics for a song: a `.lrc` file beside it, then the saved copy, then LRCLIB.
/// Blocks on the network, so it runs on the worker.
fn load(q: &Query) -> State {
    if let Some(l) = q.song.as_deref().and_then(sidecar) {
        return State::Found(l);
    }
    let name = cache_name(&q.title, &q.artist, q.duration);
    let d = dir();
    let (timed, plain, miss) = (d.join(format!("{name}.lrc")), d.join(format!("{name}.txt")), d.join(format!("{name}.none")));
    let saved = std::fs::read_to_string(&timed)
        .ok()
        .and_then(|t| kept(parse_lrc(&t)))
        .or_else(|| std::fs::read_to_string(&plain).ok().and_then(|t| kept(Lyrics::plain(&t))));
    if let Some(l) = saved {
        return State::Found(l);
    }
    if !q.force && younger_than(&miss, MISS_DAYS) {
        return State::Missing;
    }
    match from_lrclib(q) {
        Ok(Some(l)) => {
            let _ = if l.synced {
                std::fs::write(&timed, l.to_lrc())
            } else {
                std::fs::write(&plain, l.lines.iter().map(|(_, w)| w.as_str()).collect::<Vec<_>>().join("\n"))
            };
            let _ = std::fs::remove_file(&miss);
            State::Found(Arc::new(l))
        }
        Ok(None) => {
            let _ = std::fs::write(&miss, "");
            State::Missing
        }
        Err(e) => {
            eprintln!("lyrics: {e}");
            State::Failed
        }
    }
}

/// Delete saved lyrics nobody has needed for months, so the folder doesn't grow forever.
fn tidy() {
    let Ok(rd) = std::fs::read_dir(dir()) else { return };
    for e in rd.flatten() {
        if !younger_than(&e.path(), KEEP_DAYS) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stamps_tags_and_repeats() {
        let l = parse_lrc("\u{feff}[ar:Someone]\n[ti:Song]\n[00:01.50]Hello\n[00:10.00][00:30.25]Chorus line\n[01:05]Last\n[01:09.123]\n");
        assert!(l.synced);
        assert_eq!(
            l.lines,
            vec![(1.5, "Hello".into()), (10.0, "Chorus line".into()), (30.25, "Chorus line".into()), (65.0, "Last".into())]
        );
    }

    #[test]
    fn keeps_pauses_but_not_at_the_edges() {
        let l = parse_lrc("[00:00.00]\n[00:05.00]One\n[00:09.00]\n[00:12.00]Two\n[00:20.00]\n");
        assert_eq!(l.lines, vec![(5.0, "One".into()), (9.0, String::new()), (12.0, "Two".into())]);
    }

    #[test]
    fn offset_and_word_stamps() {
        let l = parse_lrc("[offset:500]\n[00:02.00]<00:02.00>Hel<00:02.40>lo <3 you\n");
        assert_eq!(l.lines, vec![(1.5, "Hello <3 you".into())]);
    }

    #[test]
    fn not_lyrics() {
        assert!(parse_lrc("Just some words\nwith no times").lines.is_empty());
        assert!(parse_lrc("[verse]\nwords").lines.is_empty());
        assert!(parse_lrc("[00:1.2.3]bad\n[:10]bad\n[00:.5]bad").lines.is_empty());
        assert_eq!(parse_lrc("[00:04.00][Chorus] la").lines, vec![(4.0, "[Chorus] la".into())]);
    }

    #[test]
    fn plain_lyrics() {
        let l = Lyrics::plain("\n\nFirst  line\nSecond\n\n\n\nThird\n\n");
        assert!(!l.synced);
        let words: Vec<&str> = l.lines.iter().map(|(_, w)| w.as_str()).collect();
        assert_eq!(words, vec!["First line", "Second", "", "Third"]);
        assert_eq!(l.line_at(50.0), None);
        assert_eq!(l.next_start(None), None);
    }

    #[test]
    fn saves_as_lrc_and_back() {
        let l = Lyrics { lines: vec![(59.996, "a".into()), (61.5, "b".into()), (600.0, String::new()), (601.0, "c".into())], synced: true };
        assert_eq!(l.to_lrc(), "[01:00.00]a\n[01:01.50]b\n[10:00.00]\n[10:01.00]c\n");
        assert_eq!(parse_lrc(&l.to_lrc()).lines.len(), 4);
    }

    #[test]
    fn finds_the_line_being_sung() {
        let l = parse_lrc("[00:05.00]a\n[00:10.00]b\n[00:15.00]c");
        assert_eq!(l.line_at(0.0), None);
        assert_eq!(l.line_at(5.0), Some(0));
        assert_eq!(l.line_at(12.0), Some(1));
        assert_eq!(l.line_at(99.0), Some(2));
        assert_eq!(l.next_start(None), Some(5.0));
        assert_eq!(l.next_start(Some(1)), Some(15.0));
        assert_eq!(l.next_start(Some(2)), None);
    }

    #[test]
    fn wraps_between_words() {
        // Every letter 1 wide, a space 1 wide.
        let w = |s: &str| s.chars().count() as f32;
        assert_eq!(wrap_words("one two three four", 9.0, w), vec!["one two", "three", "four"]);
        assert_eq!(wrap_words("short", 9.0, w), vec!["short"]);
        assert_eq!(wrap_words("", 9.0, w), vec![""]);
        // Too long for a row: broken between letters, then carries on.
        assert_eq!(wrap_words("ab 一二三四五六七 cd", 4.0, w), vec!["ab", "一二三四", "五六七", "cd"]);
    }

    #[test]
    fn cleans_names() {
        assert_eq!(clean_title("Blinding Lights (Official Video) [HD]"), "Blinding Lights");
        assert_eq!(clean_title("Song (feat. Someone)"), "Song (feat. Someone)");
        assert_eq!(clean_title("Total Eclipse (Eclipse Remix)"), "Total Eclipse (Eclipse Remix)");
        assert_eq!(clean_title("Song - 2011 Remaster"), "Song");
        assert_eq!(clean_title("addiction (Slowed)"), "addiction (Slowed)");
        assert_eq!(without_version("addiction (Slowed + Reverb)"), "addiction");
        assert_eq!(without_version("addiction - Sped Up"), "addiction");
        assert_eq!(clean_artist("The Weeknd - Topic"), "The Weeknd");
        assert_eq!(clean_artist("AdeleVEVO"), "Adele");
        assert_eq!(clean_artist("VEVO"), "VEVO");
        assert_eq!(primary_artist("MARWAN PABLO x LEGE-CY x HATEM BAS"), "MARWAN PABLO");
        assert_eq!(primary_artist("مروان بابلو و ليجي-سي"), "مروان بابلو");
        assert_eq!(primary_artist("Calvin Harris, Dua Lipa"), "Calvin Harris");
        assert_eq!(primary_artist("Xavier"), "Xavier");
        assert_eq!(speed_of("Song (Slowed + Reverb)"), Some(Speed::Slower));
        assert_eq!(speed_of("Song - sped up"), Some(Speed::Faster));
        assert_eq!(speed_of("Song (Live)"), None);
    }

    #[test]
    fn matches_names_loosely() {
        assert!(same_artist("Marwan Pablo, Lege-Cy, HatemBas", "MARWAN PABLO x LEGE-CY x HATEM BAS"));
        assert!(same_artist("Simon & Garfunkel", "Simon & Garfunkel"));
        assert!(!same_artist("Kanye West", "LONOWN"));
        assert!(same_song("addiction - Slowed", "addiction (Slowed)"));
        assert!(same_song("Shoft Kalam", "SHOFT KALAM"));
        assert!(same_song("Song (feat. X)", "Song"));
        assert!(!same_song("Addictions", "Addiction"));
    }

    #[test]
    fn video_titles_become_song_and_artist() {
        let p = |t: &str, a: &str, v: bool| name_pairs(t, a, v);
        // A "|" title in two languages, song first in both halves.
        let pairs = p("SHOFT KALAM - MARWAN PABLO x LEGE-CY x HATEM BAS | شفت كلام - مروان بابلو و ليجي-سي و حاتم بس", "Marwan Pablo", true);
        assert_eq!(pairs[0], ("SHOFT KALAM".into(), "MARWAN PABLO x LEGE-CY x HATEM BAS".into()));
        assert_eq!(pairs[1], ("شفت كلام".into(), "مروان بابلو و ليجي-سي و حاتم بس".into()));
        // The same song saved with the names mixed up: "SHOFT KALAM" ended up as the artist.
        let pairs = p("MARWAN PABLO x LEGE-CY x HATEM BAS | شفت كلام - مروان بابلو و ليجي-سي و حاتم بس", "SHOFT KALAM", false);
        assert_eq!(pairs[0], ("SHOFT KALAM".into(), "MARWAN PABLO x LEGE-CY x HATEM BAS".into()));
        // A repost channel: the title has the real names.
        let pairs = p("LONOWN - addiction (Official Video)", "Trap Nation", true);
        assert_eq!(pairs[0], ("addiction".into(), "LONOWN".into()));
        // A song with good tags is asked about as it is first.
        let pairs = p("Mr. Brightside - Live", "The Killers", false);
        assert_eq!(pairs[0], ("Mr. Brightside - Live".into(), "The Killers".into()));
        // No dash, no "|": just the title and artist.
        assert_eq!(p("Shape of You", "Ed Sheeran", false), vec![("Shape of You".to_string(), "Ed Sheeran".to_string())]);
    }

    fn entry(track: &str, artist: &str, dur: f64, synced: Option<&str>, plain: Option<&str>) -> Entry {
        Entry { track: track.into(), artist: artist.into(), duration: Some(dur), instrumental: false, synced: synced.map(String::from), plain: plain.map(String::from) }
    }

    #[test]
    fn picks_the_right_lyrics() {
        let pairs = vec![("Shoft Kalam".to_string(), "Marwan Pablo".to_string())];
        let list = vec![
            entry("Shoft Kalam", "Marwan Pablo", 210.0, None, Some("plain")),
            entry("Shoft Kalam", "Marwan Pablo, Lege-Cy, HatemBas", 209.0, Some("[00:01.00]timed"), None),
            entry("Shoft Kalam", "Someone Else", 210.0, Some("[00:01.00]wrong artist"), None),
        ];
        assert_eq!(choose(&list, &pairs, 210.0, None), Some(Pick::Synced("[00:01.00]timed".into())));
        // Another cut of the song: timing would drift, so plain lyrics instead.
        assert_eq!(choose(&list, &pairs, 260.0, None), Some(Pick::Plain("plain".into())));
        // A different song with the same name doesn't count.
        let other = vec![entry("Addiction", "Kanye West", 264.0, Some("[00:01.00]no"), Some("no"))];
        let pairs = vec![("addiction".to_string(), "LONOWN".to_string())];
        assert_eq!(choose(&other, &pairs, 264.0, None), None);
    }

    #[test]
    fn stretches_the_original_for_a_slowed_version() {
        let pairs = vec![("addiction (Slowed)".to_string(), "LONOWN".to_string())];
        let list = vec![
            entry("addiction", "LONOWN", 209.0, Some("[00:10.00]a"), None),
            entry("addiction", "LONOWN", 209.0, Some("[00:10.00]a"), None),
            entry("addiction", "LONOWN", 355.0, Some("[00:10.00]extended"), None),
        ];
        match choose(&list, &pairs, 251.0, Some(Speed::Slower)) {
            Some(Pick::Stretched(text, r)) => {
                assert_eq!(text, "[00:10.00]a");
                assert!((r - 251.0 / 209.0).abs() < 1e-4);
            }
            other => panic!("{other:?}"),
        }
        // Timed lyrics made for the slowed version itself come first.
        let mut with_own = list;
        with_own.push(entry("addiction - Slowed", "LONOWN", 250.0, Some("[00:12.00]own"), None));
        assert_eq!(choose(&with_own, &pairs, 251.0, Some(Speed::Slower)), Some(Pick::Synced("[00:12.00]own".into())));
    }

    #[test]
    fn encodes_for_urls() {
        assert_eq!(enc("Beyoncé & Jay-Z"), "Beyonc%C3%A9%20%26%20Jay-Z");
    }

    #[test]
    fn cache_names_ignore_video_talk() {
        assert_eq!(cache_name("Song (Official Video)", "ArtistVEVO", 200.2), cache_name("song", "artist", 199.8));
        assert_ne!(cache_name("Song", "Artist", 200.0), cache_name("Song", "Artist", 230.0));
    }

    #[test]
    fn sidecar_beats_everything() {
        let dir = std::env::temp_dir().join(format!("segno-lrc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.b.lrc"), "[00:01.00]mine").unwrap();
        let q = Query {
            id: 1,
            song: Some(dir.join("a.b.mp3")),
            title: "x".into(),
            artist: "y".into(),
            from_video: false,
            duration: 100.0,
            force: false,
        };
        match load(&q) {
            State::Found(l) => assert_eq!(l.lines, vec![(1.0, "mine".into())]),
            s => panic!("{s:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
