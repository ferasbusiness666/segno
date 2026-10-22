# Publishing Segno on the Microsoft Store

The Store signs the package, so there's no certificate to buy. The Store version has the
built-in updater turned off, because the Store installs updates.

## One-time setup

1. Sign up as an individual developer at https://storedeveloper.microsoft.com (free).
2. In Partner Center: **Apps and games → New product → MSIX or PWA app**, reserve the name **Segno**.
3. Open the app's **Product identity** page and copy:
   - Package/Identity/Name
   - Package/Identity/Publisher (starts with `CN=`)
   - Package/Properties/PublisherDisplayName
4. On GitHub: **Settings → Secrets and variables → Actions → Variables → New repository variable**,
   add them as `MSIX_NAME`, `MSIX_PUBLISHER` and `MSIX_PUBLISHER_NAME`.

## Each release

1. Run the Build workflow as usual. Open the run and download the **Segno-msix** artifact (a zip with `Segno.msix` inside).
2. In Partner Center, start a submission, upload `Segno.msix` under **Packages**, fill in the listing, and submit.

The version in the package is the one in `Cargo.toml` with `.0` at the end (1.1.2 → 1.1.2.0).
Each new submission needs a higher version than the last.

## Listing text

**Short description**

A light, modern music player for your own music, with playlists, a spinning-record Now Playing view, and downloads of music you're allowed to keep.

**Description**

Segno is a fast, good-looking music player for Windows.

- Plays everything in your music folder: MP3, M4A/AAC, FLAC, OGG, WAV and AIFF
- Songs, albums, artists, loved songs and your own playlists, with drag and drop
- A queue you can reorder, plus Now Playing with a spinning record and a full-screen view
- Mini player, media keys and the Windows media controls
- Light on battery: it barely redraws when nothing changes

Segno can also play and save music from YouTube, SoundCloud and other sites.
Only download music you own, music that's free to download, or music the artist or rights
holder lets you download. You're responsible for what you download and for following each
site's terms. Segno asks you to confirm this before your first download.

**Notes for certification** (the "Notes for certification" box in the submission)

Segno is a local music player. Its optional download feature uses the open-source yt-dlp tool,
which Segno downloads on first use into its own app data folder. Before the first download,
Segno shows a notice that users may only download content they own or have permission to
download. No account or sign-in is needed to test the app: pick any folder with audio files
on first launch.

**Privacy policy URL**

https://github.com/ferasbusiness666/segno/blob/main/PRIVACY.md

**Store logo**: `packaging/msix/store-logo-300.png`
