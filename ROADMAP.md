# Segno roadmap

What's done, what's half done, and what's next.

✅ done · 🟡 half done / on hold · ⬜ not started

The pitch: **one app for your own music and online music. Light, fast, free, no account.**

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
- ✅ "For you": new songs based on what you play
- ✅ Downloads list kept between restarts, unfinished downloads continue
- ✅ "Only download what you're allowed to" notice before the first download

### App
- ✅ Updates itself (GitHub releases)
- ✅ Open source (MIT), privacy policy (PRIVACY.md)

---

## First: polish (Segno 1.1.x), before building more on top

Rule: animations can cost a bit more battery, but RAM must stay light (now about 20–40 MB).

### Dragging songs
- ⬜ **Hold the song itself**: when you drag a song, the row lifts out of the list (bigger, shadow, "closer to you") instead of the small pink tag
- ⬜ **Stays in line**: while reordering, the lifted row only moves up and down with the list, the other rows slide apart to make room
- ⬜ **Flies to the queue**: drag it over the queue panel and it flies over and resizes to fit the panel (with a little motion blur), then lines up there
- ⬜ **Smooth drop**: rows animate into their new place instead of jumping
- ⬜ **Scroll while dragging**: holding a song near the top or bottom of any list scrolls it (works in the queue, not in Songs and playlists yet)

### Queue
- ⬜ **Clear what plays next**: make it obvious what comes after the current song, what you added by hand, and what Segno adds from your library
- ✅ **Queue panel slides in from the right** everywhere it opens (1.1.3)

### Full screen
- ✅ **No flicker** when going in and out of full screen: a borderless window that fills the screen, plus a quick fade (1.1.3)
- ✅ **Re-center when controls hide**: record and song info glide to the middle when the controls fade, and back up when they return (1.1.3)
- ⬜ **Lyrics spot**: when synced lyrics are on, they sit under the record and song title (see 1.2)

### General
- ⬜ **Better animations across the app**: page changes, hovers, panels, lists
- ✅ **Check for updates more often**: every 3 hours instead of once a day (1.1.3)

---

## Next: Segno 1.2 (close the gap with Cider and Liner)

- ⬜ **Synced lyrics**: lines scroll and light up with the song (free from LRCLIB); when turned on, they show in full screen under the record and song title
- ⬜ **Equalizer**: presets (Bass boost, Vocal, Rock…) and custom sliders
- ⬜ **Volume levelling**: loud and quiet songs play at the same level
- ⬜ **Crossfade and gapless playback**
- ⬜ **Discord status**: "Listening to … on Segno"

## After that: make Segno stand out

- ⬜ **Phone remote**: scan a QR code, control Segno from your phone, no app needed
- ⬜ **Playlist sync**: saved playlists update when the original changes
- ⬜ **Import from more places**: YouTube Music, Apple Music, Deezer links
- ⬜ **Last.fm scrobbling**
- ⬜ **Listening stats** and a shareable "Segno Wrapped" image

## Small but loved

- ⬜ Sleep timer
- ⬜ App colour follows the cover of the song playing
- ⬜ Keyboard shortcut list in the app (some shortcuts exist, no full list yet)

## More platforms

- ⬜ **Linux** (promised as "coming soon" on the website)
- ⬜ **Android** (later, maybe before Linux)
- ⬜ Mac: not planned yet, ask by email

---

## Outside the app

- 🟡 **Website**: plan ready (dark and bold like cider.sh, pink-red accent, email signup near the top going to Google Sheets, socials in the footer, "Made by Feras Hania"). Not built yet. Free subdomain first, segnomusic.app later.
- 🟡 **Microsoft Store**: package build worked, then put on hold until Partner Center verification works. Restore it from the git history ("Put the Microsoft Store package on hold" commit). The Store may reject apps that download from YouTube.
- ⬜ **Domain**: segnomusic.app not bought yet
- ⬜ **Screenshots** for the website and README
