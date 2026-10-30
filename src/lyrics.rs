//! Synced lyrics: found on LRCLIB (free, no account) and kept on disk. A `.lrc` file next to
//! a song always wins.

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

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lyrics {
    /// (start second, line), sorted by time. An empty line is a pause.
    pub lines: Vec<(f32, String)>,
}

impl Lyrics {
    /// The line being sung at `t` seconds (None before the first one starts).
    pub fn line_at(&self, t: f32) -> Option<usize> {
        self.lines.partition_point(|(s, _)| *s <= t).checked_sub(1)
    }

    /// When the line after `idx` starts (None after the last one).
    pub fn next_start(&self, idx: Option<usize>) -> Option<f32> {
        self.lines.get(idx.map_or(0, |i| i + 1)).map(|(s, _)| *s)
    }
}

/// What's known about the lyrics of the song playing.
#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Loading,
    Found(Arc<Lyrics>),
    /// No synced lyrics for this song.
    Missing,
    /// Couldn't ask (no internet, the site is busy). Not remembered, so it's asked again.
    Failed,
}

/// A song to find lyrics for.
pub struct Query {
    pub id: u64,
    /// The file, for a `.lrc` beside it (None for songs playing online).
    pub song: Option<PathBuf>,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f32,
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
    Lyrics { lines }
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
// Names: YouTube titles are messy, LRCLIB wants the plain song and artist

/// Words that mark a bracket or a " - …" ending as video talk, not part of the song's name.
fn is_noise(part: &str) -> bool {
    let lower = part.to_lowercase();
    lower.split(|c: char| !c.is_alphanumeric()).any(|w| {
        matches!(w, "official" | "video" | "audio" | "lyric" | "lyrics" | "hd" | "hq" | "4k" | "mv" | "explicit" | "visualizer" | "visualiser")
            || w.starts_with("remaster")
    })
}

/// "Song (Official Video) [HD]" → "Song". Brackets that are part of the name stay.
pub fn clean_title(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut open: Option<(char, char)> = None;
    let mut group = String::new();
    for ch in title.chars() {
        match open {
            None if ch == '(' || ch == '[' => open = Some((ch, if ch == '(' { ')' } else { ']' })),
            None => out.push(ch),
            Some((o, c)) if ch == c => {
                if !is_noise(&group) {
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
    if open.is_some() {
        // A bracket that never closed: keep its words.
        out.push(' ');
        out.push_str(&group);
    }
    let mut out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    // "Song - Remastered 2011", "Song | Official Video".
    for sep in [" - ", " | "] {
        if let Some((a, b)) = out.rsplit_once(sep) {
            if is_noise(b) {
                out.truncate(a.len());
            }
        }
    }
    out
}

/// "Artist - Topic", "ArtistVEVO" → the artist.
pub fn clean_artist(artist: &str) -> String {
    let a = artist.trim();
    let a = a.strip_suffix(" - Topic").unwrap_or(a);
    let cut = a.len().saturating_sub(4);
    match a.get(cut..) {
        Some(end) if cut > 0 && end.eq_ignore_ascii_case("vevo") => a[..cut].trim().to_string(),
        _ => a.to_string(),
    }
}

/// A YouTube-style title "Artist - Song": (song, artist).
fn split_dash(title: &str) -> Option<(&str, &str)> {
    let (a, s) = [" - ", " – ", " — "].iter().find_map(|d| title.split_once(d))?;
    let (a, s) = (a.trim(), s.trim());
    (!a.is_empty() && !s.is_empty()).then_some((s, a))
}

// -------------------------------------------------------------------------------------------
// LRCLIB (https://lrclib.net/docs)

#[derive(Deserialize, Debug)]
struct Entry {
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    instrumental: bool,
    #[serde(default, rename = "syncedLyrics")]
    synced: Option<String>,
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

/// The synced lyrics of the entry closest in length to the song (`dur` 0: length unknown).
fn best_match(entries: Vec<Entry>, dur: f32) -> Option<String> {
    entries
        .into_iter()
        .filter(|e| !e.instrumental)
        .filter_map(|e| {
            let text = e.synced.filter(|s| !s.trim().is_empty())?;
            let off = match e.duration {
                Some(d) if dur > 0.0 => (d as f32 - dur).abs(),
                _ => MAX_DRIFT / 2.0,
            };
            (off <= MAX_DRIFT).then_some((off, text))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, t)| t)
}

/// Ask LRCLIB: the exact song first (both ways round for "Artist - Song" titles), then a
/// search. At most three requests. Returns the raw LRC text.
fn from_lrclib(title: &str, artist: &str, album: &str, dur: f32) -> Result<Option<String>, String> {
    let title = clean_title(title);
    let artist = clean_artist(artist);
    let mut exact = vec![(title.as_str(), artist.as_str(), album)];
    if let Some((s, a)) = split_dash(&title) {
        exact.push((s, a, ""));
    }
    for (t, a, al) in exact {
        let mut url = format!("https://lrclib.net/api/get?track_name={}&artist_name={}", enc(t), enc(a));
        if !al.is_empty() {
            url.push_str(&format!("&album_name={}", enc(al)));
        }
        if (1.0..=3600.0).contains(&dur) {
            url.push_str(&format!("&duration={}", dur.round() as u32));
        }
        if let Some(body) = net::get_text_opt(&url)? {
            if let Some(text) = serde_json::from_str::<Entry>(&body).ok().and_then(|e| best_match(vec![e], dur)) {
                return Ok(Some(text));
            }
        }
    }
    let q = match split_dash(&title) {
        Some((s, a)) => format!("{a} {s}"),
        None => format!("{artist} {title}"),
    };
    let url = format!("https://lrclib.net/api/search?q={}", enc(q.trim()));
    let list = match net::get_text_opt(&url)? {
        Some(body) => serde_json::from_str::<Vec<Entry>>(&body).unwrap_or_default(),
        None => Vec::new(),
    };
    Ok(best_match(list, dur))
}

// -------------------------------------------------------------------------------------------
// Disk

fn dir() -> PathBuf {
    let d = store::cache_dir().join("lyrics");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn cache_name(title: &str, artist: &str, dur: f32) -> String {
    let id = format!("{}\n{}\n{}", clean_title(title).to_lowercase(), clean_artist(artist).to_lowercase(), dur.round() as i64);
    format!("{:016x}", store::hash_bytes(id.as_bytes()))
}

fn younger_than(p: &Path, days: u64) -> bool {
    p.metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age < Duration::from_secs(days * 86400))
}

fn parsed(text: &str) -> Option<Arc<Lyrics>> {
    let l = parse_lrc(text);
    (!l.lines.is_empty()).then(|| Arc::new(l))
}

/// The `.lrc` next to a song, if there is one.
fn sidecar(song: &Path) -> Option<Arc<Lyrics>> {
    let bytes = std::fs::read(song.with_extension("lrc")).ok()?;
    parsed(&String::from_utf8_lossy(&bytes))
}

/// Lyrics for a song: a `.lrc` file beside it, then the saved copy, then LRCLIB.
/// Blocks on the network, so it runs on the worker.
fn load(q: &Query) -> State {
    if let Some(l) = q.song.as_deref().and_then(sidecar) {
        return State::Found(l);
    }
    let name = cache_name(&q.title, &q.artist, q.duration);
    let (hit, miss) = (dir().join(format!("{name}.lrc")), dir().join(format!("{name}.none")));
    if let Some(l) = std::fs::read_to_string(&hit).ok().and_then(|t| parsed(&t)) {
        return State::Found(l);
    }
    if younger_than(&miss, MISS_DAYS) {
        return State::Missing;
    }
    match from_lrclib(&q.title, &q.artist, &q.album, q.duration) {
        Ok(Some(text)) => match parsed(&text) {
            Some(l) => {
                let _ = std::fs::write(&hit, text);
                State::Found(l)
            }
            None => {
                let _ = std::fs::write(&miss, "");
                State::Missing
            }
        },
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
    fn cleans_youtube_names() {
        assert_eq!(clean_title("Blinding Lights (Official Video) [HD]"), "Blinding Lights");
        assert_eq!(clean_title("Song (feat. Someone)"), "Song (feat. Someone)");
        assert_eq!(clean_title("Total Eclipse (Eclipse Remix)"), "Total Eclipse (Eclipse Remix)");
        assert_eq!(clean_title("Artist - Song (Lyrics)"), "Artist - Song");
        assert_eq!(clean_title("Odd [Remastered 2011] name"), "Odd name");
        assert_eq!(clean_title("Song - 2011 Remaster"), "Song");
        assert_eq!(clean_title("Song | Official Music Video"), "Song");
        assert_eq!(clean_title("Broken (Live"), "Broken Live");
        assert_eq!(clean_artist("The Weeknd - Topic"), "The Weeknd");
        assert_eq!(clean_artist("AdeleVEVO"), "Adele");
        assert_eq!(clean_artist("VEVO"), "VEVO");
        assert_eq!(clean_artist("Fairuz"), "Fairuz");
        assert_eq!(split_dash("Artist – Song"), Some(("Song", "Artist")));
        assert_eq!(split_dash("No dash here"), None);
    }

    #[test]
    fn encodes_for_urls() {
        assert_eq!(enc("Beyoncé & Jay-Z"), "Beyonc%C3%A9%20%26%20Jay-Z");
    }

    #[test]
    fn picks_the_cut_that_fits() {
        let e = |d: f64, s: Option<&str>, inst: bool| Entry { duration: Some(d), instrumental: inst, synced: s.map(String::from) };
        let list = vec![e(250.0, Some("[00:01.00]long"), false), e(201.0, Some("[00:01.00]right"), false), e(200.0, None, false), e(200.0, Some("[00:01.00]x"), true)];
        assert_eq!(best_match(list, 200.0).as_deref(), Some("[00:01.00]right"));
        assert_eq!(best_match(vec![e(300.0, Some("[00:01.00]no"), false)], 200.0), None);
        // Length unknown (a stream that hasn't loaded): anything synced will do.
        assert!(best_match(vec![e(300.0, Some("[00:01.00]ok"), false)], 0.0).is_some());
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
        let q = Query { id: 1, song: Some(dir.join("a.b.mp3")), title: "x".into(), artist: "y".into(), album: String::new(), duration: 100.0 };
        match load(&q) {
            State::Found(l) => assert_eq!(l.lines, vec![(1.0, "mine".into())]),
            s => panic!("{s:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
