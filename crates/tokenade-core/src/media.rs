//! Audio / video handling.
//!
//! Media is not text, and the text path is actively harmful on it: a media file
//! is not a `docs::Format`, so it used to fall through to the UTF-8 text
//! compactor, which lossy-decoded the bytes into replacement characters and
//! INFLATED them. Measured before this module existed: a 30 s / 240 KB mp3
//! served **107 138 tokens** of garbage, and a 10 s mp4 served 58 450 — more
//! output than input, for zero information.
//!
//! What a model actually needs from a media file is (a) what it IS (duration,
//! codecs, resolution, tracks) and (b) what is SAID in it. (a) is ~30 tokens of
//! `ffprobe`; (b) is a transcript, which is ordinary text and therefore flows
//! into the machinery the rest of tokenade already has: under the tout-vs-RAG
//! threshold it is served whole, above it the agent asks it questions.
//!
//! `ffmpeg`/`ffprobe`/`whisper` are OPTIONAL, never runtime dependencies (the
//! poppler/soffice precedent: test oracles only). Without them this module
//! still identifies the file and refuses to inflate it — which is the bulk of
//! the win — and tells the agent which binary would unlock more.

use std::path::Path;

/// What kind of media a file is. Deliberately coarse: the serve path differs
/// between "has pictures" and "does not", nothing finer matters here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Audio,
    Video,
}

impl MediaKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MediaKind::Audio => "audio",
            MediaKind::Video => "video",
        }
    }
}

/// Extensions we recognise. Mirrors the list `disclose::parsing_hint_for`
/// already used to suggest `ffprobe` — same vocabulary, now actually wired.
fn kind_from_ext(ext: &str) -> Option<MediaKind> {
    let e = ext.trim_start_matches('.').to_ascii_lowercase();
    match e.as_str() {
        // Audio: lossy, lossless, speech, tracker and playlist-adjacent
        // containers an agent may plausibly hand us.
        "mp3" | "wav" | "wave" | "ogg" | "oga" | "flac" | "m4a" | "m4b" | "aac" | "opus"
        | "amr" | "aiff" | "aif" | "aifc" | "wma" | "alac" | "ape" | "wv" | "mpc" | "ac3"
        | "au" | "snd" | "ra" | "caf" | "spx" | "voc" | "gsm" | "dsf" | "dff" | "mka" | "weba"
        | "3ga" | "mp2" | "mpga" => Some(MediaKind::Audio),
        // Video containers.
        "mp4" | "webm" | "mkv" | "mov" | "avi" | "flv" | "wmv" | "m4v" | "mpg" | "mpeg" | "mpe"
        | "3gp" | "3g2" | "m2ts" | "m2v" | "mxf" | "ogv" | "asf" | "rm" | "rmvb" | "vob"
        | "divx" | "f4v" | "swf" | "y4m" | "qt" | "dv" | "amv" | "roq" | "nsv" => {
            Some(MediaKind::Video)
        }
        // DELIBERATELY ABSENT — extensions that collide with source files our
        // users actually have. `.ts` is TypeScript far more often than it is an
        // MPEG transport stream (measured on this machine: 22 583 `.ts`, ALL
        // TypeScript, none with a 0x47 sync byte), and `.dts` is Device Tree
        // Source in kernel work. Claiming them would route source code into the
        // media path and refuse to read it — a catastrophic false positive.
        // A genuine `.ts`/`.dts` stream is still handled: it reaches the caller
        // as a normal binary, which is strictly better than breaking source.
        //
        // `.mts` belongs in this group and was in the video list above until
        // 2026-08-27. It is AVCHD, and it is ALSO TypeScript's ESM module
        // extension — the sibling of `.cts`, both added to TypeScript long
        // after AVCHD named the container. Measured on this machine: 19 574
        // `.mts` files, ZERO with a 0x47 sync byte. Every one of them came
        // back as `[tokenade:video …]` with an empty transcript, the whole
        // source lost, and the read spent a local whisper run getting there.
        // Unlike `.ts` it does NOT drop to "just a binary": `detect_media`
        // asks the header, because a name this narrow makes the sync byte
        // trustworthy in a way a bare byte-0 check never is.
        _ => None,
    }
}

