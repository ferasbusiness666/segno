# Segno roadmap

What's done, what's half done, and what's next.

✅ done · 🟡 half done / on hold · ⬜ not started

The pitch: **one app for your own music and online music. Light, fast, free, no account.**
For the website: **"Your music files and online finds, together. No account needed."**

The goal: be the best music player of its kind, not just as good as the others.

---

## Done (1.0 – 1.1.2)

### Player
- ✅ Plays your music folder (MP3, M4A/AAC, ALAC, FLAC, OGG, WAV, AIFF), subfolders too, sees new files by itself
- ✅ Songs, Albums, Artists, Loved songs, your own playlists, drag and drop everywhere
- ✅ Shuffle, repeat, Play next, Add to queue
- ✅ Queue: drag to reorder, remove, add songs from your library (+ button)
- ✅ Keeps playing from your library when the queue ends
- ✅ Now Playing with a spinning record, full screen (F11), Up next beside it
- ✅ Mini player, media keys, Windows media overlay, keeps playing in the tray
- ✅ Remembers your song, queue, volume and window
- ✅ Light on battery (redraws only when needed)

### Online music
- ✅ Search YouTube, paste links from YouTube, SoundCloud, Bandcamp and other sites yt-dlp supports
- ✅ Spotify links (songs, albums, playlists), found on YouTube
- ✅ Play online without downloading, download when you want
- ✅ Save playlists from links: they play online, download one song or all of them, files take their place
- ✅ Playlist covers from the site, or your own picture
- 🟡 "For you": hidden in 1.1.7. It used YouTube mixes from what you play, which wasn't useful enough. Bring it back when there's a better free source (ideas: Last.fm similar artists, ListenBrainz)
- ✅ Downloads list kept between restarts, unfinished downloads continue
- ✅ "Only download what you're allowed to" notice before the first download

### App
- ✅ Updates itself (GitHub releases)
- ✅ Open source (MIT), privacy policy (PRIVACY.md)

---

## First: polish (Segno 1.1.x), before building more on top

Rule: animations can cost a bit more battery, but RAM must stay light (now about 20–40 MB).

### Dragging songs
- ✅ **Hold the song itself**: when you drag a song, the row lifts out of the list (bigger, shadow, "closer to you") instead of the small pink tag (1.1.5)
- ✅ **Stays in line**: while reordering, the lifted row only moves up and down with the list, the other rows slide apart to make room (1.1.5)
- ✅ **Flies to the queue**: drag it over the queue panel and it flies over and resizes to fit the panel (with a little motion blur), then lines up there (1.1.5)
- ✅ **Smooth drop**: rows animate into their new place instead of jumping; let go nowhere and it flies back (1.1.5)
- ✅ **Scroll while dragging**: the mouse wheel works while holding a song, and holding it near the top or bottom of any list scrolls it (1.1.5)

### Queue
- ✅ **Clear what plays next**: the queue is grouped into "Added by you", "From <what you played>" and "From your library"; Add to queue goes before the rest of the list (1.1.6)
- ✅ **Queue panel slides in from the right** everywhere it opens (1.1.3)

### Full screen
- ✅ **No flicker** when going in and out of full screen: a borderless window that fills the screen, plus a quick fade (1.1.3)
- ✅ **Re-center when controls hide**: record and song info glide to the middle when the controls fade, and back up when they return (1.1.3)
- ⬜ **Lyrics spot**: when synced lyrics are on, they sit under the record and song title (see 1.2)

### General
- ✅ **What's new window**: shows what changed the first time Segno opens after an update, also in Settings; the same text is the GitHub release notes (CHANGELOG.md) (1.1.4)
- ✅ **Updates install even without "Restart now"**: closed or shut down? It installs the next time you open Segno (1.1.4)
- ✅ **Better animations across the app**: rows slide in on page open, menus grow in, sliding sidebar highlight, player bar song change, queue rows glide, button press (1.1.6)
- ✅ **Smooth volume**: fades on play, pause, skip, seek and volume changes, no clicks (1.1.6)
- ✅ **Settings in sections** with an About part (author, source code, contact) (1.1.7)
- ✅ **Reorder playlists** in the sidebar by dragging (1.1.7)
- ✅ **Moving "now playing" bars**, Now Playing fades between songs, add songs from an empty playlist (1.1.7)
- ✅ **Check for updates more often**: every 3 hours instead of once a day (1.1.3)

---

## How Segno compares (Codex review, October 2026)

Segno's biggest strength: your own music files, online music and downloads in one simple app,
no account. Songs saved from a link turn into real files in place.

| App | They do better | Segno does better |
|---|---|---|
| **Liner** | Lyrics that follow the song; checks imported songs for wrong matches; lets you control cache size | Plays your own files with no account; you keep normal music files; links from more sites. (Liner's licence doesn't let us copy its code.) |
| **Moosync** | Better organised code, tests for playback, managing saved library data | Simpler to install; a straight path from "found it online" to "it's in my library" |
| **Spotube** | Phone app, synced lyrics, volume levelling, more music services to connect | Remembers unfinished downloads after a restart; doesn't rescan files that didn't change |
| **Strawberry** | Sound controls (EQ), even volume, smooth changes between songs | Free portable Windows download (theirs needs a sponsorship); easy importing from many sites |

Weak spots Codex found:
- **Online songs wait for the whole file**: a song you play from search downloads completely
  before it starts (checked in the code: `run_preview`). The next song in line is fetched early,
  so it's the first play that's slow.
