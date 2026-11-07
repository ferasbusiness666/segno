//! YouTube Music: search songs, a song's details, and its lyrics, timed when YouTube Music has
//! them. The same requests YouTube Music's own apps make (no account).
//!
//! Its songs are the official album audio, so their names are clean and lyrics timed for them fit
//! exactly. Answers are read loosely, by looking for the parts by name anywhere in the answer, so
//! small changes in their layout don't break it. Checked against real answers in
//! `tests/data/sources` (from the "Lyrics sources check" workflow).

use serde_json::{json, Value};

const API: &str = "https://music.youtube.com/youtubei/v1/";
/// YouTube Music's own "Songs" filter.
const SONGS_ONLY: &str = "EgWKAQIIAWoMEA4QChADEAQQCRAF";
/// Timed lyrics only come to YouTube Music's Android app.
const ANDROID: (&str, &str) = ("ANDROID_MUSIC", "7.21.50");
const SECS: u64 = 10;

/// A song on YouTube Music.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Song {
    pub id: String,
    pub title: String,
    /// All its artists, as YouTube Music writes them ("Daft Punk, Pharrell Williams & Nile Rodgers").
    pub artist: String,
    pub album: String,
    pub duration: f32,
    /// A square cover picture.
    pub thumb: String,
}

/// What YouTube Music knows about one video id.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub duration: f32,
    /// An official song upload (the album audio). Otherwise a music video or anyone's upload,
    /// whose lyrics, if any, aren't timed to it.
    pub song: bool,
    /// What to ask for its lyrics, when it has some.
    pub lyrics: Option<String>,
}

/// A song's words.
#[derive(Clone, Debug, PartialEq)]
pub enum Words {
    /// (start in seconds, line), timed to the song's album audio.
    Timed(Vec<(f32, String)>),
    Plain(String),
}

/// The date as YouTube Music's web app writes it in its version ("20261107").
fn today() -> String {
    let days = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() / 86_400).unwrap_or(0) as i64;
    // Days since 1970-01-01 to a calendar date (Howard Hinnant's method).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}")
}

fn ask(endpoint: &str, client: (&str, &str), body: Value) -> Result<Value, String> {
    let mut body = body;
    body["context"] = json!({ "client": { "clientName": client.0, "clientVersion": client.1, "hl": "en", "gl": "US" } });
    let headers = [("Origin", "https://music.youtube.com"), ("Referer", "https://music.youtube.com/")];
    let text = crate::net::post_json(&format!("{API}{endpoint}?prettyPrint=false"), &body.to_string(), &headers, SECS)?;
    serde_json::from_str(&text).map_err(|_| "YouTube Music sent something unexpected.".to_string())
}

fn web() -> (&'static str, String) {
    ("WEB_REMIX", format!("1.{}.01.00", today()))
}

/// Songs YouTube Music finds for some words, best first.
pub fn search_songs(query: &str) -> Result<Vec<Song>, String> {
    let (name, version) = web();
    let answer = ask("search", (name, &version), json!({ "query": query, "params": SONGS_ONLY }))?;
    Ok(songs_in(&answer))
}

/// What YouTube Music knows about a video id, and where its lyrics are.
pub fn track(id: &str) -> Result<Track, String> {
    let (name, version) = web();
    let body = json!({
        "videoId": id,
        "playlistId": format!("RDAMVM{id}"),
        "isAudioOnly": true,
        "enablePersistentPlaylistPanel": true,
        "tunerSettingValue": "AUTOMIX_SETTING_NORMAL",
    });
    let answer = ask("next", (name, &version), body)?;
    track_in(&answer, id).ok_or_else(|| "YouTube Music didn't know this song.".to_string())
}

/// A song's words (None: YouTube Music has none for it).
pub fn lyrics(browse_id: &str) -> Result<Option<Words>, String> {
    let answer = ask("browse", ANDROID, json!({ "browseId": browse_id }))?;
    Ok(words_in(&answer))
}

// ---------------------------------------------------------------------------------------------
// Reading the answers

/// Every value called `key`, anywhere in `v`, in order.
fn find<'a>(v: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if k == key {
                    out.push(x);
                } else {
                    find(x, key, out);
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| find(x, key, out)),
        _ => {}
    }
}

fn first<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    let mut out = Vec::new();
    find(v, key, &mut out);
    out.into_iter().next()
}