/// Identify media from a header. Extension first (cheap and usually right),
/// then magic bytes so a file whose name lies — or has no extension — is still
/// caught before the text path can inflate it.
///
/// Container magic cannot always separate audio from video (an `ftyp` MP4 box
/// or a Matroska/Ogg/RIFF container may hold either), so a container-only match
/// is reported as [`MediaKind::Video`]: the serve path is the same, and only
/// `ffprobe` can answer definitively — which it does when present.
pub fn detect_media(header: &[u8], filename: &str) -> Option<MediaKind> {
    // `rsplit('.').next()` yields the WHOLE name when there is no dot, so an
    // extensionless file called `rm`, `dv`, `qt`, `au` or `snd` was classified
    // as media and denied with "reading its bytes as text is meaningless".
    if let Some(ext) = filename.rsplit_once('.').map(|(_, e)| e) {
        if let Some(k) = kind_from_ext(ext) {
            return Some(k);
        }
        // The one extension that is genuinely both. The bare MPEG-TS sync
        // check this function refuses below (every text file starting with
        // "G" opens on 0x47) becomes trustworthy once the NAME has already
        // narrowed the population to `.mts`: a TypeScript module is text, an
        // AVCHD clip is not.
        if ext.eq_ignore_ascii_case("mts")
            && header.first() == Some(&0x47)
            && !crate::textshape::looks_like_text(header)
        {
            return Some(MediaKind::Video);
        }
    }
    if header.len() < 12 {
        return None;
    }
    // Four signatures below are printable ASCII with NOTHING after them to
    // check — `ID3`, `fLaC`, `.snd`, `MAC `, `wvpk`, `MPCK`, `MP+`. Unlike
    // `RIFF`/`FORM`/`ftyp`, which validate a second field and so reject
    // "RIFF the river runs" on their own, these claim any sentence that opens
    // with those letters. `MAC host=127.0.0.1` was routed to local whisper
    // transcription on the shipped 1.1.6 binary for exactly this reason.
    //
    // Every real one of these carries a version, size or block header in the
    // bytes that follow, and those are NUL or non-UTF-8 within the 12 bytes
    // this function is given. So a header that is unambiguously text is not
    // one of them.
    let ascii_lookalike = crate::textshape::looks_like_text(header);
    // ID3-tagged or raw MPEG audio frame sync (0xFF Ex/Fx).
    if (header.starts_with(b"ID3") && !ascii_lookalike)
        || (header[0] == 0xFF && (header[1] & 0xE0) == 0xE0)
    {
        return Some(MediaKind::Audio);
    }
    if header.starts_with(b"fLaC") && !ascii_lookalike {
        return Some(MediaKind::Audio);
    }
    // ISO-BMFF: 4-byte size then 'ftyp'. Covers mp4/m4a/mov/3gp.
    if &header[4..8] == b"ftyp" {
        // `M4A ` / `M4B ` brands are audio-only; everything else may carry video.
        let brand = &header[8..12];
        if brand == b"M4A " || brand == b"M4B " {
            return Some(MediaKind::Audio);
        }
        return Some(MediaKind::Video);
    }
    // Matroska / WebM (EBML), Ogg, and RIFF (avi/wav) containers.
    if header.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some(MediaKind::Video);
    }
    // `OggS` is the fifth ASCII signature with nothing after it to check —
    // the byte that follows is the stream structure version, and it is 0 on
    // every Ogg page ever written. Same veto as the group below.
    if header.starts_with(b"OggS") && !ascii_lookalike {
        return Some(MediaKind::Audio);
    }
    if header.starts_with(b"RIFF") {
        return match &header[8..12] {
            b"WAVE" => Some(MediaKind::Audio),
            b"AVI " => Some(MediaKind::Video),
            _ => None,
        };
    }
    // AIFF/AIFC (big-endian IFF), Sun/NeXT au, Monkey's Audio, WavPack,
    // Musepack, FLV, ASF/WMV/WMA, MPEG program/transport streams, RealMedia.
    if header.starts_with(b"FORM") && matches!(&header[8..12], b"AIFF" | b"AIFC") {
        return Some(MediaKind::Audio);
    }
    if (header.starts_with(b".snd")
        || header.starts_with(b"MAC ")
        || header.starts_with(b"wvpk")
        || header.starts_with(b"MPCK")
        || header.starts_with(b"MP+"))
        && !ascii_lookalike
    {
        return Some(MediaKind::Audio);
    }
    if header.starts_with(b"FLV\x01") || header.starts_with(b".RMF") {
        return Some(MediaKind::Video);
    }
    // ASF/WMV/WMA share one GUID; only ffprobe can say which, so report the
    // container-generic kind (same serve path either way).
    if header.starts_with(&[0x30, 0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11]) {
        return Some(MediaKind::Video);
    }
    // MPEG program stream / video ES start codes.
    if header.starts_with(&[0x00, 0x00, 0x01, 0xBA])
        || header.starts_with(&[0x00, 0x00, 0x01, 0xB3])
    {
        return Some(MediaKind::Video);
    }
    // NO bare MPEG-TS sync check here. A transport stream starts with 0x47 —
    // but so does every text file beginning with the letter "G", and a
    // 12-byte header cannot check the 188-byte packet stride that would make
    // the signature trustworthy. Misfiring turns a readable file into
    // "this is video, I won't read it". Not worth it for a format that
    // essentially always arrives with an extension.
    None
}

