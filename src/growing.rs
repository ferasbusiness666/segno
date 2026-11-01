//! Playing a song from its site while it's still downloading: the file is read as it grows,
//! waiting for more when playback catches up with the download.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Start playing once this much has arrived (about 15 s of a YouTube song, which comes in at
/// many times that speed).
pub const START_BYTES: u64 = 256 * 1024;

const COMING: u8 = 0;
const DONE: u8 = 1;
const FAILED: u8 = 2;

/// How the download is going, shared by the downloader and whoever reads the file.
#[derive(Default)]
pub struct Download(AtomicU8);

impl Download {
    pub fn finish(&self, ok: bool) {
        self.0.store(if ok { DONE } else { FAILED }, Ordering::Release);
    }
    pub fn done(&self) -> bool {
        self.0.load(Ordering::Acquire) == DONE
    }
    fn state(&self) -> u8 {
        self.0.load(Ordering::Acquire)
    }
}

/// A song that has started arriving.
#[derive(Clone)]
pub struct Arrival {
    /// The file the downloader is writing. It's renamed when it's done, but stays the same
    /// file, so reading carries on.
    pub path: PathBuf,
    /// What kind of audio it is ("m4a"), for the decoder (the file ends in ".part").
    pub ext: String,
    pub download: Arc<Download>,
}

/// The file being written for `stem` in `dir`, once enough of it is there to start.
pub fn started(dir: &Path, stem: &str) -> Option<(PathBuf, String)> {
    crate::library::AUDIO_EXT.iter().find_map(|e| {
        let p = dir.join(format!("{stem}.{e}.part"));
        let len = std::fs::metadata(&p).ok()?.len();
        (len >= START_BYTES).then(|| (p, e.to_string()))
    })
}

/// Reads a file that's still being written: at its current end it waits for more, until the
/// download is done (the real end) or fails (the song ends there).
pub struct Growing {
    file: File,
    download: Arc<Download>,
    /// Once playing, wait up to a minute for a slow download; while the decoder first looks at
    /// the file, give up after a few seconds (then the song waits for the whole file instead).
    patient: Arc<AtomicBool>,
}

impl Growing {
    pub fn open(a: &Arrival) -> io::Result<(Self, Arc<AtomicBool>)> {
        let patient = Arc::new(AtomicBool::new(false));
        let file = File::open(&a.path)?;
        Ok((Growing { file, download: a.download.clone(), patient: patient.clone() }, patient))
    }
}

impl Read for Growing {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let since = Instant::now();
        loop {
            let n = self.file.read(buf)?;
            if n > 0 || buf.is_empty() {
                return Ok(n);
            }
            match self.download.state() {
                // Whatever came in at the very end, then the real end.
                DONE => return self.file.read(buf),
                FAILED => return Ok(0),
                _ => {}
            }
            let limit = if self.patient.load(Ordering::Relaxed) { 60 } else { 3 };
            if since.elapsed() > Duration::from_secs(limit) {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "the download stopped"));
            }
            std::thread::sleep(Duration::from_millis(40));
        }
    }
}

impl Seek for Growing {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        if matches!(pos, SeekFrom::End(_)) && self.download.state() == COMING {
            return Err(io::Error::new(io::ErrorKind::Unsupported, "still downloading"));
        }
        self.file.seek(pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn arrival(name: &str) -> (Arrival, File) {
        let dir = std::env::temp_dir().join(format!("segno-growing-{}-{name}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("song.m4a.part");
        let f = File::create(&path).unwrap();
        (Arrival { path, ext: "m4a".into(), download: Arc::default() }, f)
    }

    #[test]
    fn reads_as_it_arrives_then_ends_when_done() {
        let (a, mut w) = arrival("done");
        w.write_all(b"abc").unwrap();
        let (mut g, patient) = Growing::open(&a).unwrap();
        patient.store(true, Ordering::Relaxed);
        let dl = a.download.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            w.write_all(b"def").unwrap();
            std::thread::sleep(Duration::from_millis(100));
            w.write_all(b"g").unwrap();
            dl.finish(true);
        });
        let mut all = Vec::new();
        g.read_to_end(&mut all).unwrap();
        writer.join().unwrap();
        assert_eq!(all, b"abcdefg");
        // Done: its length is known now.
        assert_eq!(g.seek(SeekFrom::End(0)).unwrap(), 7);
    }

    #[test]
    fn a_failed_download_ends_the_song_where_it_got_to() {
        let (a, mut w) = arrival("failed");
        w.write_all(b"abc").unwrap();
        let (mut g, _) = Growing::open(&a).unwrap();
        assert!(g.seek(SeekFrom::End(0)).is_err(), "no end yet");
        g.seek(SeekFrom::Start(0)).unwrap();
        a.download.finish(false);
        let mut all = Vec::new();
        g.read_to_end(&mut all).unwrap();
        assert_eq!(all, b"abc");
    }

    #[test]
    fn found_once_enough_has_arrived() {
        let (a, mut w) = arrival("started");
        let dir = a.path.parent().unwrap();
        assert!(started(dir, "song").is_none());
        w.write_all(&vec![0u8; START_BYTES as usize]).unwrap();
        w.flush().unwrap();
        assert_eq!(started(dir, "song"), Some((a.path.clone(), "m4a".to_string())));
        assert!(started(dir, "other").is_none());
        let _ = std::fs::remove_dir_all(dir);
    }
}
