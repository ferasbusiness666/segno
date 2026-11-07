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
- 🟡 **Preview cache**: no mix-ups between songs (1.1.9). A size limit in Settings only if it turns out to grow big (it clears after 3 days)
- ✅ **Synced lyrics** (1.1.12): lines scroll and light up with the song (free from LRCLIB), straight on the background beside the record on Now Playing and full screen, click a line to jump, Arabic reads right to left; your own .lrc file next to a song wins. 1.1.13: Try again, plain lyrics when there are no timed ones, slowed/sped-up versions stretch the original, much better matching for YouTube titles (checked against LRCLIB with real songs). 1.1.16: lyrics in every language (Japanese, Chinese, Korean, Thai, Hindi… with Windows' own fonts, loaded only when needed), "Wrong lyrics?", timed lyrics saved as .lrc next to songs, better exact matching. Next idea: pick from a list of matches
- 🟡 **Online songs start sooner**: 1.1.19 keeps yt-dlp's cache (it used to re-solve YouTube's player code for every song) and searches YouTube directly (no yt-dlp start per search). 1.1.21 plays a song while it's still coming in (from about 256 KB, read as the file grows; seeking waits for the whole file). Maybe later: yt-dlp's unpacked Windows build (yt-dlp_win.zip) so it doesn't unpack itself on every start
- ⬜ **Volume levelling**: loud and quiet songs play at the same level (later, to get it right)
- ⬜ **Crossfade and gapless playback**: no gap between album songs (later, to get it right)
- ✅ **No stutter when the laptop lags** (1.1.21): songs are decoded (and converted to the
  output's format) 2–3 s ahead on their own thread, which runs above normal priority; the audio
  callback only copies samples out of a lock-free buffer (`ahead.rs`). The audio thread asks
  Windows for audio priority (MMCSS, cpal's "realtime"). Seeks are handed to the decode thread.
  Crossfade, gapless and levelling build on this later.
- ⬜ **Equalizer**: presets (Bass boost, Vocal, Rock…) and custom sliders. **Top of the list, with the sleep timer.**
- ✅ **Sleep timer** (1.1.20): a moon in the player bar opens a small menu: 15 / 30 / 45 min, 1 hour or end of song. The last 30 s fade out, then it pauses and the volume goes back for next time. +10 min, turn off, and touching anything during the fade cancels it. Runs in the audio engine, so it works with the window in the tray
- ✅ **Discord status** (1.1.20): "Listening to Segno" with the song, artist, cover, a time bar and a "Get Segno" button, through the Discord app on the same PC (looks for it every 20 s). Cleared on pause. On by default with a one-time note; off in Settings → Playback
- ✅ **Wrong song? Pick another** (1.1.18): Spotify songs that play the wrong YouTube video: pick the right one from the results (same length as on Spotify is marked), remembered everywhere. The first guess also skips live, cover, sped-up and other versions unless the song itself is one
- 🟡 **Tests for playback**, so bugs like these get caught before a release. 1.1.21: decode-ahead (order, seeks, a slow decoder, channels in step, the thread stopping), files read while downloading, the sleep timer's fade. Still to do: the queue itself (next, shuffle, repeat, end of queue), which needs the player to run without a sound card
- ~~New icon shows after an update~~: not needed, Segno launched with the new icon

## After that: make Segno stand out

- ⬜ **Phone remote**: scan a QR code, control Segno from your phone, no app needed
- ⬜ **Playlist sync**: saved playlists update when the original changes
- ✅ **Import playlists page** (1.1.15): the "+" next to Playlists offers New playlist or Import playlists. Paste one or many links: playlists from any site, a YouTube channel (all its public playlists), or songs copied from the Spotify app (Ctrl+A, Ctrl+C: Liked Songs and private playlists). Tick what to keep. Steps for each site on the page. Spotify profiles can't work: tested with Feras's profile, the page only fills in its playlists in the browser, so the app explains and asks for playlist links (1.1.17).
- 🟡 **Sign in with Google**: on hold, not wanted for now. (It would bring every YouTube playlist, private ones too, but needs Google to check the app first.)
- ⬜ **Spotify sign-in: not possible.** Since 2025–26 Spotify only allows it for companies with 250,000+ monthly users; others are capped at 5 users. The import page covers Spotify without it.
- ⬜ **Import from more places**: Apple Music and Deezer links (Deezer has a free public API)
- ⬜ **Paste a plain list**: "Artist - Song" lines (from notes, a CSV, anywhere) become a playlist
- ⬜ **Last.fm scrobbling**
- ⬜ **Listening stats** and a shareable "Segno Wrapped" image

## Next: lyrics you can trust, on desktop and Android (before Android 1.0.0 ships lyrics)

Why lyrics go wrong today:
1. **The audio isn't the version the timing was made for.** Music videos have intros, skits and longer
   endings; live, sped-up and radio versions differ. That's "off from the start" and "completely off".
2. **Messy names.** A video title like "MARWAN PABLO x LEGE-CY | شفت كلام" makes the search pick the
   wrong song.
3. **One source.** LRCLIB is often busy (503) and doesn't have everything.
4. **Segno's clock runs a little ahead of the speakers** (the output buffer, about 0.15–0.2 s on
   desktop), so lines change slightly early.

### Step 1: play the right audio, with clean names
- ✅ **Two buttons in Search, on both apps: YouTube Music and YouTube.** Segno remembers the last one.
  (Android 1.0.0, desktop 1.1.22)
  YouTube Music gives songs: the official album audio with a clean title, artist, album and length.
  YouTube gives everything else: music videos, live versions, covers. Play or download from either.
  If YouTube Music doesn't work in your country, Segno says so and offers YouTube.
- ✅ **Spotify songs are matched to YouTube Music songs first** (the same album audio, so the same
  length), and to YouTube videos only if that fails. (desktop 1.1.22)
- 🟡 **Every song remembers where it came from** (the YouTube video id is used for lyrics since
  desktop 1.1.22) (YouTube Music song, YouTube video, Spotify, file)
  and its clean title, artist, album and length. For YouTube videos, use YouTube's "Music in this
  video" details when the video has them, before guessing from the title.

### Step 2: more sources, asked together, the best answer wins
In this order of trust:
1. 🟡 **Yours**: an .lrc file next to the song (lyrics inside the file: not yet). Always wins.
2. ✅ **Saved**: what Segno found before, with your timing fixes.
3. ✅ **YouTube Music's own timed lyrics: the main source** (desktop 1.1.22) (Feras's call). Timed to the album audio
   itself; everything below is a backup for when it has no timed lyrics, isn't reachable (it's
   blocked in some countries), or the song isn't on YouTube Music.