/// [`detect_media`] for a file on disk: reads only a 12-byte header, never the
/// file. Identifying a file must never mean loading it.
pub fn detect_media_at(path: &Path) -> Option<MediaKind> {
    use std::io::Read;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    // Extension alone settles the common case with no I/O at all.
    if let Some(ext) = name.rsplit_once('.').map(|(_, e)| e) {
        if let Some(k) = kind_from_ext(ext) {
            return Some(k);
        }
    }
    let mut buf = [0u8; 12];
    let mut f = std::fs::File::open(path).ok()?;
    let n = f.read(&mut buf).ok()?;
    detect_media(&buf[..n], name)
}

/// Timeout for an optional-binary probe. Media tooling is not on the critical
/// path: if it does not answer promptly we degrade rather than stall a hook.
const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Run a command with a timeout, returning stdout on success. `None` when the
/// binary is absent, fails, or overruns — the caller degrades gracefully. This
/// is the whole "optional dependency" contract in one function.
fn run_bounded(bin: &str, args: &[&str]) -> Option<String> {
    run_bounded_for(bin, args, PROBE_TIMEOUT)
}

/// Is an optional media binary usable? Reuses the shared cached PATH lookup —
/// NOT a version probe: `whisper` has no `--version` (it prints usage), so
/// probing reported it missing on a machine where it was installed. "Is it on
/// PATH" is both the real question and cheaper (no spawn at all).
pub fn has_binary(bin: &str) -> bool {
    crate::availability::is_available(bin)
}

/// Compact `ffprobe` summary: what the file IS, in ~30 tokens. `None` when
/// ffprobe is absent or the file is unreadable.
pub fn probe_summary(path: &Path) -> Option<String> {
    let p = path.to_str()?;
    let raw = run_bounded(
        "ffprobe",
        &[
            "-v", "error",
            "-show_entries",
            "format=duration,size,bit_rate,format_long_name:stream=codec_type,codec_name,width,height,channels,sample_rate",
            "-of", "default=noprint_wrappers=1",
            p,
        ],
    )?;
    if raw.trim().is_empty() {
        return None;
    }
    Some(probe_line(&raw))
}

/// Extension of a sidecar subtitle, in the order we prefer them.
const SUBTITLE_EXTS: [&str; 4] = ["srt", "vtt", "ass", "ssa"];