- **Preview cache**: there's no size limit, only a 3-day cleanup (checked).
- **Album songs back to back**: the fades stop clicks, but there's still a small gap between songs.
- **Four bugs** Codex reproduced, all checked and fixed in 1.1.9, each with a test:
  - ✅ **Crash on some artist names**: lowercasing a name to find "feat." and then cutting the
    original at that spot broke on letters that change length (the Kelvin sign "K"). With such a
    song in the music folder, 1.1.8 crashed every time it opened. Other names were grouped wrong
    ("İlkay, X" became the artist "İlkay,").
  - ✅ **Preview cache gave the wrong song**: cache names came from the id with symbols turned
    into "_", so "soundcloud:a.b" and "soundcloud:a_b" shared a file. Names now add a hash of the
    full id, and each entry is checked against the song's id before it's used.
  - ✅ **Cleanup deleted another download's files**: cleaning up "soundcloud_12" also deleted
    "soundcloud_123.m4a". Now only "<name>." files count.
  - ✅ **Windows device names got through**: "COM4", "LPT9", "CON.txt" couldn't be saved. Now
    the full list is checked (CON, PRN, AUX, NUL, CONIN$, CONOUT$, COM/LPT 0–9 and ¹²³), with or
    without an extension.
- **Four more bugs** from Codex's second review (it also confirmed the first four are fixed),
  all fixed in 1.1.10, each with a test:
  - ✅ **Direct links with the same file name were one song**: yt-dlp's Generic extractor names
    a song after its file, so ".../album-a/song.mp3" and ".../album-b/song.mp3" shared an id and
    a cached file. Generic links now use a hash of the whole link as their id.
  - ✅ **Over 300 downloads lost waiting ones**: the saved list kept only the newest 300 of
    everything. Now every unfinished download is kept and only finished ones are capped at 300.
  - ✅ **Folder covers added later didn't show**: the watcher ignored pictures, and a rescan
    skipped unchanged songs before looking for one. Now cover pictures trigger a rescan, and each
    song keeps a fingerprint (name, size, time) of its folder picture, so a new, changed or
    removed picture is noticed without reading every picture on every scan.
  - ✅ **"Unknown artist" links went nowhere**: the Artists page used "Unknown artist" while the
    links used an empty name. One shared `Track::artist_key` now.

Codex's order: 1. the four bugs, 2. cache mix-ups and a storage limit, 3. synced lyrics,
4. online songs start sooner, 5. even volume and smooth changes between songs.

## Next: Segno 1.2 (catch up, then pass Cider, Liner and the rest)

In this order:
- ✅ **Fix the four bugs** Codex reproduced (1.1.9)
- 🟡 **Preview cache**: no mix-ups between songs (1.1.9); still to do: a size limit you can set in Settings
- ⬜ **Synced lyrics**: lines scroll and light up with the song (free from LRCLIB); when turned on, they show in full screen under the record and song title
- ⬜ **Online songs start sooner**: start playing while the song is still coming in, not after
- ⬜ **Volume levelling**: loud and quiet songs play at the same level
- ⬜ **Crossfade and gapless playback**: no gap between album songs
- ⬜ **No stutter when the laptop lags**: much better since the 150 ms output buffer, but music
  still stutters when the laptop freezes for longer than that (reported on Feras's i3 laptop).
  The songs are decoded inside the audio callback, so a long stall empties the buffer. Fix:
  decode a second or two ahead on its own thread (the callback only copies samples), and ask
  Windows to give the audio thread priority (MMCSS "Pro Audio"). Do it together with crossfade
  and gapless, which change the same part of the player.
- ⬜ **Equalizer**: presets (Bass boost, Vocal, Rock…) and custom sliders
- ⬜ **Discord status**: "Listening to … on Segno"
- ⬜ **Wrong match? Pick another**: for imported songs (Spotify links) that found the wrong video
- ⬜ **Tests for playback**, so bugs like these get caught before a release
- ⬜ **New icon shows after an update**: tell Windows to reload Segno's icon the first time a new version opens

## After that: make Segno stand out

- ⬜ **Phone remote**: scan a QR code, control Segno from your phone, no app needed
- ⬜ **Playlist sync**: saved playlists update when the original changes
- ⬜ **Import from more places**: YouTube Music, Apple Music, Deezer links
- ⬜ **Last.fm scrobbling**
- ⬜ **Listening stats** and a shareable "Segno Wrapped" image

## Small but loved

- ⬜ Sleep timer
- ⬜ App colour follows the cover of the song playing
- ✅ Keyboard shortcut list in Settings

## More platforms

- ⬜ **Linux** (promised as "coming soon" on the website)
- ⬜ **Android** (later, maybe before Linux)
- ⬜ Mac: not planned yet, ask by email

---

## Outside the app

- 🟡 **Website**: plan ready (dark and bold like cider.sh, pink-red accent, email signup near the top going to Google Sheets, socials in the footer, "Made by Feras Hania"). Not built yet. Free subdomain first, segnomusic.app later. Email form: Google Sheet "Segno emails" with an Apps Script web app, POST `email` (and a hidden `website` field for bots) to https://script.google.com/macros/s/AKfycbxvFcS3lS_x1E3Le78fC-EDkydAy3mfSmWmqOkU8JWjTNenniW6h61SkDriBRBKeHLk/exec. Still to do: say in PRIVACY.md what the email list keeps and how to be removed.
- 🟡 **Microsoft Store**: package build worked, then put on hold until Partner Center verification works. Restore it from the git history ("Put the Microsoft Store package on hold" commit). The Store may reject apps that download from YouTube.
- ⬜ **Domain**: segnomusic.app not bought yet
- ✅ **New logo** (1.1.8): the pink record. Files, colours and rules in `brand/`
- ✅ **Screenshots** for the website: 4 pictures from the real app at 2×, with a made-up demo library (invented artists and songs, covers drawn in code) so nothing in them is someone else's
