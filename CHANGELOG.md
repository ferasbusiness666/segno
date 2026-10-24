# What's new in Segno

<!--
How this file is used:
- Segno shows the newest entries in a "What's new" window the first time it opens after an
  update, and the whole list under Settings > Updates > What's new.
- A published GitHub release uses the section for its version as the release notes.
Each version starts with "## <version>" (the same as in Cargo.toml). Inside, use
"### New", "### Better" or "### Fixed", with one "- " line per change. Keep lines short.
-->

## 1.1.10

### Fixed
- Two songs from direct links with the same file name (like ".../album-a/song.mp3" and ".../album-b/song.mp3") were treated as one song. Each link is its own song now.
- Queueing more than 300 downloads could lose the oldest ones still waiting when Segno closed. Every unfinished download is kept now.
- Adding, changing or removing a cover picture (cover.jpg, folder.png…) in a song's folder now updates its art right away.
- "Go to artist" on a song with no artist opens the Unknown artist page instead of nothing.

## 1.1.9

### Fixed
- Segno could close right after opening if an artist's name had certain letters in it (like a special "K" before "feat."). Those names work now and go under the right artist.
- Two different songs from the same site could share one saved file, so playing one gave you the other.
- Finishing or stopping one download could delete the half-done file of another download.
- Songs named like a Windows device ("CON", "COM4", "LPT9"…) couldn't be saved. They get a "_" added now.

## 1.1.8

### New
- A new logo: a pink record with a play mark and two lines of shine. It's the app icon, the taskbar icon and the logo inside Segno.
- At small sizes (the taskbar tray), a simpler version keeps the icon sharp.

## 1.1.7

### New
- Drag playlists in the sidebar to put them in the order you want.
- Settings has sections now (Library, Playback, Downloads, Updates, Shortcuts, About), and About shows who made Segno, the source code and how to get in touch.
- An empty playlist or Loved songs page lets you search your library and add songs right there.

### Better
- The bars next to the playing song move while it plays and stop when you pause, everywhere they show up.
- Now Playing and full screen fade between songs: the cover, the record label, the title and the background.
- The sidebar highlight moves faster, and icons react a little when you point at them.
- Renaming a playlist happens right where the name is, and new playlists get a nicer picture.
- More room between albums and artists, so the hover doesn't touch the row below.
- "For you" is hidden for now, until it can suggest songs well.

### Fixed
- The Loved songs picture now has round corners at the bottom too.

## 1.1.6

### New
- The queue is split into groups so you can see what plays next: "Added by you", then the rest of what you pressed play on ("From Road Trip Mix"), then "From your library".
- The sound fades out when you pause and fades back in when you play. Skipping, seeking and the volume slider are smooth too, with no clicks.

### Better
- "Add to queue" puts the song right after the current one and the others you added, not at the very end behind a whole album.
- Animations across the app: rows slide in when a page opens, menus grow from where you click, the sidebar highlight glides between pages, and the song in the player bar changes with a fade.
- Queue rows glide to their new spot when songs are added, removed or played, and new songs fade in.
- Every button gives a little press when you click it.

## 1.1.5

### New
- Dragging a song now lifts the row itself out of the list. It follows your mouse and the other songs slide apart to make room.
- Drag a song over the queue and it flies across and lines up there. Drop it exactly where you want it to play.
- Drag songs from the queue into a playlist, at any spot.

### Better
- While you hold a song, the mouse wheel still scrolls, and holding it near the top or bottom of a list scrolls too. This works in Songs and playlists now, not only the queue.
- Dropped songs glide into their new place. Let go somewhere that doesn't take songs and it flies back.

## 1.1.4

### New
- A "What's new" window shows what changed the first time you open Segno after an update. It's also in Settings, under Updates.

### Better
- If you close Segno or shut down your computer without pressing "Restart now", the update installs the next time you open Segno.
- Installing an update is safer: the new version is copied in first, then swapped in at once.

## 1.1.3

### Better
- Full screen no longer flickers: the window fills the screen without switching the display mode, with a quick fade.
- In full screen, the record and song title glide to the middle when the controls hide.
- The queue slides in from the right everywhere it opens.
- Segno looks for updates every 3 hours instead of once a day.

## 1.1.2

### New
- Before your first download, Segno asks you to only download music you're allowed to.

## 1.1.1

### New
- Saved playlists play online right away. Download one song from its row, or all of them with the Download button on the playlist.
- Downloaded songs take their own place in the playlist, so a playlist can mix online and downloaded songs.
- Songs that play online work in the queue, Play next and Add to playlist, and stay in the queue after a restart.

## 1.1.0

### New
- Paste links from SoundCloud, Bandcamp and the other sites yt-dlp supports, not just YouTube.
- Spotify links work too: Segno finds each song on YouTube.
- Save a pasted playlist to your sidebar, with its own cover.
- Pick your own picture for any playlist.
- For you: new songs based on what you play.
- Now Playing has a spinning record and a full-screen view (F11).
- Up next is fully editable: drag to reorder, remove songs, and add songs from your library with +.

### Better
- The Downloads list is kept after a restart, and unfinished downloads carry on.
- Song rows no longer show the drag handle.

### Fixed
- The play button on album and artist cards no longer flickers.

## 1.0.1

### Better
- A polished look across the whole app.

## 1.0.0

### New
- The first release of Segno.
