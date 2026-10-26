//! Files a client sends with a prompt because the daemon cannot read its
//! disk, as when the daemon runs on another machine. Each is saved as
//! `<MALAC_HOME>/uploads/<uuid>/<name>` and attached from there; folders
//! older than a week are removed.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use malac_core::attach::human_size;

/// The largest file a prompt may carry, decoded.
pub const MAX_UPLOAD_BYTES: usize = 20 * 1024 * 1024;
/// The most all files of one prompt may come to, decoded.
pub const MAX_PROMPT_UPLOAD_BYTES: usize = 40 * 1024 * 1024;
/// The largest message, and frame, a client may send: a prompt whose files
/// reach the cap is about 54 MB of base64, which leaves room for its text
/// and images. Clients such as Node's `ws` send a message as one frame.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
/// Uploads are kept this long.
const KEEP: Duration = Duration::from_secs(7 * 24 * 3600);

/// Where uploads go.
pub fn dir() -> PathBuf {
    malac_core::config::home_dir().join("uploads")
}

/// Saves an uploaded file under `dir` and returns its path. `name` is the
/// client's file name; only its last component is used.
pub fn save(dir: &Path, name: &str, data: &str) -> Result<PathBuf> {
    let base = name
        .rsplit(['/', '\\'])
        .next()
        .filter(|n| !n.is_empty() && *n != "." && *n != "..")
        .with_context(|| format!("`{name}` is not a file name"))?;
    let data = data.trim();
    // Refused before decoding.
    let size = decoded_len(data);
    if size > MAX_UPLOAD_BYTES {
        bail!(too_large(base, size));
    }
    let bytes = BASE64
        .decode(data)
        .with_context(|| format!("{base} is not valid base64"))?;
    if bytes.len() > MAX_UPLOAD_BYTES {
        bail!(too_large(base, bytes.len()));
    }
    prune(dir, KEEP);
    let folder = dir.join(uuid::Uuid::now_v7().to_string());
    std::fs::create_dir_all(&folder)
        .with_context(|| format!("cannot create {}", folder.display()))?;
    let path = folder.join(base);
    std::fs::write(&path, bytes).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(path)
}

/// Refuses the files of a prompt, given as names and base64, when one is
/// over its cap or together they come to more than one prompt may carry.
/// Checked before any is decoded or saved, so a refused prompt leaves none.
pub fn check<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> Result<()> {
    let mut total = 0;
    for (name, data) in files {
        let size = decoded_len(data);
        if size > MAX_UPLOAD_BYTES {
            bail!(too_large(name, size));
        }
        total += size;
    }
    if total > MAX_PROMPT_UPLOAD_BYTES {
        bail!(
            "the files sent with this prompt come to {}, more than the {} one prompt may carry; \
             send some with the next one",
            human_size(total as u64),
            human_size(MAX_PROMPT_UPLOAD_BYTES as u64)
        );
    }
    Ok(())
}

/// How many bytes padded base64 decodes to: three for every four
/// characters, less the padding.
fn decoded_len(data: &str) -> usize {
    let data = data.trim();
    let padding = data.bytes().rev().take(2).filter(|b| *b == b'=').count();
    (data.len() / 4 * 3).saturating_sub(padding)
}

fn too_large(name: &str, bytes: usize) -> String {
    format!(
        "{name} is {}, more than the {} a file sent with a prompt may be",
        human_size(bytes as u64),
        human_size(MAX_UPLOAD_BYTES as u64)
    )
}

/// Removes upload folders last changed more than `keep` ago.
fn prune(dir: &Path, keep: Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|at| now.duration_since(at).ok())
            .is_some_and(|age| age > keep);
        if old && entry.path().is_dir() {
            if let Err(err) = std::fs::remove_dir_all(entry.path()) {
                tracing::warn!("cannot remove an old upload: {err}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_under_the_file_name_only() {
        let dir = tempfile::tempdir().unwrap();
        let data = BASE64.encode("hello\n");
        let path = save(dir.path(), "../../etc/notes.txt", &data).unwrap();
        assert_eq!(path.file_name().unwrap(), "notes.txt");
        assert_eq!(path.parent().unwrap().parent().unwrap(), dir.path());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\n");
        let windows = save(dir.path(), r"C:\Users\me\shot.png", &data).unwrap();
        assert_eq!(windows.file_name().unwrap(), "shot.png");
        assert!(save(dir.path(), "..", &data).is_err());
        assert!(save(dir.path(), "a.txt", "not base64!").is_err());
    }

    #[test]
    fn refuses_files_over_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let data = BASE64.encode(vec![0u8; MAX_UPLOAD_BYTES + 1]);
        let err = save(dir.path(), "big.bin", &data).unwrap_err().to_string();
        assert!(
            err.contains("big.bin is 20.0 MB, more than the 20.0 MB"),
            "{err}"
        );
        assert!(
            std::fs::read_dir(dir.path()).unwrap().next().is_none(),
            "nothing saved"
        );
    }

    #[test]
    fn measures_base64_without_decoding() {
        for len in 0..10 {
            let data = BASE64.encode(vec![7u8; len]);
            assert_eq!(decoded_len(&data), len, "{data}");
        }
    }

    #[test]
    fn refuses_prompts_whose_files_come_to_too_much() {
        // Measured, not decoded, so the contents need not be real.
        let file = "A".repeat(MAX_UPLOAD_BYTES / 3 * 4);
        let most = MAX_PROMPT_UPLOAD_BYTES - 2 * decoded_len(&file);
        let rest = "A".repeat(most / 3 * 4);
        let fits = [
            ("a", file.as_str()),
            ("b", file.as_str()),
            ("c", rest.as_str()),
        ];
        check(fits).unwrap();
        let over = [
            ("a", file.as_str()),
            ("b", file.as_str()),
            ("c", "AAAAAAAA"),
        ];
        let err = check(over).unwrap_err().to_string();
        assert!(
            err.contains("come to 40.0 MB, more than the 40.0 MB one prompt may carry"),
            "{err}"
        );
        let big = format!("{file}AAAA");
        let err = check([("ok.txt", "AAAA"), ("big.bin", big.as_str())])
            .unwrap_err()
            .to_string();
        assert!(err.starts_with("big.bin is 20.0 MB"), "{err}");
        check([]).unwrap();
    }

    #[test]
    fn prunes_old_folders() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), "a.txt", &BASE64.encode("a")).unwrap();
        prune(dir.path(), Duration::from_secs(3600));
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "a fresh one stays"
        );
        std::thread::sleep(Duration::from_millis(20));
        prune(dir.path(), Duration::from_millis(1));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
