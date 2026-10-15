//! Songs that play from their site (YouTube, SoundCloud…) without being downloaded: in saved
//! playlists, search results and suggestions. The audio thread asks for each one's file just
//! before it plays (and the next one while the current plays), so they play like files.

use crate::app::{Action, App, Transient};
use crate::player::{Cmd, QItem};
use crate::store::{self, stream_entry, stream_id, stream_key, LockExt};
use crate::ytdlp::{self, YtItem};
use std::collections::HashSet;
use std::path::PathBuf;

impl App {
    /// Remember a song that plays from its site, so lists, the queue and the player bar can
    /// show it. Returns its track id.
    pub(crate) fn register_stream(&mut self, item: YtItem) -> u64 {
        let id = stream_id(&item.id);
        self.stream_reg.lk().insert(id, item.clone());
        let entry = self.transient.entry(id).or_insert(Transient { item: item.clone(), art: None, color: None, path: None });
        // Keep the newest details (a search result can be richer than a saved copy).
        if !item.title.is_empty() {
            entry.item = item;
        }
        id
    }

    /// Every playlist's streamed songs, registered at startup and after a playlist changes.
    pub(crate) fn register_playlist_streams(&mut self) {
        let items: Vec<YtItem> = self.playlists.iter().flat_map(|p| p.streams.values().cloned()).collect();
        for it in items {
            self.register_stream(it);
        }
    }

    /// A queue entry for a streamed song: its cached file if there is one, else a placeholder
    /// the player waits on.
    pub(crate) fn stream_qitem(&self, id: u64) -> Option<QItem> {
        let tr = self.transient.get(&id)?;
        let path = tr
            .path
            .clone()
            .filter(|p| p.is_file())
            .or_else(|| ytdlp::cached(&tr.item))
            .unwrap_or_else(|| stream_entry(&tr.item.id));
        Some(QItem::stream(id, path))
    }

    pub(crate) fn is_stream(&self, id: u64) -> bool {
        self.lib.get(id).is_none() && self.transient.contains_key(&id)
    }

    /// Play one song from its site right away (a search result or a suggestion).
    pub(crate) fn play_stream(&mut self, item: YtItem) {
        let id = self.register_stream(item);
        if let Some(q) = self.stream_qitem(id) {
            self.player.send(Cmd::PlayList { items: vec![q], start: 0, shuffle: None });
        }
    }

    pub(crate) fn on_fetched(&mut self, item: YtItem, path: PathBuf, art: Option<(u64, [u8; 3])>) {
        let id = self.register_stream(item);
        if let Some(tr) = self.transient.get_mut(&id) {
            tr.path = Some(path);
            if art.is_some() {
                tr.art = art.map(|a| a.0);
                tr.color = art.map(|a| a.1);
            }
        }
    }

    pub(crate) fn on_fetch_failed(&mut self, item: YtItem, msg: String) {
        // The audio thread already moved on; say why if it was the song you were waiting for.
        let id = stream_id(&item.id);
        if self.current_id() == Some(id) {
            let name = self.info_for(id).map(|t| t.title).unwrap_or_default();
            self.toast(format!("Couldn't play “{name}”: {msg}"));
        }
    }

    /// Streamed songs that are now in the library (downloaded here or found in the folder)
    /// become the files themselves, in the same place in every playlist.
    pub(crate) fn resolve_streams(&mut self) {
        let mut changed = false;
        for p in self.playlists.iter_mut() {
            if p.streams.is_empty() {
                continue;
            }
            for entry in p.tracks.iter_mut() {
                let Some(key) = stream_key(entry).map(str::to_string) else { continue };
                if let Some(t) = self.lib.by_key.get(&key).and_then(|id| self.lib.get(*id)) {
                    *entry = t.path.clone();
                    p.streams.remove(&key);
                    changed = true;
                }
            }
        }
        if changed {
            store::save_playlists(&self.playlists);
        }
    }

    /// A download finished: its streamed entries become the file right away (the library
    /// catches up on its next scan).
    pub(crate) fn stream_downloaded(&mut self, key: &str, path: &std::path::Path) {
        let entry = stream_entry(key);
        let mut changed = false;
        for p in self.playlists.iter_mut() {
            if let Some(i) = p.tracks.iter().position(|t| *t == entry) {
                if p.tracks.contains(&path.to_path_buf()) {
                    p.tracks.remove(i);
                } else {
                    p.tracks[i] = path.to_path_buf();
                }
                p.streams.remove(key);
                changed = true;
            }
        }
        if changed {
            store::save_playlists(&self.playlists);
        }
    }

    /// The songs of a playlist that aren't downloaded yet (in playlist order).
    pub(crate) fn playlist_streams(&self, pid: u64) -> Vec<YtItem> {
        let Some(p) = self.playlist(pid) else { return Vec::new() };
        p.tracks.iter().filter_map(|t| stream_key(t).and_then(|k| p.streams.get(k)).cloned()).collect()
    }

    /// Download every streamed song of a playlist; each takes its file's place when done.
    pub(crate) fn download_playlist(&mut self, pid: u64) {
        let busy: HashSet<String> = self
            .yt
            .shared
            .lock()
            .map(|s| {
                s.jobs
                    .iter()
                    .filter(|j| matches!(j.status, ytdlp::JobStatus::Waiting | ytdlp::JobStatus::Downloading { .. } | ytdlp::JobStatus::Saving))
                    .map(|j| j.item.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        let need: Vec<YtItem> = self.playlist_streams(pid).into_iter().filter(|i| !busy.contains(&i.id)).collect();
        if need.is_empty() {
            return;
        }
        let n = need.len();
        self.yt_requested = true;
        self.yt.download(need, Some(pid));
        self.toast(format!("Downloading {}", crate::theme::plural(n, "song", "songs")));
    }

    /// Download one streamed song (from a row, its menu, or the player bar).
    pub(crate) fn download_stream(&mut self, id: u64) {
        if let Some(tr) = self.transient.get(&id) {
            let item = tr.item.clone();
            self.actions.push(Action::YtDownload { items: vec![item], playlist_name: None });
        }
    }
}