/// Text written as `{"runs": [{"text": …}, …]}` or `{"simpleText": …}`.
fn text(v: &Value) -> String {
    if let Some(s) = v.get("simpleText").and_then(Value::as_str) {
        return s.trim().to_string();
    }
    v.get("runs")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(|x| x.get("text").and_then(Value::as_str)).collect::<String>())
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// "4:09" or "1:02:03" in seconds.
fn clock(s: &str) -> Option<f32> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    if !(2..=3).contains(&parts.len()) || parts.iter().any(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    Some(parts.iter().fold(0.0, |acc, p| acc * 60.0 + p.parse::<f32>().unwrap_or(0.0)))
}

/// The biggest picture, asked for at a size good for covers.
fn cover(v: &Value) -> String {
    let mut all = Vec::new();
    find(v, "thumbnails", &mut all);
    let url = all
        .first()
        .and_then(|t| t.as_array())
        .and_then(|a| a.last())
        .and_then(|t| t.get("url"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    // Google picture links end with their size ("=w120-h120-l90-rj"): ask for a bigger one.
    match url.rfind("=w") {
        Some(i) if url.contains("googleusercontent.com") => format!("{}=w544-h544-l90-rj", &url[..i]),
        _ => url.to_string(),
    }
}

/// The songs in a search answer, once each.
fn songs_in(answer: &Value) -> Vec<Song> {
    let mut rows = Vec::new();
    find(answer, "musicResponsiveListItemRenderer", &mut rows);
    let mut seen = std::collections::HashSet::new();
    rows.into_iter().filter_map(song_from).filter(|s| seen.insert(s.id.clone())).collect()
}

/// One row: the title, then "Artists • Album • 3:45".
fn song_from(r: &Value) -> Option<Song> {
    let id = r
        .get("playlistItemData")
        .and_then(|p| p.get("videoId"))
        .or_else(|| first(r, "watchEndpoint").and_then(|w| w.get("videoId")))
        .and_then(Value::as_str)?
        .to_string();
    let cols: Vec<String> = r
        .get("flexColumns")?
        .as_array()?
        .iter()
        .map(|c| c.get("musicResponsiveListItemFlexColumnRenderer").and_then(|f| f.get("text")).map(text).unwrap_or_default())
        .collect();
    let title = cols.first()?.clone();
    let parts: Vec<&str> = cols.get(1).map(|s| s.split(" • ").collect()).unwrap_or_default();
    let duration = parts.iter().rev().find_map(|p| clock(p))?;
    let rest: Vec<&str> = parts.iter().copied().filter(|p| clock(p).is_none()).collect();
    if title.is_empty() || id.len() != 11 {
        return None;
    }
    Some(Song {
        id,
        title,
        artist: rest.first().map(|s| s.to_string()).unwrap_or_default(),
        album: rest.get(1).map(|s| s.to_string()).unwrap_or_default(),
        duration,
        thumb: cover(r.get("thumbnail").unwrap_or(&Value::Null)),
    })
}

/// The video `id` in a "next" answer: its names, length, kind and lyrics.
fn track_in(answer: &Value, id: &str) -> Option<Track> {
    let mut panels = Vec::new();
    find(answer, "playlistPanelVideoRenderer", &mut panels);
    let p = panels.iter().find(|p| p.get("videoId").and_then(Value::as_str) == Some(id)).or(panels.first())?;
    let byline = p.get("longBylineText").or_else(|| p.get("shortBylineText")).map(text).unwrap_or_default();
    let kind = p.get("navigationEndpoint").and_then(|e| first(e, "musicVideoType")).and_then(Value::as_str).unwrap_or_default();
    let mut tabs = Vec::new();
    find(answer, "tabRenderer", &mut tabs);
    let lyrics = tabs.iter().find_map(|t| {
        let b = t.get("endpoint")?.get("browseEndpoint")?;
        let page = first(b, "pageType").and_then(Value::as_str)?;
        (page == "MUSIC_PAGE_TYPE_TRACK_LYRICS").then(|| b.get("browseId")?.as_str().map(String::from)).flatten()
    });
    Some(Track {
        id: p.get("videoId").and_then(Value::as_str).unwrap_or(id).to_string(),
        title: p.get("title").map(text).unwrap_or_default(),
        artist: byline.split(" • ").next().unwrap_or_default().trim().to_string(),
        duration: p.get("lengthText").map(text).and_then(|t| clock(&t)).unwrap_or(0.0),
        song: kind == "MUSIC_VIDEO_TYPE_ATV",
        lyrics,
    })
}

/// The words in a lyrics answer: timed when every line has its time, plain otherwise.
fn words_in(answer: &Value) -> Option<Words> {
    if let Some(lines) = first(answer, "timedLyricsData").and_then(Value::as_array).filter(|l| !l.is_empty()) {
        let timed: Option<Vec<(f32, String)>> = lines
            .iter()
            .map(|l| {
                let at = l.get("cueRange")?.get("startTimeMilliseconds")?;
                let ms = at.as_str().and_then(|s| s.parse::<f64>().ok()).or_else(|| at.as_f64())?;
                Some((ms as f32 / 1000.0, l.get("lyricLine").and_then(Value::as_str).unwrap_or_default().to_string()))
            })
            .collect();
        return Some(match timed {
            Some(t) => Words::Timed(t),
            None => Words::Plain(lines.iter().filter_map(|l| l.get("lyricLine").and_then(Value::as_str)).collect::<Vec<_>>().join("\n")),
        });
    }
    let shelf = first(answer, "musicDescriptionShelfRenderer")?;
    let plain = shelf.get("description").map(text).unwrap_or_default();
    (!plain.trim().is_empty()).then_some(Words::Plain(plain))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(name: &str) -> Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/sources").join(name);
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn reads_songs_from_a_real_search() {
        let songs = songs_in(&data("ytm-search-songs-getlucky.json"));
        assert!(songs.len() >= 3, "{songs:?}");
        let s = &songs[0];
        assert_eq!(s.id, "4D7u5KF7SP8");
        assert_eq!(s.title, "Get Lucky (feat. Pharrell Williams and Nile Rodgers)");
        assert_eq!(s.artist, "Daft Punk, Pharrell Williams & Nile Rodgers");
        assert_eq!(s.album, "Random Access Memories");
        assert_eq!(s.duration, 370.0);
        assert!(s.thumb.contains("googleusercontent.com") && s.thumb.ends_with("=w544-h544-l90-rj"), "{}", s.thumb);
        // Arabic and several artists.
        let k = &songs_in(&data("ytm-search-songs-shoftkalam.json"))[0];
        assert_eq!((k.title.as_str(), k.artist.as_str(), k.duration), ("Shoft Kalam", "Marwan Pablo, Lege-Cy & HatemBas", 210.0));
    }

    #[test]
    fn tells_songs_from_music_videos_and_finds_the_lyrics() {
        let t = track_in(&data("ytm-next-atv-getlucky.json"), "4D7u5KF7SP8").unwrap();
        assert!(t.song);
        assert_eq!(t.duration, 370.0);
        assert_eq!(t.artist, "Daft Punk, Pharrell Williams & Nile Rodgers");
        assert_eq!(t.lyrics.as_deref(), Some("MPLYt_K8qWMWVqXGi-8"));
        let v = track_in(&data("ytm-next-omv-blindinglights.json"), "4NRXx6U8ABQ").unwrap();
        assert!(!v.song, "a music video");
        assert_eq!((v.title.as_str(), v.artist.as_str(), v.duration), ("Blinding Lights (Official Video)", "The Weeknd", 263.0));
        assert!(v.lyrics.is_some());
    }

    #[test]
    fn reads_timed_untimed_and_missing_lyrics() {
        let Some(Words::Timed(lines)) = words_in(&data("ytm-lyrics-timed-thickofit.json")) else { panic!("timed") };
        assert_eq!(lines[0], (1.11, "I'm in the thick of it, everybody knows".to_string()));
        assert!(lines.windows(2).all(|w| w[0].0 <= w[1].0));
        // A music video's lyrics come without times: plain.
        let Some(Words::Plain(p)) = words_in(&data("ytm-lyrics-untimed-omv-blindinglights.json")) else { panic!("plain") };
        assert!(p.starts_with("Yeah"), "{p}");
        assert_eq!(words_in(&data("ytm-lyrics-none-shoftkalam.json")), None);
    }

    #[test]
    fn small_readers() {
        assert_eq!(clock("4:09"), Some(249.0));
        assert_eq!(clock("1:02:03"), Some(3723.0));
        assert_eq!(clock("1.9B plays"), None);
        assert_eq!(clock("2013"), None);
        assert_eq!(today().len(), 8);
        assert!(today().starts_with("20"));
    }
}
