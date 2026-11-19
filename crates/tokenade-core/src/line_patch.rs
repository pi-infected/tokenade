//! Same-length patches written in place into a file another process may be
//! appending to — an agent's live transcript.
//!
//! Rewriting such a file (read, change, write a temp copy, rename it over the
//! original) loses every line the owner appends between the read and the
//! rename, gives the file a new identity under readers that follow it by
//! offset, and on Windows fails outright while the owner holds it open without
//! `FILE_SHARE_DELETE` (#12). A patch that keeps its byte length needs none of
//! that: it is written at its offset, and only while the bytes there are still
//! the ones it was computed from. Appends past the patched range are never
//! touched, and a range someone else changed is left as they wrote it.

use std::io::{Read, Seek, SeekFrom, Write};

/// Replace `old` with `new` at `offset`. Both have the same length.
pub struct Patch {
    pub offset: u64,
    pub old: Vec<u8>,
    pub new: Vec<u8>,
}

/// Apply each patch whose range still holds its `old` bytes. Returns, per
/// patch, whether it was written. A patch whose lengths differ, or whose
/// range is gone or changed, is skipped, never forced.
///
/// What it does not cover: a truncation landing between one patch's check and
/// its write (the file would regrow with that patch's bytes — agents only
/// append), and a power loss tearing the one patch being written.
pub fn apply(path: &std::path::Path, patches: &[Patch]) -> std::io::Result<Vec<bool>> {
    let mut applied = vec![false; patches.len()];
    if patches.is_empty() {
        return Ok(applied);
    }
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    let mut buf = Vec::new();
    for (p, done) in patches.iter().zip(applied.iter_mut()) {
        if p.old.len() != p.new.len() {
            continue;
        }
        buf.resize(p.old.len(), 0);
        f.seek(SeekFrom::Start(p.offset))?;
        if f.read_exact(&mut buf).is_err() || buf != p.old {
            continue;
        }
        f.seek(SeekFrom::Start(p.offset))?;
        f.write_all(&p.new)?;
        *done = true;
    }
    if applied.contains(&true) {
        f.sync_data()?;
    }
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(offset: u64, old: &str, new: &str) -> Patch {
        Patch {
            offset,
            old: old.into(),
            new: new.into(),
        }
    }

    #[test]
    fn only_ranges_still_holding_their_bytes_are_written() {
        let p = std::env::temp_dir().join(format!("tokenade-line-patch-{}", std::process::id()));
        std::fs::write(&p, "aaaa\nbbbb\ncccc\n").unwrap();
        #[cfg(unix)]
        let ino = std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&p).unwrap());
        // Someone else changed line 2 and appended a line after the plan was made.
        std::fs::write(&p, "aaaa\nBBBB\ncccc\n").unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&p)
            .unwrap()
            .write_all(b"dddd\n")
            .unwrap();
        let plan = [
            patch(0, "aaaa", "xx  "),
            patch(5, "bbbb", "yy  "),
            patch(10, "cccc", "zz  "),
            patch(99, "eeee", "ww  "),
        ];
        assert_eq!(apply(&p, &plan).unwrap(), [true, false, true, false]);
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "xx  \nBBBB\nzz  \ndddd\n"
        );
        #[cfg(unix)]
        assert_eq!(
            std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(&p).unwrap()),
            ino
        );
        let _ = std::fs::remove_file(&p);
    }
}