/// A subtitle file sitting next to the media (`talk.mp4` → `talk.srt`).
/// FREE transcript: no ASR, no ffmpeg, exact text. Always worth trying first.
pub fn sidecar_subtitle(path: &Path) -> Option<std::path::PathBuf> {
    for ext in SUBTITLE_EXTS {
        let cand = path.with_extension(ext);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

/// Embedded subtitle track, extracted with ffmpeg. Free relative to ASR: the
/// text is already in the container, exact and timed. `None` when ffmpeg is
/// absent or the file carries no text track.
pub fn embedded_subtitles(path: &Path) -> Option<String> {
    let p = path.to_str()?;
    let raw = run_bounded(
        "ffmpeg",
        &["-v", "error", "-i", p, "-map", "0:s:0", "-f", "srt", "-"],
    )?;
    let text = subtitle_to_text(&raw);
    (!text.trim().is_empty()).then_some(text)
}

/// ASR model. `base` is the speed/quality knee for meeting-and-podcast speech;
/// `TOKENADE_WHISPER_MODEL` overrides (`tiny` when speed matters, `small`/
/// `medium` when accuracy does).
fn whisper_model() -> String {
    let raw = std::env::var("TOKENADE_WHISPER_MODEL").unwrap_or_else(|_| "base".to_string());
    // SANITIZE: this value is interpolated into a cache FILENAME. Unfiltered,
    // `TOKENADE_WHISPER_MODEL=../../../../tmp/evil` wrote outside the stash —
    // escaping the TTL sweep and voiding the "transcripts cannot accumulate
    // forever" guarantee.
    let clean: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '_' || *c == '-')
        .take(48)
        .collect();
    if clean.is_empty() {
        "base".to_string()
    } else {
        clean
    }
}

/// Wall-clock ceiling for one transcription. Whisper is minutes-scale on long
/// audio, so this is generous — but bounded, because an unbounded child process
/// is how a CLI hangs forever on someone's 3-hour recording.
/// `TOKENADE_WHISPER_TIMEOUT_SECS` overrides.
fn whisper_timeout() -> std::time::Duration {
    let secs = std::env::var("TOKENADE_WHISPER_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(900);
    std::time::Duration::from_secs(secs)
}

/// Where a finished transcript is cached, keyed by CONTENT hash + model.
/// Transcribing is the expensive step; asking the transcript questions is not.
/// Caching by content (not path) means a renamed or copied file is free, and a
/// re-edited one correctly misses.
fn transcript_cache_path(content_hash: &str, model: &str) -> std::path::PathBuf {
    crate::disclose::stash_dir().join(format!("asr-{model}-{content_hash}.txt"))
}

/// A cached transcript for this exact content, if we already made one.
pub fn cached_transcript(content_hash: &str) -> Option<String> {
    let p = transcript_cache_path(content_hash, &whisper_model());
    let text = std::fs::read_to_string(p).ok()?;
    (!text.trim().is_empty()).then_some(text)
}

mod ocr;
mod probe;
mod scanned;
mod subtitles;
pub use ocr::{image_read, ocr_text, ocr_text_cached};
pub(crate) use probe::probe_line;
pub use scanned::ocr_scanned_pdf_pages;
pub use subtitles::subtitle_to_text;

/// A document's text, as `docs::extract` reads it, with a scanned PDF's pages
/// read by OCR when the machine can (see `scanned`).
pub fn extract_document(bytes: &[u8], fmt: crate::docs::Format) -> String {
    let text = crate::docs::extract(bytes, fmt);
    if fmt == crate::docs::Format::Pdf {
        ocr_scanned_pdf_pages(&text, bytes)
    } else {
        text
    }
}

/// [`extract_document`], with the savings of the text actually served.
pub fn extract_document_with_savings(
    bytes: &[u8],
    fmt: crate::docs::Format,
) -> (String, crate::Savings) {
    let (text, savings) = crate::docs::extract_with_savings(bytes, fmt);
    if fmt != crate::docs::Format::Pdf || scanned::scanned_pages(&text).is_empty() {
        return (text, savings);
    }
    let text = ocr_scanned_pdf_pages(&text, bytes);
    let after = crate::tokenizer::estimate(&text);
    (text, crate::Savings::new(savings.tokens_before, after))
}

/// Is any ASR backend usable?
pub fn asr_available() -> bool {
    has_binary("whisper") || (has_binary("whisper-cli") && has_binary("ffmpeg"))
}

/// Transcribe speech to timestamped text with a LOCAL whisper. Returns `None`
/// when no backend is installed, the media carries no speech, or the run
/// overruns [`whisper_timeout`].
///
/// Deliberately explicit: this is never called from a hook. Transcription is
/// minutes-scale and hooks have hard timeouts — a hook that shells out to ASR
/// is a hung session. It runs on the CLI path, where the agent asked for it,
/// and the result is cached so every later question is free.
pub fn transcribe(path: &Path, content_hash: &str) -> Option<String> {
    if let Some(hit) = cached_transcript(content_hash) {
        return Some(hit);
    }
    let model = whisper_model();
    // Per-PROCESS scratch dir. Keying it on the content hash alone made
    // concurrent readers of the same file collide: three simultaneous reads
    // produced ONE transcript, because each finished run `remove_dir_all`s the
    // directory the others are still writing into. The CACHE stays keyed on
    // content (that is the point); only the scratch space is private.
    let tmp = std::env::temp_dir().join(format!(
        "tokenade-asr-{content_hash}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&tmp).ok()?;
    let p = path.to_str()?;
    let out_dir = tmp.to_str()?;

    // OpenAI whisper CLI reads video directly (it shells to ffmpeg itself) and
    // writes <stem>.srt into --output_dir. Preferred: one call, timestamps
    // included.
    // Every early return below must clean the scratch dir, or a corrupt file /
    // non-UTF-8 path leaves hundreds of MB of 16 kHz PCM in /tmp forever (/tmp
    // is NOT covered by the stash TTL sweep). A tiny guard beats remembering.
    struct Scrub<'a>(&'a std::path::Path);
    impl Drop for Scrub<'_> {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0);
        }
    }
    let _scrub = Scrub(&tmp);
    let produced = if has_binary("whisper") {
        run_bounded_for(
            "whisper",
            &[
                p,
                "--model",
                &model,
                "--output_format",
                "srt",
                "--output_dir",
                out_dir,
                "--verbose",
                "False",
            ],
            whisper_timeout(),
        )
        .is_some()
    } else if has_binary("whisper-cli") && has_binary("ffmpeg") {
        // whisper.cpp wants 16 kHz mono PCM; ffmpeg makes it.
        let wav = tmp.join("audio.wav");
        let wav_s = wav.to_str()?;
        run_bounded_for(
            "ffmpeg",
            &[
                "-v", "error", "-y", "-i", p, "-ar", "16000", "-ac", "1", wav_s,
            ],
            whisper_timeout(),
        )?;
        run_bounded_for(
            "whisper-cli",
            &["-f", wav_s, "-osrt", "-of", tmp.join("out").to_str()?],
            whisper_timeout(),
        )
        .is_some()
    } else {
        return None;
    };
    if !produced {
        let _ = std::fs::remove_dir_all(&tmp);
        return None;
    }

    // Collect whichever .srt/.txt the backend wrote.
    let mut text = String::new();
    if let Ok(entries) = std::fs::read_dir(&tmp) {
        for e in entries.flatten() {
            let ep = e.path();
            let is_out = ep
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x == "srt" || x == "txt")
                .unwrap_or(false);
            if is_out {
                if let Ok(raw) = std::fs::read_to_string(&ep) {
                    text.push_str(&subtitle_to_text(&raw));
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    if text.trim().is_empty() {
        return None;
    }
    // Cache in the stash dir, which the TTL sweep already reclaims — a
    // transcript must not become the next thing that accumulates forever.
    let cache = transcript_cache_path(content_hash, &model);
    if let Some(parent) = cache.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Atomic: `cached_transcript` trusts the file for existing, so a write cut
    // short would be served as the whole transcript from then on.
    let _ = crate::platform::write_atomic_600(&cache, text.as_bytes());
    Some(text)
}

/// [`run_bounded`] with an explicit ceiling (ASR needs minutes, probes need
/// seconds).
pub(crate) fn run_bounded_for(
    bin: &str,
    args: &[&str],
    limit: std::time::Duration,
) -> Option<String> {
    let mut cmd = crate::process_util::internal_command(bin);
    cmd.args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null());
    crate::process_util::configure_no_window(&mut cmd);
    // A census of child-process sites listed this one as "bounded by its own
    // timeout, which it applies by killing the child" — true about ITS timeout,
    // and silent about OURS. `whisper_timeout` is minutes-scale, so a SIGKILLed
    // tokenade left an ASR run burning a core for minutes with nobody to read
    // its output. One choke point covers whisper, ffmpeg and ffprobe alike.
    crate::process_util::configure_own_group(&mut cmd);
    crate::process_util::configure_die_with_parent(&mut cmd);
    let mut child = cmd.spawn().ok()?;
    let mut reaper = if !crate::process_util::terminal_owns_the_group() {
        crate::process_util::guard_group_against_our_death(child.id())
    } else {
        None
    };
    // DRAIN stdout on a thread. The previous version polled `try_wait` while
    // leaving the pipe unread: once the child filled the 64 KiB pipe buffer it
    // blocked on `write`, never exited, and we killed it at the timeout.
    // Measured: 40 KB returned in 25 ms, 200 KB deadlocked for the full 10 s.
    // `embedded_subtitles` pipes a whole SRT through ffmpeg — a feature-length
    // subtitle track is 100-200 KB, so the "free exact transcript" NEVER fired
    // on the content it exists for, and cost 10 s each time to not fire.
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut buf = String::new();
        let _ = stdout.read_to_string(&mut buf);
        buf
    });
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) => {
                if started.elapsed() > limit {
                    let _ = child.kill();
                    // Reap, or every timeout leaves a zombie for the life of
                    // the process.
                    let _ = child.wait();
                    if let Some(r) = reaper.take() {
                        r.stand_down();
                    }
                    let _ = reader.join();
                    return None;
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(_) => {
                if let Some(r) = reaper.take() {
                    r.stand_down();
                }
                let _ = reader.join();
                return None;
            }
        }
    };
    // The child is reaped: stand the reaper down BEFORE its pid can be recycled.
    // Dropping it instead would fire `kill(-pgid)` at a group id that no longer
    // belongs to us — the same recycling hazard `stop_forwarding_termination`
    // exists to avoid on the other four sites, and the success path is the one
    // this function takes almost every time.
    if let Some(r) = reaper.take() {
        r.stand_down();
    }
    let out = reader.join().ok()?;
    status.success().then_some(out)
}