4. ✅ **LRCLIB**: only used as timed when its length is close to the song's.
5. ✅ **NetEase Cloud Music** as a backup, same rule. (desktop 1.1.22)
6. ⬜ **For YouTube videos: the video's uploaded captions** when they're the lyrics (not
   auto-generated). Timed to that exact video, intro included.
7. If only untimed lyrics exist anywhere, show them as "Not timed to the song" (as now).
- ✅ Ask the sources at the same time with short time limits, so one busy source doesn't hold things
  up. A source that fails while the others answer is rested for half an hour. (desktop 1.1.22)
- Honest risk: YouTube Music's lyrics and NetEase are unofficial and can break or be blocked. That's
  why there are several.

### Step 3: make the timing fit the audio that's playing
- 🟡 Same length (within 2 s): use as is. Sped up or slowed: stretch (desktop does this already).
- ⬜ **A music video with an intro or extra parts: line it up with the official audio.** Segno
  compares the sound of the video with the YouTube Music audio of the same song (how the notes
  change over time) and works out where each moment of one sits in the other. That fixes a late
  start and cuts in the middle. Done once per song in the background, then saved.
- ✅ **Count the speaker delay**, so a line lights up exactly when it's sung. (desktop 1.1.22)
- 🟡 Still off: Earlier/Later (desktop, 1.1.17), plus a new **Tap to sync**: play the song and tap
  as each line starts, saved as an .lrc next to the song. Later: offer to share it with LRCLIB so
  everyone gets it.

### Step 4: backups and checks
- 🟡 Found lyrics are saved and kept next to songs as .lrc (desktop, 1.1.16). Android also writes
  them into downloaded M4A files, so other music apps show them.
- 🟡 No lyrics today: Segno tries again later by itself, and again after a matching fix.
- 🟡 **One list of tricky test songs** (tests/data/tricky-songs.json, checked live by the "Lyrics
  live check" workflow; Android uses the same file) (Arabic, "x" and "feat.", remixes, sped-up, live, long intros)
  that both apps are tested against, so desktop and Android always pick the same lyrics.
- 🟡 "Wrong lyrics?" and "Wrong song? Pick another" on both apps (desktop has both).

### Step 5: Android gets lyrics
- ⬜ The same engine in Kotlin, lyrics in Now Playing (the current line large, tap a line to jump
  there), and the same fixes. Kept light for weak phones: the line-up in Step 3 only runs when the
  lyrics are open and the lengths don't match.

Order: Step 1 on both apps → Steps 2 and 4 on desktop → Step 5 (Android, with the same rules) →
Step 3's line-up, desktop first.

## Small but loved

- ⬜ **Move your stuff (migration)**: export and import your playlists, loved songs, play history
  and settings as one file. Use it for a new PC, a backup, or to bring everything to Segno for
  Android (the same file works on both). Saved files also carry a version number, so updates can
  upgrade old ones safely instead of losing them.

- ⬜ **Customizable full screen**: choose what shows (the record or just the cover, lyrics, Up next, the controls), the background (blurred cover, the cover's colour, plain dark), where things sit and how big the lyrics are

- ⬜ App colour follows the cover of the song playing
- ✅ Keyboard shortcut list in Settings

## More platforms

- ⬜ **Linux** (promised as "coming soon" on the website)
- 🟡 **Android**: built (Segno-Android repo), 1.0.0 ships with Back up and restore and the two search buttons; lyrics come in 1.1
- ⬜ Mac: not planned yet, ask by email

---

## Outside the app

- ✅ **Website**: live at https://segnomusic.pages.dev (Cloudflare Pages project `segnomusic`, repo `segno-website`). Designed in Claude Design, turned into plain HTML/CSS/JS. Has the email form (Google Sheet "Segno emails" through Apps Script), crypto donations (popup after the first download and a Support section, addresses checked), Privacy Policy and Terms of Use. Domain segnomusic.app later.
- 🟡 **Microsoft Store**: package build worked, then put on hold until Partner Center verification works. Restore it from the git history ("Put the Microsoft Store package on hold" commit). The Store may reject apps that download from YouTube.
- ⬜ **Domain**: segnomusic.app not bought yet
- ✅ **New logo** (1.1.8): the pink record. Files, colours and rules in `brand/`
- ✅ **Screenshots** for the website: 4 pictures from the real app at 2×, with a made-up demo library (invented artists and songs, covers drawn in code) so nothing in them is someone else's
