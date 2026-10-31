//! YouTube search without starting yt-dlp: the same request YouTube's own page makes, which
//! answers in well under a second. The answer is shaped like yt-dlp's, so it goes through the
//! same reading as before; anything unexpected and the caller uses yt-dlp instead.

use serde_json::{json, Value};

const URL: &str = "https://www.youtube.com/youtubei/v1/search?prettyPrint=false";
const CLIENT_VERSION: &str = "2.20251001.01.00";
/// YouTube's own "Videos only" filter.
const VIDEOS_ONLY: &str = "EgIQAQ%3D%3D";

/// The videos YouTube finds for some words, as a yt-dlp search answer.
pub fn search(query: &str) -> Result<Value, String> {
    let body = json!({
        "context": { "client": { "clientName": "WEB", "clientVersion": CLIENT_VERSION, "hl": "en", "gl": "US" } },
        "query": query,
        "params": VIDEOS_ONLY,
    });
    let headers = [("X-YouTube-Client-Name", "1"), ("X-YouTube-Client-Version", CLIENT_VERSION), ("Origin", "https://www.youtube.com")];
    let text = crate::net::post_json(URL, &body.to_string(), &headers, 8)?;
    let answer: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let entries = videos(&answer);
    if entries.is_empty() {
        return Err("YouTube's answer had no videos Segno could read".into());
    }
    Ok(json!({ "extractor_key": "YoutubeSearch", "entries": entries }))
}

/// The videos in YouTube's answer, in order, once each, like yt-dlp's search entries.
fn videos(answer: &Value) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    walk(answer, &mut out);
    let mut seen = std::collections::HashSet::new();
    out.retain(|e| seen.insert(e["id"].as_str().unwrap_or_default().to_string()));
    out
}

fn walk(v: &Value, out: &mut Vec<Value>) {
    match v {
        Value::Object(m) => {
            if let Some(r) = m.get("videoRenderer") {
                out.extend(entry(r));
            }
            for (k, x) in m {
                if k != "videoRenderer" {
                    walk(x, out);
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
        _ => {}
    }
}

/// One video. None for live streams and premieres (no length), which can't be played as songs.
fn entry(r: &Value) -> Option<Value> {
    let id = r.get("videoId")?.as_str()?;
    let title = text(r.get("title")?)?;
    let channel = ["ownerText", "longBylineText", "shortBylineText"].iter().find_map(|k| r.get(*k).and_then(text)).unwrap_or_default();
    let secs = r.get("lengthText").and_then(text).and_then(|t| clock(&t))?;
    Some(json!({
        "ie_key": "Youtube",
        "id": id,
        "title": title,
        "channel": channel,
        "duration": secs,
        "url": format!("https://www.youtube.com/watch?v={id}"),
    }))
}

/// YouTube's text: `{"simpleText": …}` or `{"runs": [{"text": …}, …]}`.
fn text(v: &Value) -> Option<String> {
    let s = match v.get("simpleText").and_then(Value::as_str) {
        Some(s) => s.to_string(),
        None => v.get("runs")?.as_array()?.iter().filter_map(|r| r.get("text").and_then(Value::as_str)).collect(),
    };
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// "3:45" or "1:02:03" → seconds.
fn clock(t: &str) -> Option<f64> {
    t.split(':').try_fold(0.0, |acc, part| part.trim().parse::<u32>().ok().map(|n| acc * 60.0 + n as f64))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(id: &str, title: &str, by: &str, len: Option<&str>) -> Value {
        let mut r = json!({
            "videoId": id,
            "title": { "runs": [{ "text": title }] },
            "ownerText": { "runs": [{ "text": by, "navigationEndpoint": {} }] },
        });
        if let Some(l) = len {
            r["lengthText"] = json!({ "accessibility": {}, "simpleText": l });
        }
        json!({ "videoRenderer": r })
    }

    #[test]
    fn reads_youtubes_answer() {
        let answer = json!({
            "contents": { "twoColumnSearchResultsRenderer": { "primaryContents": { "sectionListRenderer": { "contents": [
                { "itemSectionRenderer": { "contents": [
                    video("dQw4w9WgXcQ", "Never Gonna Give You Up", "Rick Astley", Some("3:33")),
                    { "reelShelfRenderer": { "items": [{ "reelItemRenderer": { "videoId": "shortsVideo" } }] } },
                    video("liveStream1", "Radio 24/7", "Lofi Girl", None),
                    { "shelfRenderer": { "content": { "verticalListRenderer": { "items": [
                        video("abcdefghijk", "Another Song", "Someone", Some("1:02:03")),
                        video("dQw4w9WgXcQ", "Never Gonna Give You Up", "Rick Astley", Some("3:33")),
                    ] } } } },
                ] } },
                { "continuationItemRenderer": {} }
            ] } } } }
        });
        let v = videos(&answer);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0]["id"], "dQw4w9WgXcQ");
        assert_eq!(v[0]["channel"], "Rick Astley");
        assert_eq!(v[0]["duration"], 213.0);
        assert_eq!(v[1]["duration"], 3723.0);
        // It goes through the same reading as yt-dlp's answers.
        let (items, playlist) = crate::ytdlp::read_search(&json!({ "extractor_key": "YoutubeSearch", "entries": v }));
        assert!(playlist.is_none());
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "Never Gonna Give You Up");
        assert_eq!(items[0].duration, 213.0);
    }

    #[test]
    fn clocks() {
        assert_eq!(clock("3:45"), Some(225.0));
        assert_eq!(clock("1:02:03"), Some(3723.0));
        assert_eq!(clock("LIVE"), None);
    }
}
