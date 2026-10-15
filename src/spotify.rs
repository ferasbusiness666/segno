//! Spotify links. Spotify's audio is locked (DRM), so nothing can download it. Instead the song
//! names are read from Spotify's public embed page, and each song is found on YouTube when it's
//! played or downloaded (see `YtItem::find`).

use crate::ytdlp::{PlaylistInfo, YtItem};
use serde_json::Value;

pub fn is_spotify(url: &str) -> bool {
    let u = url.to_lowercase();
    u.contains("open.spotify.com/") || u.starts_with("spotify:") || u.contains("spotify.link/")
}

/// ("playlist" | "album" | "track", id) from any form of Spotify link.
fn parse_link(url: &str) -> Option<(String, String)> {
    let u = url.trim();
    if let Some(rest) = u.strip_prefix("spotify:") {
        let mut parts = rest.split(':');
        let kind = parts.next()?.to_string();
        let id = parts.next()?.to_string();
        return Some((kind, id));
    }
    let after = u.split("open.spotify.com/").nth(1)?;
    let mut segs = after.split(['/', '?', '#']).filter(|s| !s.is_empty());
    let mut kind = segs.next()?.to_string();
    if kind.starts_with("intl-") || kind == "embed" {
        kind = segs.next()?.to_string();
    }
    let id = segs.next()?.to_string();
    matches!(kind.as_str(), "playlist" | "album" | "track").then_some((kind, id))
}

pub fn resolve(url: &str) -> Result<(Vec<YtItem>, Option<PlaylistInfo>), String> {
    let mut url = url.trim().to_string();
    // Short share links ("spotify.link/…") point to the real page; find it in the page.
    if url.contains("spotify.link/") {
        let page = crate::net::get_page(&url).map_err(|_| unreachable())?;
        url = find_canonical(&page).ok_or("Couldn't open this Spotify link.")?;
    }
    let (kind, id) = parse_link(&url).ok_or("Segno can open Spotify songs, albums and playlists. This link is something else.")?;
    let page = crate::net::get_page(&format!("https://open.spotify.com/embed/{kind}/{id}")).map_err(|_| unreachable())?;
    parse_embed(&page, &kind, &id, &url)
}

fn unreachable() -> String {
    "Couldn't reach Spotify. Check your internet connection.".into()
}

fn find_canonical(page: &str) -> Option<String> {
    let i = page.find("https://open.spotify.com/")?;
    let tail = &page[i..];
    let end = tail.find(['"', '\'', '<', ' ']).unwrap_or(tail.len());
    Some(tail[..end].to_string())
}

/// The JSON Spotify's embed page is built from.
fn page_json(page: &str) -> Option<Value> {
    let marker = "id=\"__NEXT_DATA__\"";
    let i = page.find(marker)?;
    let start = i + page[i..].find('>')? + 1;
    let end = start + page[start..].find("</script>")?;
    serde_json::from_str(&page[start..end]).ok()
}

/// The object describing the song, album or playlist, wherever it sits in the page's JSON.
fn find_entity(v: &Value) -> Option<&Value> {
    if let Some(e) = v.pointer("/props/pageProps/state/data/entity") {
        return Some(e);
    }
    fn walk(v: &Value) -> Option<&Value> {
        match v {
            Value::Object(m) => {
                if m.contains_key("trackList") || (m.contains_key("type") && (m.contains_key("name") || m.contains_key("title")) && m.contains_key("uri")) {
                    return Some(v);
                }
                m.values().find_map(walk)
            }
            Value::Array(a) => a.iter().find_map(walk),
            _ => None,
        }
    }
    walk(v)
}

fn text(v: &Value, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()))
        .unwrap_or("")
        .to_string()
}

/// The largest cover picture listed.
fn cover(v: &Value) -> String {
    let lists = [
        v.pointer("/coverArt/sources"),
        v.pointer("/visualIdentity/image"),
        v.get("images"),
    ];
    for list in lists.into_iter().flatten() {
        if let Some(a) = list.as_array() {
            let best = a.iter().max_by_key(|i| {
                i.get("width").or_else(|| i.get("maxWidth")).and_then(Value::as_u64).unwrap_or(0)
            });
            if let Some(u) = best.and_then(|i| i.get("url")).and_then(Value::as_str) {
                return u.to_string();
            }
        }
    }
    String::new()
}

fn artists(v: &Value) -> String {
    if let Some(a) = v.get("artists").and_then(Value::as_array) {
        let names: Vec<&str> = a.iter().filter_map(|x| x.get("name").and_then(Value::as_str)).collect();
        if !names.is_empty() {
            return names.join(", ");
        }
    }
    text(v, &["subtitle", "artist"])
}

fn ms(v: &Value) -> f32 {
    v.get("duration")
        .and_then(|d| d.as_f64().or_else(|| d.get("totalMilliseconds").and_then(Value::as_f64)))
        .map(|d| (d / 1000.0) as f32)
        .unwrap_or(0.0)
}

fn track_id(v: &Value) -> String {
    let uri = text(v, &["uri"]);
    uri.rsplit(':').next().unwrap_or("").to_string()
}

