//! NetEase Cloud Music: a huge library of timed lyrics (Arabic songs too), used as a backup when
//! YouTube Music and LRCLIB have nothing. Its public web API, the way its PC app asks.
//! Unofficial: it can change or block, so a failure here never stops the other sources.

use serde_json::Value;

const SECS: u64 = 8;
const HEADERS: [(&str, &str); 2] = [("Referer", "https://music.163.com/"), ("Cookie", "appver=8.2.30; os=pc")];

/// A song NetEase has.
#[derive(Clone, Debug, PartialEq)]
pub struct Song {
    pub id: u64,
    pub title: String,
    pub artist: String,
    pub duration: f32,
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

/// Songs NetEase finds for some words.
pub fn search(query: &str) -> Result<Vec<Song>, String> {
    let url = format!("https://music.163.com/api/search/get?s={}&type=1&offset=0&limit=8", enc(query));
    let v: Value = serde_json::from_str(&crate::net::get_with(&url, &HEADERS, SECS)?).map_err(|_| "NetEase sent something unexpected.".to_string())?;
    Ok(songs_in(&v))
}

/// A song's timed lyrics as LRC text, without the credit lines NetEase adds at the top (None:
/// it has none).
pub fn lyrics(id: u64) -> Result<Option<String>, String> {
    let url = format!("https://music.163.com/api/song/lyric?id={id}&lv=1&kv=1&tv=-1");
    let v: Value = serde_json::from_str(&crate::net::get_with(&url, &HEADERS, SECS)?).map_err(|_| "NetEase sent something unexpected.".to_string())?;
    Ok(lrc_in(&v))
}

fn songs_in(v: &Value) -> Vec<Song> {
    let Some(songs) = v.get("result").and_then(|r| r.get("songs")).and_then(Value::as_array) else { return Vec::new() };
    songs
        .iter()
        .filter_map(|s| {
            let artists = s.get("artists").or_else(|| s.get("ar"))?.as_array()?;
            Some(Song {
                id: s.get("id")?.as_u64()?,
                title: s.get("name")?.as_str()?.to_string(),
                artist: artists.iter().filter_map(|a| a.get("name").and_then(Value::as_str)).collect::<Vec<_>>().join(", "),
                duration: s.get("duration").or_else(|| s.get("dt")).and_then(Value::as_f64).unwrap_or(0.0) as f32 / 1000.0,
            })
        })
        .collect()
}

fn lrc_in(v: &Value) -> Option<String> {
    if v.get("nolyric").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let lrc = v.get("lrc")?.get("lyric")?.as_str()?;
    let kept: Vec<&str> = lrc.lines().filter(|l| !is_credit(l)).collect();
    // Something with a time and words left.
    kept.iter().any(|l| l.starts_with('[') && l.split(']').next_back().is_some_and(|w| !w.trim().is_empty())).then(|| kept.join("\n"))
}

/// "[00:00.000] 作曲 : Name" and the like: who wrote, produced or mixed it, not words to sing.
/// The label before the colon is Chinese; sung lines rarely look like that.
fn is_credit(line: &str) -> bool {
    let words = line.rsplit(']').next().unwrap_or(line).trim();
    let Some((label, _)) = words.split_once([':', '：']) else { return false };
    let label = label.trim();
    !label.is_empty() && label.chars().count() <= 8 && label.chars().all(|c| ('\u{4e00}'..='\u{9fff}').contains(&c) || c == ' ' || c == '/')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(name: &str) -> Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/sources").join(name);
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn reads_a_real_search_and_lyrics() {
        let songs = songs_in(&data("netease-search-shoftkalam.json"));
        let s = &songs[0];
        assert_eq!((s.id, s.title.as_str(), s.artist.as_str()), (3407079783, "Shoft Kalam", "Marwan Pablo, Lege-Cy, HatemBas"));
        assert!((s.duration - 209.49).abs() < 0.01);
        let lrc = lrc_in(&data("netease-lyric-shoftkalam.json")).unwrap();
        assert!(!lrc.contains("作曲"), "credits are left out");
        assert!(lrc.contains("حاتم، حاتم، بس"));
        let l = crate::lyrics::parse_lrc(&lrc);
        assert!(l.synced && l.lines.len() > 20);
        let lucky = lrc_in(&data("netease-lyric-getlucky.json")).unwrap();
        assert!(!lucky.contains("作曲") && !lucky.contains("制作人"));
    }

    #[test]
    fn credits_are_told_from_words() {
        assert!(is_credit("[00:00.000] 作曲 : Pharrell Williams"));
        assert!(is_credit("[00:01.000] 制作人：The Weeknd"));
        assert!(!is_credit("[00:12.10]Like the legend of the phoenix"));
        assert!(!is_credit("[00:30.00]Note: this isn't a credit"));
        assert!(!is_credit("[00:04.945]حاتم، حاتم، بس"));
        assert_eq!(lrc_in(&serde_json::json!({ "nolyric": true })), None);
    }
}
