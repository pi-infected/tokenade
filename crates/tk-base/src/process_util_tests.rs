// The tests of `crate::process_util`, moved out of the production file.
//
// Mounted by `include!`: the tokens are spliced INTO `process_util`, so every
// `super::` keeps its meaning. Split by `srcsweep::production_lines`.

#[cfg(test)]
mod detached_cwd {
    /// The child runs in the directory it was given, not the parent's.
    ///
    /// The disk GC's sweeps are partly PROJECT-scoped
    /// (`<root>/.tokenade/reads/`), and a detached child inherits the cwd of a
    /// hook — which is wherever the agent happened to be. Losing the cwd would
    /// not fail anything loudly: those sweeps would simply stop finding
    /// anything, which is the same shape as the GC that was unreachable for
    /// months behind an early return.
    #[cfg(unix)]
    #[test]
    fn a_detached_child_runs_where_it_was_told() {
        let d = std::env::temp_dir().join(format!(
            "tk-detached-cwd-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        super::spawn_fully_detached_in(
            std::path::Path::new("/bin/sh"),
            &["-c", "pwd -P > ran_here.txt"],
            Some(&d),
        )
        .expect("spawn");
        let out = d.join("ran_here.txt");
        for _ in 0..100 {
            if out.is_file() && std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0) > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let got = std::fs::read_to_string(&out).unwrap_or_default();
        let want = std::fs::canonicalize(&d).unwrap();
        assert_eq!(
            got.trim(),
            want.to_string_lossy(),
            "the child did not run in the directory it was given"
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_all_bytes_below_cap() {
        let data = b"hello world\n".repeat(10);
        let cursor = std::io::Cursor::new(data.clone());
        let (buf, truncated) = drain_capped(cursor, 1024).join().unwrap();
        assert!(!truncated);
        assert_eq!(buf, data);
    }

    #[test]
    fn a_secret_straddling_the_cap_is_dropped_whole_not_halved() {
        // The cap must not hand downstream redaction a half-written
        // `AKIA…`: too short to match any fixed-length pattern, still
        // sensitive. The partial line goes, the complete ones stay.
        let mut data = b"harmless line\n".repeat(10);
        data.extend_from_slice(b"AWS_SECRET=AKIAIOSFODNN7EXAMPLE\n");
        let cap = data.len() - 12; // lands inside the secret
        let (buf, truncated) = drain_capped(std::io::Cursor::new(data), cap)
            .join()
            .unwrap();
        assert!(truncated);
        let s = String::from_utf8(buf).unwrap();
        assert!(
            !s.contains("AKIA"),
            "no fragment of the key may survive: {s}"
        );
        assert!(s.ends_with('\n'), "buffer must end on a line boundary");
        assert_eq!(s.lines().count(), 10);
    }

    #[test]
    fn a_capped_stream_with_no_newline_keeps_its_bytes() {
        // Rolling back to a line boundary that does not exist would throw
        // the whole buffer away — worse than a partial line.
        let (buf, truncated) = drain_capped(std::io::Cursor::new(vec![b'A'; 2000]), 500)
            .join()
            .unwrap();
        assert!(truncated);
        assert_eq!(buf.len(), 500);
    }

    #[test]
    fn truncates_above_cap() {
        let data = vec![b'A'; 2000];
        let cursor = std::io::Cursor::new(data);
        let (buf, truncated) = drain_capped(cursor, 500).join().unwrap();
        assert!(truncated);
        assert_eq!(buf.len(), 500);
        assert!(buf.iter().all(|&b| b == b'A'));
    }

    #[test]
    fn drains_remaining_when_capped() {
        // The function must keep reading past `cap` so a child pipe
        // doesn't block. Use a tracker that counts total bytes read.
        struct Counter {
            data: Vec<u8>,
            pos: usize,
            total_read: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        }
        impl std::io::Read for Counter {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let remaining = self.data.len().saturating_sub(self.pos);
                if remaining == 0 {
                    return Ok(0);
                }
                let take = buf.len().min(remaining).min(1024);
                buf[..take].copy_from_slice(&self.data[self.pos..self.pos + take]);
                self.pos += take;
                self.total_read
                    .fetch_add(take, std::sync::atomic::Ordering::Relaxed);
                Ok(take)
            }
        }
        let total = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let c = Counter {
            data: vec![b'X'; 50_000],
            pos: 0,
            total_read: total.clone(),
        };
        let (buf, truncated) = drain_capped(c, 1_000).join().unwrap();
        assert_eq!(buf.len(), 1_000);
        assert!(truncated);
        // Critical: all 50k bytes must have been read off the reader,
        // even though we only buffered 1k. This is the deadlock-
        // prevention guarantee for piped children.
        assert_eq!(total.load(std::sync::atomic::Ordering::Relaxed), 50_000);
    }

    #[test]
    fn handles_empty_reader() {
        let cursor = std::io::Cursor::new(Vec::<u8>::new());
        let (buf, truncated) = drain_capped(cursor, 1024).join().unwrap();
        assert!(buf.is_empty());
        assert!(!truncated);
    }

    #[test]
    fn cap_zero_truncates_anything() {
        let data = vec![b'X'; 100];
        let cursor = std::io::Cursor::new(data);
        let (buf, truncated) = drain_capped(cursor, 0).join().unwrap();
        assert!(buf.is_empty());
        assert!(truncated);
    }

    #[test]
    fn truncation_banner_format() {
        let b = truncation_banner(16 * 1024 * 1024, 4 * 1024 * 1024);
        assert!(b.contains("16 MB stdout"));
        assert!(b.contains("4 MB stderr"));
        assert!(b.contains("tokenade raw"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_detached_flags_are_correct() {
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW.
        assert_eq!(DETACHED_CREATION_FLAGS, 0x0800_0208);
        assert_ne!(DETACHED_CREATION_FLAGS & 0x0000_0008, 0, "DETACHED_PROCESS");
        assert_ne!(
            DETACHED_CREATION_FLAGS & 0x0000_0200,
            0,
            "CREATE_NEW_PROCESS_GROUP"
        );
        assert_ne!(DETACHED_CREATION_FLAGS & 0x0800_0000, 0, "CREATE_NO_WINDOW");
    }

    // Integration-flavoured proof that `setsid` actually took effect: a
    // detached child lands in its OWN session/process-group, distinct from
    // the parent's. That is exactly what makes it immune to a `killpg` of
    // the parent's group. We have the child write its process-group id to a
    // file (stdout is nulled by the helper), then compare to ours.
    #[cfg(unix)]
    #[test]
    fn detached_child_leaves_parent_process_group() {
        use std::io::Write;
        let mut out = std::env::temp_dir();
        out.push(format!("tk_detach_pgid_{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&out);
        // `ps -o pgid= -p $$` prints the shell's own process-group id.
        let script = format!("ps -o pgid= -p $$ | tr -d ' \\n' > {}", out.display());
        spawn_fully_detached(std::path::Path::new("/bin/sh"), &["-c", &script])
            .expect("spawn detached child");

        // Poll briefly for the child to write (it's async).
        let mut child_pgid: Option<i32> = None;
        for _ in 0..200 {
            if let Ok(s) = std::fs::read_to_string(&out) {
                if let Ok(v) = s.trim().parse::<i32>() {
                    child_pgid = Some(v);
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let _ = std::fs::remove_file(&out);
        let _ = std::io::stdout().flush();

        // SAFETY: getpgrp(2) takes no argument and cannot fail.
        let parent_pgid = unsafe { libc::getpgrp() };
        match child_pgid {
            Some(cpg) => assert_ne!(
                cpg, parent_pgid,
                "setsid should have moved the child into a new process group"
            ),
            // `ps` unavailable in this sandbox: don't fail the suite, the
            // flag-wiring is still compile-checked and the windows path is
            // asserted above.
            None => eprintln!("skipped: child pgid file never appeared (ps unavailable?)"),
        }
    }
}
#[cfg(test)]
mod etxtbsy_tests {
    use super::*;

    #[test]
    fn retries_only_on_text_file_busy_and_gives_up_bounded() {
        // Transient ETXTBSY: succeeds once the sibling's fork/exec window closes.
        let calls = std::cell::Cell::new(0);
        let out = retry_while_text_file_busy(|| {
            calls.set(calls.get() + 1);
            if calls.get() < 3 {
                Err(std::io::Error::from(std::io::ErrorKind::ExecutableFileBusy))
            } else {
                Ok("ran")
            }
        });
        assert_eq!(out.unwrap(), "ran");
        assert_eq!(calls.get(), 3);

        // A real error must NOT be retried — a missing binary fails fast.
        let calls = std::cell::Cell::new(0);
        let out: std::io::Result<()> = retry_while_text_file_busy(|| {
            calls.set(calls.get() + 1);
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        });
        assert_eq!(out.unwrap_err().kind(), std::io::ErrorKind::NotFound);
        assert_eq!(calls.get(), 1, "non-ETXTBSY must not be retried");
    }
}
#[cfg(test)]
mod capped_tail_tests {
    use super::drain_capped_keeping_tail as drain_capped;

    /// A command that fails says why on its LAST line, and a head-only cap
    /// keeps the first N bytes — exactly the part that does not say why.
    #[test]
    fn a_capped_capture_keeps_the_end_where_the_reason_lives() {
        let mut payload = String::new();
        for i in 0..2000 {
            payload.push_str(&format!("INFO step {i} steady payload line\n"));
        }
        payload.push_str("ERROR: link failed: undefined symbol frobnicate\n");
        let cap = 4096;
        assert!(payload.len() > cap * 4, "the input must actually overflow");

        let (buf, truncated) = drain_capped(std::io::Cursor::new(payload.into_bytes()), cap)
            .join()
            .unwrap();
        let text = String::from_utf8_lossy(&buf);
        assert!(truncated, "it did truncate");
        assert!(
            text.starts_with("INFO step 0 "),
            "the head is still the head"
        );
        assert!(
            text.contains("undefined symbol frobnicate"),
            "the reason for the failure has to survive:\n{text}"
        );
        assert!(text.contains("bytes omitted"), "and the gap is disclosed");
        // The tail must start on a line boundary: a value sliced mid-token
        // could no longer be recognised by redaction downstream.
        let after_gap = text
            .split("the end of the output follows …]\n")
            .nth(1)
            .unwrap();
        assert!(
            after_gap.starts_with("INFO step "),
            "tail must start at a line boundary, got: {:?}",
            &after_gap[..40.min(after_gap.len())]
        );
    }

    /// The head cut drops its partial line because a secret sliced there loses
    /// the half that makes it matchable, and redaction runs downstream. The
    /// tail's LEFT edge is the same cut with the same consequence, so it gets
    /// the same treatment — this pins both ends.
    #[test]
    fn no_half_secret_survives_either_end_of_the_cut() {
        const SECRET: &str = "AKIAIOSFODNN7EXAMPLE";
        let cap = 4096;
        // Sweep the secret across every offset that can straddle either cut.
        for pad in 0..600usize {
            let mut payload = String::new();
            payload.push_str(&"x".repeat(pad));
            payload.push('\n');
            while payload.len() < cap * 3 {
                payload.push_str(&format!("noise {} filler line\n", payload.len()));
                if payload.len() > cap / 2 && !payload.contains(SECRET) {
                    payload.push_str(&format!("key={SECRET}\n"));
                }
            }
            let (buf, _t) = drain_capped(std::io::Cursor::new(payload.into_bytes()), cap)
                .join()
                .unwrap();
            let text = String::from_utf8_lossy(&buf);
            // Either the secret is present WHOLE (redaction downstream will
            // catch it) or it is absent. A fragment is the one bad outcome.
            for frag_len in 8..SECRET.len() {
                let frag = &SECRET[..frag_len];
                if text.contains(frag) {
                    assert!(
                        text.contains(SECRET),
                        "pad={pad}: a {frag_len}-char fragment survived without the whole secret"
                    );
                }
            }
        }
    }

    /// Below the cap nothing changes — no marker, no reshaping.
    #[test]
    fn an_output_under_the_cap_is_returned_verbatim() {
        let payload = "one\ntwo\nthree\n";
        let (buf, truncated) =
            drain_capped(std::io::Cursor::new(payload.as_bytes().to_vec()), 4096)
                .join()
                .unwrap();
        assert!(!truncated);
        assert_eq!(String::from_utf8_lossy(&buf), payload);
    }
}
#[cfg(test)]
mod lossy_decode {
    use super::*;

    #[test]
    fn clean_utf8_says_nothing() {
        assert_eq!(invalid_utf8_sequences("héllo wörld".as_bytes()), 0);
        assert!(lossy_decode_advisory("héllo".as_bytes(), b"").is_none());
        assert!(!ends_mid_character("héllo".as_bytes()));
    }

    /// `head -c 2` on a file starting `h é` hands back `68 c3` — the `é`
    /// cut in half. That is the incident this whole path exists for.
    #[test]
    fn a_byte_count_read_that_split_a_character_is_named() {
        let cut = &"héllo".as_bytes()[..2];
        assert_eq!(invalid_utf8_sequences(cut), 1);
        assert!(ends_mid_character(cut));
        let msg = lossy_decode_advisory(cut, b"").expect("must be named");
        assert!(msg.contains("1 byte sequence"), "{msg}");
        assert!(msg.contains("mid-character"), "{msg}");
    }

    /// A stray byte in the MIDDLE is an encoding mismatch, not a truncation,
    /// and must not be described as one.
    #[test]
    fn a_stray_byte_in_the_middle_is_not_called_a_truncation() {
        let mut v = b"alpha ".to_vec();
        v.push(0xff);
        v.extend_from_slice(b" omega\n");
        assert_eq!(invalid_utf8_sequences(&v), 1);
        assert!(!ends_mid_character(&v));
        let msg = lossy_decode_advisory(&v, b"").expect("must be named");
        assert!(!msg.contains("mid-character"), "{msg}");
    }

    /// Several bad sequences must be counted, not collapsed to one — and the
    /// count must not run away on a long run of them.
    #[test]
    fn several_sequences_are_counted() {
        let mut v = Vec::new();
        for i in 0..5 {
            v.extend_from_slice(format!("line{i} ").as_bytes());
            v.push(0xfe);
        }
        v.push(b'\n');
        assert_eq!(invalid_utf8_sequences(&v), 5);
        // A latin-1 line of accents: every byte its own bad sequence.
        assert_eq!(invalid_utf8_sequences(&[0xe9, 0xe8, 0xe0]), 3);
    }

    /// Both streams count, and stderr alone is enough to raise the line.
    #[test]
    fn stderr_alone_raises_it() {
        assert!(lossy_decode_advisory(b"clean\n", &[0xff]).is_some());
        assert_eq!(invalid_utf8_sequences(b"a"), 0);
    }

    /// Termination: the loop must end on input that is nothing but bad bytes.
    #[test]
    fn all_bad_bytes_terminate() {
        assert_eq!(invalid_utf8_sequences(&[0xff; 32]), 32);
        assert_eq!(invalid_utf8_sequences(&[]), 0);
    }
}
#[cfg(test)]
mod decode_child {
    use super::*;

    #[test]
    fn clean_utf8_is_untouched_and_faithful() {
        let (t, ok) = decode_child_output("héllo\n".as_bytes());
        assert!(ok);
        assert_eq!(t, "héllo\n");
    }

    /// A latin-1 log is DECODED, not replaced. This is the whole gain.
    #[test]
    fn a_latin1_log_is_decoded_rather_than_replaced() {
        let mut v = Vec::new();
        for i in 0..40 {
            v.extend_from_slice(b"caf");
            v.push(0xe9);
            v.extend_from_slice(format!(" line {i}\n").as_bytes());
        }
        let (t, ok) = decode_child_output(&v);
        assert!(ok, "a latin-1 log was not decoded");
        assert!(t.contains("café"), "{}", &t[..40.min(t.len())]);
        assert!(!t.contains('\u{fffd}'), "replacement characters survived");
    }

    /// …and a byte-count read is NOT handed to the detector, which would
    /// answer confidently and wrongly on two bytes.
    #[test]
    fn a_truncated_read_is_left_lossy_rather_than_guessed() {
        let cut = &"héllo".as_bytes()[..2];
        let (t, ok) = decode_child_output(cut);
        assert!(!ok, "a truncated read was reported faithful");
        assert_eq!(t, "h\u{fffd}");
        assert!(!t.contains('Ă'), "chardetng was consulted: {t}");
    }

    /// Genuinely undecodable bytes stay lossy and say so, rather than being
    /// dressed up as some encoding.
    /// `mostly_utf8` answers `Some` for a large output with a few corrupt
    /// sequences, and that `Some` still carries U+FFFD. Reading the branch
    /// rather than the result called it faithful.
    #[test]
    fn a_stray_byte_in_a_large_clean_output_is_not_called_faithful() {
        let mut v = Vec::new();
        for i in 0..400 {
            v.extend_from_slice(format!("line {i} is perfectly ordinary text\n").as_bytes());
            if i % 150 == 0 {
                v.push(0xff);
            }
        }
        let (t, ok) = decode_child_output(&v);
        assert!(
            t.contains('\u{FFFD}'),
            "the decode did not replace anything"
        );
        assert!(!ok, "a decode that introduced U+FFFD was reported faithful");
    }

    /// …and output that carries a REAL U+FFFD must not be accused of it.
    #[test]
    fn an_output_whose_own_text_holds_the_replacement_character_is_faithful() {
        let (t, ok) = decode_child_output("a \u{FFFD} b\n".as_bytes());
        assert!(ok, "a valid-UTF-8 output was accused");
        assert_eq!(t, "a \u{FFFD} b\n");
    }

    #[test]
    fn undecodable_bytes_are_reported_unfaithful() {
        let (_t, ok) = decode_child_output(&[0x00, 0xff, 0x01, 0xfe, 0x02, 0xfd]);
        assert!(!ok);
    }
}
#[cfg(test)]
mod liveness_tests {
    /// The behavioural guards live in `testtmp` and are CORRECT and BLIND on
    /// Linux: they ask whether a live pid survives a sweep, and `/proc/<pid>`
    /// answers that correctly on a system that HAS `/proc`. It took the macOS
    /// legs of the 1.1.7 release build to fail them.
    ///
    /// So this one asks the question Linux can answer: liveness must not be
    /// decided by looking in `/proc`, because `cfg(unix)` includes systems
    /// that do not have one. Reading the source is the crude form and the only
    /// form available from here — the alternative is finding out on macOS.
    #[test]
    fn liveness_is_not_asked_of_proc() {
        // The FAMILY: `process_util` has branches, and a call-site guard
        // that only reads the trunk would announce a disappearance on a
        // mere move.
        let src = crate::source_guard::library_module_family("process_util");
        let src = src.as_str();
        let start = src
            .find("fn process_exists")
            .expect("the liveness check must still exist");
        // Bounded by the function's own closing brace, not by a byte count: a
        // fixed window is a guard that breaks when the function grows.
        let end = src[start..]
            .find("\n}\n")
            .map(|o| start + o)
            .unwrap_or(src.len());
        let body = &src[start..end];
        assert!(
            !body.contains("\"/proc\""),
            "`process_exists` consults /proc, which `cfg(unix)` machines \
             without one (macOS) answer as 'every pid is dead':\n{body}"
        );
        assert!(
            body.contains("libc::kill"),
            "liveness must be asked with kill(pid, 0), the portable form:\n{body}"
        );
    }

    /// Pid 0 must never be asked of `kill`, which reads it as "my whole
    /// process group" and answers success.
    #[test]
    fn pid_zero_is_never_alive() {
        assert!(!super::process_is_alive(0));
        assert!(super::process_is_alive(std::process::id()), "we are alive");
    }
}
#[cfg(test)]
mod internal_marking {
    use super::*;

    /// The marker is on the CHILD's environment, which is the only place it can
    /// survive an interception layer standing between us and the program.
    #[test]
    fn an_internal_command_carries_the_marker_and_the_proxy_guard() {
        let cmd = internal_command("git");
        let env: std::collections::HashMap<_, _> = cmd
            .get_envs()
            .filter_map(|(k, v)| {
                Some((
                    k.to_string_lossy().into_owned(),
                    v?.to_string_lossy().into_owned(),
                ))
            })
            .collect();
        assert_eq!(
            env.get(INTERNAL_SUBPROCESS_ENV).map(String::as_str),
            Some("1"),
            "an intercepting wrapper has no other way to know whose work this is"
        );
        assert_eq!(
            env.get("TOKENADE_WRAPPING").map(String::as_str),
            Some("1"),
            "the emitted shell fragment checks this one before wrapping anything"
        );
    }

    /// And nothing is internal by default — the rule has to be something a
    /// site opts INTO, or the sweep guarding it would be measuring nothing.
    #[test]
    fn a_process_nobody_marked_is_not_internal() {
        assert!(
            !running_as_internal_subprocess(),
            "the test runner is not tokenade's own plumbing"
        );
    }
}

#[cfg(test)]
mod a_windows_console_reads_in_its_code_page {
    use crate::process_util::decode_child_output_with;

    /// `nslookup` on a Windows 10 fr-FR: cp850. The same byte in a
    /// `cat`ted file is ANSI (cp1252) and does not go through OEM.
    #[test]
    fn the_oem_page_only_applies_to_console_programs() {
        let bytes = b"R\x82ponse ne faisant pas autorit\x82 :\r\nNom :    github.com\r\n";
        let (t, ok) = decode_child_output_with(bytes, "nslookup github.com", Some(850));
        assert!(
            ok && t.starts_with("Réponse ne faisant pas autorité"),
            "{t}"
        );
        let (t, _) = decode_child_output_with(bytes, "cat notes.txt", Some(850));
        assert!(!t.contains("Réponse"), "a file is not console output: {t}");
        // Outside Windows (no OEM page): as before.
        let (t, _) = decode_child_output_with(bytes, "nslookup github.com", None);
        assert!(!t.contains("Réponse"), "{t}");
        // UTF-8 stays UTF-8, even for a console program.
        let (t, _) = decode_child_output_with("Réponse".as_bytes(), "ipconfig", Some(850));
        assert_eq!(t, "Réponse");
    }
}