fn song(id: &str, title: &str, artist: &str, duration: f32, cover: &str, album: &str) -> YtItem {
    // The first artist is enough to find it; extra featured names tend to mislead the search.
    let main = artist.split(',').next().unwrap_or(artist).trim();
    YtItem {
        id: format!("spotify:{id}"),
        title: title.to_string(),
        channel: artist.to_string(),
        duration,
        url: format!("https://open.spotify.com/track/{id}"),
        thumb: cover.to_string(),
        site: "Spotify".into(),
        find: format!("{main} - {title} audio"),
        album: album.to_string(),
    }
}

fn parse_embed(page: &str, kind: &str, id: &str, link: &str) -> Result<(Vec<YtItem>, Option<PlaylistInfo>), String> {
    let json = page_json(page).ok_or("Spotify changed its page, so Segno couldn't read this link.")?;
    let e = find_entity(&json).ok_or("Couldn't read this Spotify link.")?;
    let name = text(e, &["name", "title"]);
    let pic = cover(e);
    if kind == "track" {
        let tid = { let t = track_id(e); if t.is_empty() { id.to_string() } else { t } };
        if name.is_empty() {
            return Err("Couldn't read this Spotify song.".into());
        }
        return Ok((vec![song(&tid, &name, &artists(e), ms(e), &pic, "")], None));
    }
    let list = e.get("trackList").and_then(Value::as_array).ok_or("Couldn't read the songs in this Spotify link.")?;
    // Album tracks share the album's cover; playlist tracks get their YouTube picture.
    let album_cover = if kind == "album" { pic.clone() } else { String::new() };
    let album_name = if kind == "album" { name.clone() } else { String::new() };
    let items: Vec<YtItem> = list
        .iter()
        .filter_map(|t| {
            let title = text(t, &["title", "name"]);
            let tid = track_id(t);
            if title.is_empty() || tid.is_empty() {
                return None;
            }
            Some(song(&tid, &title, &artists(t), ms(t), &album_cover, &album_name))
        })
        .collect();
    if items.is_empty() {
        return Err("This Spotify link has no songs Segno can read.".into());
    }
    let owner = text(e, &["subtitle"]);
    let info = PlaylistInfo {
        title: if name.is_empty() { "Spotify playlist".into() } else { name },
        channel: owner,
        url: link.to_string(),
        cover: pic,
        site: "Spotify".into(),
    };
    Ok((items, Some(info)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(entity: Value) -> String {
        let data = serde_json::json!({"props": {"pageProps": {"state": {"data": {"entity": entity}}}}});
        format!("<html><head></head><body><script id=\"__NEXT_DATA__\" type=\"application/json\">{data}</script></body></html>")
    }

    #[test]
    fn links() {
        assert_eq!(parse_link("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M?si=abc"), Some(("playlist".into(), "37i9dQZF1DXcBWIGoYBM5M".into())));
        assert_eq!(parse_link("https://open.spotify.com/intl-ar/track/4uLU6hMCjMI75M1A2tKUQC"), Some(("track".into(), "4uLU6hMCjMI75M1A2tKUQC".into())));
        assert_eq!(parse_link("spotify:album:1ATL5GLyefJaxhQzSPVrLX"), Some(("album".into(), "1ATL5GLyefJaxhQzSPVrLX".into())));
        assert_eq!(parse_link("https://open.spotify.com/artist/123"), None);
        assert!(is_spotify("https://open.spotify.com/track/x"));
    }

    #[test]
    fn playlist_page() {
        let p = page(serde_json::json!({
            "type": "playlist", "name": "Road Trip", "uri": "spotify:playlist:abc", "subtitle": "Feras",
            "coverArt": {"sources": [{"url": "https://i.scdn.co/small", "width": 60}, {"url": "https://i.scdn.co/big", "width": 640}]},
            "trackList": [
                {"uri": "spotify:track:t1", "title": "Blinding Lights", "subtitle": "The Weeknd", "duration": 200040},
                {"uri": "spotify:track:t2", "title": "Levitating", "subtitle": "Dua Lipa, DaBaby", "duration": 203064},
                {"uri": "", "title": "Broken"}
            ]
        }));
        let (items, info) = parse_embed(&p, "playlist", "abc", "https://open.spotify.com/playlist/abc").unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "spotify:t1");
        assert_eq!(items[0].find, "The Weeknd - Blinding Lights audio");
        assert_eq!(items[1].find, "Dua Lipa - Levitating audio");
        assert!((items[1].duration - 203.064).abs() < 0.01);
        assert!(items[0].thumb.is_empty());
        let info = info.unwrap();
        assert_eq!(info.title, "Road Trip");
        assert_eq!(info.cover, "https://i.scdn.co/big");
    }

    #[test]
    fn track_page() {
        let p = page(serde_json::json!({
            "type": "track", "name": "Blinding Lights", "uri": "spotify:track:t1",
            "artists": [{"name": "The Weeknd"}], "duration": 200040,
            "visualIdentity": {"image": [{"url": "https://i.scdn.co/cover", "maxWidth": 640}]}
        }));
        let (items, info) = parse_embed(&p, "track", "t1", "x").unwrap();
        assert!(info.is_none());
        assert_eq!(items[0].title, "Blinding Lights");
        assert_eq!(items[0].channel, "The Weeknd");
        assert_eq!(items[0].thumb, "https://i.scdn.co/cover");
    }
}
