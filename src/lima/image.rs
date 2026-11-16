//! The Ubuntu cloud image, downloaded by bunker because a sandboxed `limactl start` has no network.
use crate::error::{BunkerError, Result};
use crate::output::ExitCode;
use crate::run::{Cmd, Runner};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

pub const SUMS_URL: &str = "https://cloud-images.ubuntu.com/releases/24.04/release/SHA256SUMS";
pub const IMAGE_NAME: &str = "ubuntu-24.04-server-cloudimg-arm64.img";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRef {
    pub path: PathBuf,
    pub sha256: String,
}

/// The current release's image in `dir`, downloading and verifying it when missing or stale.
/// Files are named by checksum prefix, so a new Ubuntu point release never overwrites an image an
/// existing VM was created from.
pub fn ensure_image(r: &dyn Runner, dir: &Path) -> Result<ImageRef> {
    let sums = r
        .run_ok(&Cmd::new("/usr/bin/curl").args(["-fsSL", SUMS_URL]))?
        .stdout_str();
    let sha = sums
        .lines()
        .find_map(|l| {
            let (sha, name) = l.split_once(' ')?;
            (name.trim_start_matches([' ', '*']) == IMAGE_NAME
                && sha.len() == 64
                && sha.chars().all(|c| c.is_ascii_hexdigit()))
            .then(|| sha.to_ascii_lowercase())
        })
        .ok_or_else(|| {
            BunkerError::new(
                "image_not_listed",
                ExitCode::Failure,
                format!("{IMAGE_NAME} is not in {SUMS_URL}"),
            )
        })?;
    crate::paths::ensure_private_dir(dir)?;
    let path = dir.join(format!("ubuntu-24.04-arm64-{}.img", &sha[..12]));
    let io = |what: &str, p: &Path, e| BunkerError::io(format!("cannot {what} {}", p.display()), e);
    if path.exists() && sha256_file(&path).map_err(|e| io("read", &path, e))? == sha {
        return Ok(ImageRef { path, sha256: sha });
    }
    let tmp = dir.join(format!(".download-{}.img", std::process::id()));
    let url = SUMS_URL.replace("SHA256SUMS", IMAGE_NAME);
    let fetched = r.run_ok(&Cmd::new("/usr/bin/curl").args([
        "-fL",
        "--retry",
        "3",
        "-o",
        &tmp.to_string_lossy(),
        &url,
    ]));
    let got = fetched.and_then(|_| sha256_file(&tmp).map_err(|e| io("read", &tmp, e)));
    match got {
        Ok(got) if got == sha => {
            std::fs::rename(&tmp, &path).map_err(|e| io("move", &path, e))?;
            Ok(ImageRef { path, sha256: sha })
        }
        Ok(got) => {
            let _ = std::fs::remove_file(&tmp);
            Err(BunkerError::new(
                "image_checksum",
                ExitCode::Failure,
                format!("{IMAGE_NAME} downloaded with sha256 {got}, expected {sha}"),
            )
            .hint("retry; if it repeats, the download is being tampered with or the mirror is mid-update"))
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Hex SHA-256 of a file, streamed.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::{Cmd, Output, RecordingRunner};

    const EMPTY_SHA: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    fn sums(sha: &str) -> String {
        format!("aaaa *ubuntu-24.04-server-cloudimg-amd64.img\n{sha} *{IMAGE_NAME}\n")
    }

    /// Answers `curl ... -o <path>` by writing `body` there.
    fn writes(body: &'static [u8]) -> impl Fn(&Cmd) -> Output + Send + Sync {
        move |c: &Cmd| {
            let i = c.args.iter().position(|a| a == "-o").unwrap();
            std::fs::write(&c.args[i + 1], body).unwrap();
            Output::ok("")
        }
    }

    #[test]
    fn sha256_of_empty_file() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("e"), b"").unwrap();
        assert_eq!(sha256_file(&d.path().join("e")).unwrap(), EMPTY_SHA);
    }

    #[test]
    fn downloads_verifies_and_reuses() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        r.respond(
            "/usr/bin/curl",
            &["-fsSL", SUMS_URL],
            Output::ok(sums(EMPTY_SHA)),
        );
        r.respond_with("/usr/bin/curl", &["-fL"], writes(b""));
        let img = ensure_image(&r, d.path()).unwrap();
        assert_eq!(img.sha256, EMPTY_SHA);
        assert!(img.path.starts_with(d.path()) && img.path.exists());
        assert_eq!(r.calls().len(), 2);
        let r2 = RecordingRunner::new();
        r2.respond(
            "/usr/bin/curl",
            &["-fsSL", SUMS_URL],
            Output::ok(sums(EMPTY_SHA)),
        );
        assert_eq!(ensure_image(&r2, d.path()).unwrap(), img);
        assert_eq!(
            r2.calls().len(),
            1,
            "a verified local image is not downloaded again"
        );
    }

    #[test]
    fn image_checksum_mismatch_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        r.respond(
            "/usr/bin/curl",
            &["-fsSL", SUMS_URL],
            Output::ok(sums(EMPTY_SHA)),
        );
        r.respond_with("/usr/bin/curl", &["-fL"], writes(b"tampered"));
        let e = ensure_image(&r, d.path()).unwrap_err();
        assert_eq!(e.code, "image_checksum");
        assert_eq!(
            std::fs::read_dir(d.path()).unwrap().count(),
            0,
            "the bad download is deleted"
        );
    }

    #[test]
    fn sums_without_the_image_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        let r = RecordingRunner::new();
        r.respond(
            "/usr/bin/curl",
            &["-fsSL", SUMS_URL],
            Output::ok("aaaa *other.img\n"),
        );
        assert_eq!(
            ensure_image(&r, d.path()).unwrap_err().code,
            "image_not_listed"
        );
    }
}
