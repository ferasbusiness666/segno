//! Small HTTP helpers (thumbnails and the one-time downloader setup).

use std::io::{Read, Write};
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        let b = ureq::Agent::config_builder()
            .user_agent(concat!("Segno/", env!("CARGO_PKG_VERSION"), " (https://segnomusic.pages.dev)"))
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .max_redirects(10);
        #[cfg(windows)]
        let b = b.tls_config(
            ureq::tls::TlsConfig::builder()
                .provider(ureq::tls::TlsProvider::NativeTls)
                .root_certs(ureq::tls::RootCerts::PlatformVerifier)
                .build(),
        );
        b.build().into()
    })
}

pub fn get_bytes(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let mut resp = agent().get(url).call().map_err(|e| e.to_string())?;
    resp.body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|e| e.to_string())
}

/// Stream a file to disk with progress (bytes so far, total if known).
pub fn download(url: &str, dest: &Path, mut progress: impl FnMut(u64, Option<u64>)) -> Result<(), String> {
    let mut resp = agent().get(url).call().map_err(|e| e.to_string())?;
    let total = resp.body().content_length();
    let tmp = dest.with_extension("part");
    let mut out = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    let mut reader = resp.body_mut().with_config().limit(1 << 31).reader();
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    let mut last_report = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        if done - last_report > 256 * 1024 {
            last_report = done;
            progress(done, total);
        }
    }
    out.flush().map_err(|e| e.to_string())?;
    drop(out);
    if let Some(t) = total {
        if done < t {
            let _ = std::fs::remove_file(&tmp);
            return Err("The download stopped early.".into());
        }
    }
    let _ = std::fs::remove_file(dest);
    std::fs::rename(&tmp, dest).map_err(|e| e.to_string())?;
    progress(done, total);
    Ok(())
}

/// A web page as text, asked for the way a browser would (some sites serve bots a bare page).
pub fn get_page(url: &str) -> Result<String, String> {
    let mut resp = agent()
        .get(url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36",
        )
        .header("Accept-Language", "en")
        .call()
        .map_err(|e| e.to_string())?;
    resp.body_mut()
        .with_config()
        .limit(16 << 20)
        .read_to_string()
        .map_err(|e| e.to_string())
}

/// A small text answer from an API. A plain "not found" is `Ok(None)`; anything else that
/// goes wrong (no internet, a server error) is an `Err`.
pub fn get_text_opt(url: &str) -> Result<Option<String>, String> {
    match agent().get(url).call() {
        Ok(mut resp) => resp
            .body_mut()
            .with_config()
            .limit(4 << 20)
            .read_to_string()
            .map(Some)
            .map_err(|e| e.to_string()),
        Err(ureq::Error::StatusCode(404)) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}