/// Cheap identity for a media file: size + mtime + a bounded PREFIX hash.
///
/// The obvious `hash_content(&fs::read(path))` reads the WHOLE file — a 4 GB
/// `.mkv` allocated 4 GB inside a PreToolUse hook (and twice on the CLI
/// `--frames` path), which is exactly the "identifying a file must never mean
/// loading it" rule the header-only detector follows. A 1 MiB prefix plus size
/// and mtime distinguishes any two real media files an agent re-reads, and the
/// worst case of a collision is a reused cache entry for identical content.
pub fn cheap_media_identity(path: &Path) -> Option<String> {
    use std::io::Read;
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut head = vec![0u8; 1024 * 1024];
    let mut f = std::fs::File::open(path).ok()?;
    let n = f.read(&mut head).ok()?;
    head.truncate(n);
    let mut keyed = format!("{}:{}:", meta.len(), mtime).into_bytes();
    keyed.extend_from_slice(&head);
    Some(crate::dedup_ref::hash_content(&keyed))
}

/// Credit for serving a media file: **ZERO saving**, by design.
///
/// The Read-hook path has NO baseline to beat — Claude Code cannot read audio
/// or video at all, so serving a ~100-token summary COSTS more than doing
/// nothing. That is capability, not economy. And the CLI path's old "107k
/// tokens avoided" was OUR OWN inflation bug (lossy UTF-8 decoding of binary):
/// crediting its removal would bill a regression we caused as a win.
///
/// The genuine media saving lives elsewhere and is already counted there: when
/// a transcript exceeds the tout-vs-RAG threshold, `answer_over_blob` serves
/// passages instead of the whole thing — ordinary text, ordinary path.
pub fn plausible_media_credit(_size_bytes: u64, delivered_tokens: usize) -> (usize, usize) {
    (delivered_tokens, delivered_tokens)
}

/// Duration in seconds via ffprobe, for spacing frame samples.
fn probe_duration_secs(path: &Path) -> Option<f64> {
    let p = path.to_str()?;
    let out = run_bounded(
        "ffprobe",
        &[
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "csv=p=0",
            p,
        ],
    )?;
    out.trim().parse().ok()
}

/// Default ceiling on extracted frames. Each frame the agent Reads costs ~1.5k
/// vision tokens, so 8 frames ≈ 12k — already more than most transcripts.
pub const DEFAULT_MAX_FRAMES: usize = 8;

/// Representative frames, sampled EVENLY across the duration then deduplicated
/// by content hash.
///
/// Not ffmpeg's `select='gt(scene,…)'`: measured on a three-cut clip whose cuts
/// genuinely score 0.4, that filter emitted ONE frame — its metric is too
/// content-dependent to build on, and when it under-fires the agent silently
/// loses most of the video. Even sampling gives predictable coverage, and
/// hash-dedup removes exactly the redundancy scene detection was meant to
/// remove: a screencast parked on one slide yields identical JPEGs, which
/// collapse to a single frame.
pub fn extract_scene_frames(
    path: &Path,
    content_hash: &str,
    max_frames: usize,
) -> Option<Vec<std::path::PathBuf>> {
    if !has_binary("ffmpeg") {
        return None;
    }
    let n = max_frames.clamp(1, 32);
    let final_dir = crate::disclose::stash_dir().join(format!("frames-{content_hash}"));
    if let Ok(rd) = std::fs::read_dir(&final_dir) {
        let mut cached: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        if !cached.is_empty() {
            cached.sort();
            cached.truncate(n);
            return Some(cached);
        }
    }
    // Stage into a PRIVATE directory and publish atomically. Writing straight
    // into the shared cache let a concurrent reader observe a half-written
    // frame set and treat it as complete.
    let dir = crate::disclose::stash_dir().join(format!(
        "frames-{content_hash}.staging-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).ok()?;
    let p = path.to_str()?;
    let pattern = dir.join("f%03d.jpg");
    let pattern_s = pattern.to_str()?;

    let duration = probe_duration_secs(path).unwrap_or(0.0);
    let want = (n * 3).min(64);
    // No LOWER clamp on fps: clamping it up made a 3 h recording sample only
    // its first ~20 minutes while still being labelled "evenly sampled".
    let vf = if duration > 1.0 {
        format!(
            "fps={:.6},scale='min(1024,iw)':-2,format=yuvj420p",
            (want as f64 / duration).clamp(0.000_1, 8.0)
        )
    } else {
        "fps=4,scale='min(1024,iw)':-2,format=yuvj420p".to_string()
    };
    // `format=yuvj420p` is not cosmetic: the mjpeg encoder REFUSES non-full-
    // range YUV ("Error while opening encoder") and ffmpeg still exits 0, so
    // the failure is silent — zero frames, no error.
    let ok = run_bounded_for(
        "ffmpeg",
        &[
            "-v",
            "error",
            "-y",
            "-i",
            p,
            "-vf",
            &vf,
            "-frames:v",
            &want.to_string(),
            pattern_s,
        ],
        whisper_timeout(),
    );
    if ok.is_none() {
        let _ = std::fs::remove_dir_all(&dir);
        return None;
    }
    let mut sampled: Vec<_> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .collect();
    if sampled.is_empty() {
        let _ = std::fs::remove_dir_all(&dir);
        return None;
    }
    sampled.sort();

    // Dedup by content hash: byte-identical frames are the static-slide case,
    // and paying vision tokens for the same picture twice is the waste this
    // whole module exists to prevent.
    let mut seen = std::collections::HashSet::new();
    let mut kept = Vec::new();
    for f in sampled {
        let Ok(bytes) = std::fs::read(&f) else {
            continue;
        };
        if seen.insert(crate::dedup_ref::hash_content(&bytes)) && kept.len() < n {
            kept.push(f);
        } else {
            let _ = std::fs::remove_file(&f);
        }
    }
    if kept.is_empty() {
        let _ = std::fs::remove_dir_all(&dir);
        return None;
    }
    // Publish. A rename is atomic within the stash, so a concurrent reader sees
    // either no cache or a complete one. Losing the race is fine: the winner's
    // frames are equivalent (same content hash).
    if std::fs::rename(&dir, &final_dir).is_ok() {
        let mut published: Vec<_> = std::fs::read_dir(&final_dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .collect();
        published.sort();
        published.truncate(n);
        return Some(published);
    }
    let _ = std::fs::remove_dir_all(&dir);
    let mut theirs: Vec<_> = std::fs::read_dir(&final_dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .collect();
    theirs.sort();
    theirs.truncate(n);
    (!theirs.is_empty()).then_some(theirs)
}

/// Which optional binary would unlock more, given what is installed. `None`
/// when nothing is missing. The nudge names ONE next step, not a shopping list.
pub fn install_nudge(kind: MediaKind) -> Option<&'static str> {
    let ffmpeg = has_binary("ffmpeg");
    let ffprobe = has_binary("ffprobe");
    if !ffprobe || !ffmpeg {
        return Some(
            "install `ffmpeg` (ships `ffprobe`) to unlock duration/codecs/tracks and free \
             extraction of any embedded subtitles",
        );
    }
    // ffmpeg present: the remaining gap is speech, and only for media that has any.
    let _ = kind;
    if !has_binary("whisper") && !has_binary("whisper-cli") {
        return Some(
            "install `whisper` (or `whisper.cpp`) to transcribe speech — a transcript is \
             ordinary text you can then question directly",
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_media_by_extension_and_magic() {
        // Extension path.
        assert_eq!(detect_media(b"", "talk.mp3"), Some(MediaKind::Audio));
        assert_eq!(detect_media(b"", "clip.MKV"), Some(MediaKind::Video));
        assert_eq!(detect_media(b"", "notes.txt"), None);

        // Magic-byte path — a file whose name lies must still be caught, or the
        // text compactor inflates it (the 107k-token bug this module exists for).
        let mut mp4 = vec![0, 0, 0, 0x20];
        mp4.extend_from_slice(b"ftypisom");
        assert_eq!(detect_media(&mp4, "not-really"), Some(MediaKind::Video));

        let mut m4a = vec![0, 0, 0, 0x20];
        m4a.extend_from_slice(b"ftypM4A ");
        assert_eq!(detect_media(&m4a, "x"), Some(MediaKind::Audio));

        assert_eq!(
            detect_media(b"ID3\x04\x00\x00\x00\x00\x00\x00\x00\x00", "x"),
            Some(MediaKind::Audio)
        );
        // Raw MPEG frame sync (no ID3 tag).
        assert_eq!(
            detect_media(b"\xFF\xFB\x90\x00\x00\x00\x00\x00\x00\x00\x00\x00", "x"),
            Some(MediaKind::Audio)
        );
        assert_eq!(
            detect_media(b"fLaC\x00\x00\x00\x22aaaa", "x"),
            Some(MediaKind::Audio)
        );
        assert_eq!(
            detect_media(b"OggS\x00\x02\x00\x00\x00\x00\x00\x00", "x"),
            Some(MediaKind::Audio)
        );
        assert_eq!(
            detect_media(&[0x1A, 0x45, 0xDF, 0xA3, 1, 2, 3, 4, 5, 6, 7, 8], "x"),
            Some(MediaKind::Video)
        );
        assert_eq!(
            detect_media(b"RIFF\x00\x00\x00\x00WAVE", "x"),
            Some(MediaKind::Audio)
        );
        assert_eq!(
            detect_media(b"RIFF\x00\x00\x00\x00AVI ", "x"),
            Some(MediaKind::Video)
        );

        // Plain text must never be mistaken for media.
        assert_eq!(detect_media(b"hello world, this is text", "x"), None);
    }

    /// The false positive that would hurt most: our users are developers, and
    /// their trees are full of extensions that also name media containers.
    /// Routing source code into the media path makes it UNREADABLE — a far
    /// worse failure than not recognising an exotic stream.
    #[test]
    fn source_files_are_never_mistaken_for_media() {
        // `.ts` is TypeScript far more often than MPEG-TS. Measured on a real
        // machine: 22 583 `.ts`, every one TypeScript.
        assert_eq!(detect_media(b"export const x = 1;", "index.d.ts"), None);
        assert_eq!(detect_media(b"import foo from 'bar';", "main.ts"), None);
        // `.dts` is Device Tree Source in kernel work.
        assert_eq!(detect_media(b"/dts-v1/;", "board.dts"), None);
        // A text file starting with "G" (0x47) must not trip an MPEG-TS sync
        // guess — this is why the bare 0x47 magic check does not exist.
        assert_eq!(
            detect_media(b"GET /health HTTP/1.1\r\n\r\n", "access.log"),
            None
        );
        assert_eq!(
            detect_media(b"Group: admins\nMembers: 3\n", "groups.conf"),
            None
        );
    }

    #[test]
    fn subtitle_becomes_readable_timed_text() {
        let srt = "1\n00:00:04,120 --> 00:00:06,900\nWelcome to the meeting.\n\n\
                   2\n00:01:12,000 --> 00:01:14,500\nThe migration timed out.\n";
        let text = subtitle_to_text(srt);
        // Cue indices gone, text kept, start timestamps kept (an agent asks
        // "when was X said" and must be able to answer).
        assert!(!text.contains("\n1\n"), "cue index leaked: {text}");
        assert!(text.contains("Welcome to the meeting."));
        assert!(text.contains("The migration timed out."));
        assert!(
            text.contains("00:00:04"),
            "start timestamp must survive: {text}"
        );
        assert!(
            !text.contains("-->"),
            "end timestamps are cost without answers: {text}"
        );
        // A VTT header is noise.
        assert!(
            !subtitle_to_text("WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nHi\n").contains("WEBVTT")
        );
    }

    /// Frames are the expensive way to read a video (~1.5k vision tokens each),
    /// so the caps and the opt-in are part of the contract, not tuning.
    #[test]
    fn frame_extraction_is_bounded_and_opt_in() {
        // Absent ffmpeg → None, never a panic or a stall.
        // (When ffmpeg IS present this returns frames; the bound is what we
        // assert, and it holds either way.)
        if let Some(frames) = extract_scene_frames(
            Path::new("/nonexistent-tokenade-media.mp4"),
            "deadbeefcafe",
            4,
        ) {
            assert!(frames.len() <= 4, "the cap must hold");
        }
        // The advertised default stays small enough that reading every frame
        // costs less than a typical transcript.
        const {
            assert!(
                DEFAULT_MAX_FRAMES <= 8,
                "a large default would silently cost ~1.5k tok/frame"
            )
        };
    }

    /// Regression guard for the accounting incident: a media serve booked
    /// `bytes/4` as the avoided cost, so ONE 800 MB video claimed 209 MILLION
    /// tokens saved and 136 such rows reached 3.1 BILLION — 99.8 % of the whole
    /// ledger, surfacing as "3.1B tokens saved (=46 688 $)". A credit must be
    /// bounded by what could actually have been delivered, never by file size.
    #[test]
    fn a_huge_media_file_cannot_claim_a_huge_saving() {
        // 800 MB of video, ~90 tokens actually delivered.
        let credit = plausible_media_credit(800 * 1024 * 1024, 90);
        let saved = credit.0.saturating_sub(credit.1);
        assert_eq!(
            saved, 0,
            "a media serve has no baseline to beat (Claude Code cannot read media at \
             all) — it is capability, not savings, and must book zero"
        );
        assert!(
            saved < crate::gain::MAX_PLAUSIBLE_ROW_SAVING,
            "a single row must never approach the plausibility ceiling"
        );
        // A small file cannot claim more than it could have cost either.
        let small = plausible_media_credit(1024, 80);
        assert!(small.0 >= small.1, "credit must never be inverted");
    }

    #[test]
    fn a_missing_binary_degrades_instead_of_failing() {
        // The optional-dependency contract: an absent binary is None, never a
        // panic and never a stall.
        assert!(run_bounded("tokenade-no-such-binary-xyz", &["-version"]).is_none());
        assert!(!has_binary("tokenade-no-such-binary-xyz"));
        // A binary with no `--version` flag must still count as present —
        // `whisper` prints usage instead, and a version probe wrongly reported
        // it missing on a machine that had it. Presence on PATH is the test.
        assert!(
            has_binary("sh") || cfg!(windows),
            "a PATH binary must be detected"
        );
    }
    /// `.mts` is AVCHD and it is also TypeScript's ESM module
    /// extension. It sat in the video list, so 19 574 TypeScript files on this
    /// machine (ZERO with a 0x47 sync byte) came back as
    /// `[tokenade:video …]` with an empty transcript and the source lost.
    /// The name narrows the population enough to make the sync byte
    /// trustworthy, so the header decides.
    #[test]
    fn an_mts_typescript_module_is_not_a_video() {
        let src = b"// A TypeScript ESM module.\nimport { x } from \"./y.mjs\";\n";
        assert_eq!(detect_media(src, "mod.mts"), None);
    }

    #[test]
    fn an_mts_avchd_clip_is_still_a_video() {
        let mut pkt = vec![0x47u8, 0x40, 0x00, 0x10];
        pkt.extend(std::iter::repeat_n(0u8, 184));
        assert_eq!(detect_media(&pkt, "clip.mts"), Some(MediaKind::Video));
    }

    /// The rule needs BOTH halves, and each refuses something the other lets
    /// through. A `.mts` opening on the letter "G" — 0x47 — is the exact false
    /// positive the bare sync check is refused for further down, and only the
    /// text half sees it.
    #[test]
    fn an_mts_that_merely_starts_with_g_is_not_a_video() {
        assert_eq!(detect_media(b"GET /health HTTP/1.1\r\n", "log.mts"), None);
    }

    /// And a `.mts` that is binary but carries no transport-stream sync byte is
    /// not AVCHD either — a truncated download, a mis-named blob. Only the sync
    /// half sees this one: sending it to whisper costs a local transcription
    /// run to produce nothing.
    #[test]
    fn an_mts_binary_that_is_not_a_transport_stream_is_not_a_video() {
        let blob = [
            0x1fu8, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x03, 0xed, 0x5b,
        ];
        assert_eq!(detect_media(&blob, "blob.mts"), None);
    }
}
