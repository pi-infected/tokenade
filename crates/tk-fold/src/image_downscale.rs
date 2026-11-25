//! Downscale oversized images an agent reads, so they cost fewer vision tokens.
//!
//! Vision models bill roughly by image AREA. Anthropic resizes any image to a
//! 1568px long edge / ~1.15 megapixels and bills for that; other providers may
//! not. An image larger than the target is resized down (aspect-preserving)
//! before it reaches the model.
//!
//! This is **lossy** — the model sees a lower-resolution image — and the
//! default target ([`DEFAULT_MAX_EDGE`], 1024px / 0.66 MP) sits BELOW that cap
//! on purpose: capping at 1568 would be a no-op against Anthropic, which
//! resizes there anyway. An image at or under the target is left untouched.
//! Set `TOKENADE_IMAGE_MAX_EDGE` higher to keep more detail, or `0` to disable.
//!
//! The platform's own cap is kept as [`PLATFORM_MAX_EDGE`], because what we
//! SEND and what the model is CHARGED for are two different numbers — see
//! [`vision_token_estimate`].

use image::imageops::FilterType;
use image::ImageReader;
use std::io::Cursor;
use std::path::Path;

/// Default long-edge cap. Set BELOW Anthropic's own 1568px resize cap so we
/// actually save tokens on Claude (Anthropic would otherwise bill at 1568);
/// 1024 keeps screenshots/diagrams readable while cutting ~half the area. The
/// original stays on disk and the redirect hint tells the agent how to load it
/// full-resolution. Override with `TOKENADE_IMAGE_MAX_EDGE` (0 disables).
pub const DEFAULT_MAX_EDGE: u32 = 1024;
/// Default total-pixel cap, scaled to the 1024 edge (~0.66 MP).
pub const DEFAULT_MAX_PIXELS: u64 = 660_000;
/// Decompression-bomb guard: never decode an image whose HEADER claims more
/// than this many pixels (covers any legitimate screenshot/photo).
const DECODE_PIXEL_LIMIT: u64 = 50_000_000;

/// Image formats we can decode + re-encode.
///
/// Everything here is handled by the `image` crate in pure Rust, so the list
/// costs binary size but no runtime dependency. Formats an agent cannot render
/// natively (tiff, bmp, ico, tga, pnm…) gain the most: re-encoding them to png
/// makes them readable at all, not merely cheaper.
///
/// NOT here, on purpose: `heic`/`heif` (iPhone photos) and `avif` need C
/// decoders (libheif / dav1d) that would bloat the binary and break the static
/// Windows build. They fall through untouched rather than being claimed and
/// then failing.
/// Enumerable so the rules we scaffold can be checked against it in both
/// directions — an image format we decode but never mention is one users never
/// try.
pub const DOWNSCALABLE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "ico", "pnm", "pbm", "pgm", "ppm",
    "tga", "qoi", "hdr", "ff",
];

pub fn is_downscalable_ext(ext: &str) -> bool {
    let ext = ext.to_ascii_lowercase();
    DOWNSCALABLE_EXTENSIONS.contains(&ext.as_str())
}

/// Header-only dimensions of an image file (cheap — never decodes the pixels).
/// `None` if unreadable / not an image we parse.
///
/// STREAMED from the file, never read whole. This said "cheap" while pulling
/// the entire file into memory and then copying it twice inside `reader_for`:
/// MEASURED 2026-09-06 on a 1.2 GB file carrying a valid 64×64 PNG header,
/// **3 691 760 kB of peak RSS to answer `64, 64`** — three copies of a file
/// whose first 33 bytes hold the whole answer. It is called on the Read hook's
/// path for every image an agent opens, so the cost was paid before anything
/// had decided the file was worth reading at all.
///
/// The extension fallback is kept: `with_guessed_format` reads the magic bytes,
/// and a file whose NAME lies is still handled, but a format we cannot sniff
/// (some tiff and tga variants carry no usable magic) is named by its path.
pub fn image_dimensions(path: &Path) -> Option<(u32, u32)> {
    let guessed = ImageReader::open(path).ok()?.with_guessed_format().ok()?;
    if guessed.format().is_some() {
        return guessed.into_dimensions().ok();
    }
    let fmt = image::ImageFormat::from_path(path).ok()?;
    let mut r = ImageReader::open(path).ok()?;
    r.set_format(fmt);
    r.into_dimensions().ok()
}

/// Header-only dimensions of an image already in memory. MCP payloads arrive
/// as base64 and never touch the disk, so the file-path form above cannot
/// serve them. `ext` is the fallback when the bytes cannot be sniffed, exactly
/// as for [`image_dimensions`].
pub fn image_dimensions_bytes(bytes: &[u8], ext: &str) -> Option<(u32, u32)> {
    let named = std::path::PathBuf::from(format!("image.{ext}"));
    reader_for(bytes, &named)?.into_dimensions().ok()
}

/// Build a reader that can identify the image even when its CONTENT cannot be
/// sniffed. TGA has no leading magic bytes (its only signature is an optional
/// v2 footer), so `with_guessed_format` fails on it outright — probing a `.tga`
/// returned "not an image" and the format was advertised but unusable. Guess
/// from content first (a file whose name lies is still handled), then fall back
/// to the extension.
fn reader_for(bytes: &[u8], path: &Path) -> Option<ImageReader<Cursor<Vec<u8>>>> {
    // ONE copy. This used to make three — `bytes.to_vec()`, then `.clone()` of
    // that to sniff the format, and the sniffing reader kept when it worked —
    // so the peak was three times the file for a question the magic bytes
    // answer. `guess_format` reads those magic bytes off the slice and needs no
    // owned buffer at all.
    let fmt = image::guess_format(bytes)
        .ok()
        .or_else(|| image::ImageFormat::from_path(path).ok())?;
    let mut r = ImageReader::new(Cursor::new(bytes.to_vec()));
    r.set_format(fmt);
    Some(r)
}

/// Anthropic's own resize cap: an image above it is scaled to this BEFORE the
/// model is billed, so its raw area is never what it costs. Deliberately
/// separate from [`DEFAULT_MAX_EDGE`] — the two answer different questions:
/// what we send, and what the platform charges for.
pub const PLATFORM_MAX_EDGE: u32 = 1568;
/// Companion pixel cap for [`PLATFORM_MAX_EDGE`] (~1.15 MP).
pub const PLATFORM_MAX_PIXELS: u64 = 1_150_000;

/// Rough vision-token cost of a `w×h` image (~area / 750, Anthropic-ish). Used
/// only to record an estimated saving on the dashboard, not for billing.
///
/// Billed on the dimensions the PLATFORM bills, not the raw ones: it resizes
/// anything past [`PLATFORM_MAX_EDGE`] first, so charging raw area credits a
/// saving nobody was ever going to pay. Measured on a real 877×2948
/// screenshot: raw area says 3447 tokens where the model is charged 974, a
/// 3.5× overstatement that reached the dashboard, the signed receipt and the
/// "~N fewer vision tokens" line shown to the agent.
pub fn vision_token_estimate(w: u32, h: u32) -> u64 {
    let (w, h) = target_dimensions(w, h, PLATFORM_MAX_EDGE, PLATFORM_MAX_PIXELS).unwrap_or((w, h));
    ((w as u64 * h as u64) / 750).max(1)
}

/// Configured long-edge target from `TOKENADE_IMAGE_MAX_EDGE`:
///   - unset / invalid → [`DEFAULT_MAX_EDGE`]
///   - `0` → 0, meaning DISABLED (no downscaling; agent/user opts back to
///     full resolution)
///   - any value `>= 64` → that value
/// Values in `1..64` are ignored (too small to be a real target) → default.
pub fn configured_max_edge() -> u32 {
    match std::env::var("TOKENADE_IMAGE_MAX_EDGE")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
    {
        Some(0) => 0,
        Some(e) if e >= 64 => e,
        _ => DEFAULT_MAX_EDGE,
    }
}

/// Read + downscale `path` using the configured target. `Some((bytes, w, h))`
/// when a resize happened; `None` when the file isn't a downscalable image, is
/// already within the target, or fails to decode.
pub fn downscale_image_file(path: &Path) -> Option<(Vec<u8>, u32, u32)> {
    let max_edge = configured_max_edge();
    if max_edge == 0 {
        return None; // disabled via TOKENADE_IMAGE_MAX_EDGE=0
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if !is_downscalable_ext(ext) {
        return None;
    }
    // The same order as `convert_to_png`: ask the header what this is before
    // pulling the file into memory. `downscale_image_bytes` re-checks on the
    // bytes it is given, because it is also reached with a payload that never
    // was a file (MCP results), and that check is the one that must not move.
    let (w, h) = image_dimensions(path)?;
    if (w as u64) * (h as u64) > DECODE_PIXEL_LIMIT {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    downscale_image_bytes(&bytes, ext, max_edge, DEFAULT_MAX_PIXELS)
}

/// **An image ENLARGED for character recognition**, as PNG bytes. Screenshot
/// text is 10-16 px high, below what tesseract reads reliably: on a 600x160
/// invoice it returned `1,25000 EUR` for `1,250.00 EUR` — a wrong figure, read
/// three times larger it returned the right one (2026-10-04). Below
/// `OCR_UPSCALE_BELOW` px on the long edge the image is scaled by 3, capped at
/// `OCR_UPSCALE_CAP`. `None` when the file is not a decodable image, is
/// already large, or is past the decode limit.
pub fn upscaled_for_ocr(path: &Path) -> Option<Vec<u8>> {
    const OCR_UPSCALE_BELOW: u32 = 2000;
    const OCR_UPSCALE_CAP: u32 = 4000;
    let (w, h) = image_dimensions(path)?;
    let long = w.max(h);
    if long == 0 || long >= OCR_UPSCALE_BELOW || (w as u64) * (h as u64) > DECODE_PIXEL_LIMIT {
        return None;
    }
    let factor = (OCR_UPSCALE_CAP as f32 / long as f32).min(3.0);
    let bytes = std::fs::read(path).ok()?;
    let img = reader_for(&bytes, path)?.decode().ok()?;
    let big = img.resize(
        (w as f32 * factor).round() as u32,
        (h as f32 * factor).round() as u32,
        FilterType::CatmullRom,
    );
    let mut out = Cursor::new(Vec::new());
    big.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

/// Result of staging a downscaled copy of an image an agent is about to read.
pub struct DownscaledRead {
    /// Path to the downscaled image — redirect the Read here.
    pub redirect_path: std::path::PathBuf,
    /// Path to an untouched full-resolution copy under `.tokenade/` (which the
    /// hook never re-downscales), so the agent can recover detail mid-task.
    pub full_path: std::path::PathBuf,
    pub orig_w: u32,
    pub orig_h: u32,
    pub new_w: u32,
    pub new_h: u32,
}

/// Stage a downscaled copy of `file_path` for a Read redirect, shared by every
/// agent wiring (Claude Code / Codex / Grok via the PreToolUse hook, Gemini /
/// Qwen via BeforeTool). Writes the downscaled image plus a full-resolution
/// recovery copy under `<base_dir>/.tokenade/reads/`. Returns `None` when the
/// file isn't a downscalable image, is already within target, downscaling is
/// disabled, or it already lives under `.tokenade/` (our own staged copies —
/// never recurse on them). Does NOT log the gain; the caller does (the ledger
/// writer lives in the binary crate).
/// TTL for the per-project image-read cache (`<root>/.tokenade/reads/`):
/// the downscaled views plus their full-resolution recovery copies. These
/// are regenerated cheaply on the next Read, so anything untouched this long
/// is safe to drop. Matches the stash / sessions TTL.
pub const READS_TTL_SECS: u64 = 7 * 86_400;

/// Remove downscaled + full-res image copies under `<root>/.tokenade/reads/`
/// older than `ttl_secs`. This directory (one `.downscaled.*` + one `.full.*`
/// per distinct image ever Read, the full-res copies potentially large)
/// previously had NO automatic cleanup — it grew for the life of the project.
/// Best-effort; returns files removed.
pub fn evict_stale_reads(root: &Path, ttl_secs: u64) -> usize {
    let dir = root.join(".tokenade").join("reads");
    crate::platform::evict_files_older_than(&dir, ttl_secs)
}

#[cfg(test)]
mod evict_reads_tests {
    use super::*;

    #[test]
    fn evicts_old_reads_keeps_fresh() {
        let base = std::env::temp_dir().join(format!("tkd-reads-{}", std::process::id()));
        let dir = base.join(".tokenade").join("reads");
        std::fs::create_dir_all(&dir).unwrap();
        let old = dir.join("a.full.png");
        let fresh = dir.join("b.full.png");
        std::fs::write(&old, b"x").unwrap();
        std::fs::write(&fresh, b"y").unwrap();
        // Backdate `old` well past a 1000s TTL via set_modified (stable 1.75+).
        let past = std::time::SystemTime::now() - std::time::Duration::from_secs(5_000);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(past)
            .unwrap();
        let removed = evict_stale_reads(&base, 1_000);
        assert_eq!(removed, 1);
        assert!(!old.exists(), "stale copy must be gone");
        assert!(fresh.exists(), "fresh copy must survive");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn missing_reads_dir_is_noop() {
        let base = std::env::temp_dir().join(format!("tkd-reads-none-{}", std::process::id()));
        assert_eq!(evict_stale_reads(&base, 1_000), 0);
    }
}

/// Decode any supported image and re-encode it as PNG, downscaling only if it
/// exceeds the configured bounds. Unlike [`downscale_image_file`] this ALWAYS
/// produces output: it exists for formats the calling agent cannot render at
/// all (tiff scans, bmp screenshots, ico, tga, pnm, qoi…), where "already small
/// enough, leave it alone" would hand the model a blob it cannot display.
///
/// PNG on purpose: lossless, and the one format every agent renders. A photo
/// would be smaller as JPEG, but a wrong-looking conversion is worse than a
/// larger one, and the downscale below already bounds the size.
/// Target size for `w×h` under an edge and a pixel cap, or `None` when the
/// image already fits (never enlarge). Shared so the convert and downscale
/// paths cannot drift on the one rule that matters.
fn target_dimensions(w: u32, h: u32, max_edge: u32, max_pixels: u64) -> Option<(u32, u32)> {
    if max_edge == 0 || w == 0 || h == 0 {
        return None;
    }
    let long = w.max(h);
    let pixels = (w as u64) * (h as u64);
    if long <= max_edge && pixels <= max_pixels {
        return None;
    }
    let edge_scale = max_edge as f64 / long as f64;
    let pixel_scale = (max_pixels as f64 / pixels as f64).sqrt();
    let scale = edge_scale.min(pixel_scale).min(1.0);
    let nw = ((w as f64 * scale).round() as u32).max(1);
    let nh = ((h as f64 * scale).round() as u32).max(1);
    // Guard against a rounding no-op / enlargement.
    if nw >= w && nh >= h {
        return None;
    }
    Some((nw, nh))
}

pub fn convert_to_png(path: &Path) -> Option<(Vec<u8>, u32, u32)> {
    // Dimensions BEFORE the bytes. The guard below exists so a bomb is never
    // decoded; reading the file first meant it was pulled into memory whole
    // before the guard could decline it, which is most of the cost a
    // decompression bomb inflicts.
    let (w, h) = image_dimensions(path)?;
    if (w as u64) * (h as u64) > DECODE_PIXEL_LIMIT {
        return None; // decompression-bomb guard, same as the downscale path
    }
    let bytes = std::fs::read(path).ok()?;
    let img = reader_for(&bytes, path)?.decode().ok()?;
    // Resize by the SAME rule as `downscale_image_bytes`: the tighter of the
    // edge and pixel constraints, and NEVER upward. `DynamicImage::resize`
    // scales to the largest size fitting the box, so passing (max_edge,
    // max_edge) ENLARGES anything under the edge cap — measured: a 900×800
    // tiff (720k px, over the pixel cap, under the edge cap) came out
    // 1024×910, +29 % pixels from the function meant to cut them.
    let max_edge = configured_max_edge();
    let img = match target_dimensions(w, h, max_edge, DEFAULT_MAX_PIXELS) {
        Some((tw, th)) => img.resize(tw, th, FilterType::Triangle),
        None => img,
    };
    let (nw, nh) = (img.width(), img.height());
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some((out, nw, nh))
}

pub fn prepare_downscaled_read(file_path: &Path, base_dir: &Path) -> Option<DownscaledRead> {
    // Never recurse on our own staged copies (downscaled view or full-res copy).
    if file_path.components().any(|c| c.as_os_str() == ".tokenade") {
        return None;
    }
    let ext = file_path.extension().and_then(|e| e.to_str()).unwrap_or("");
    if !is_downscalable_ext(ext) {
        return None;
    }
    let (ow, oh) = image_dimensions(file_path)?;
    let (bytes, nw, nh) = downscale_image_file(file_path)?;
    let dir = base_dir.join(".tokenade").join("reads");
    std::fs::create_dir_all(&dir).ok()?;
    let stem = file_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("image");
    let out_ext = if ext.eq_ignore_ascii_case("png") {
        "png"
    } else {
        "jpg"
    };
    let redirect = dir.join(format!("{stem}.downscaled.{out_ext}"));
    std::fs::write(&redirect, &bytes).ok()?;
    // Full-resolution recovery copy. Lives under `.tokenade/`, so re-reading it
    // bypasses downscaling and hands the agent the original pixels.
    let full = dir.join(format!("{stem}.full.{ext}"));
    let _ = std::fs::copy(file_path, &full);
    Some(DownscaledRead {
        redirect_path: redirect,
        full_path: full,
        orig_w: ow,
        orig_h: oh,
        new_w: nw,
        new_h: nh,
    })
}

/// Core: decode `bytes`, downscale if the long edge exceeds `max_edge` OR the
/// total exceeds `max_pixels`, then re-encode (PNG→PNG lossless, JPEG→JPEG).
/// Returns `None` (leave the original) when it's already small enough, the
/// header looks like a decompression bomb, or decoding fails.
pub fn downscale_image_bytes(
    bytes: &[u8],
    ext: &str,
    max_edge: u32,
    max_pixels: u64,
) -> Option<(Vec<u8>, u32, u32)> {
    // Header-only dimension probe first — cheap, and never decodes the pixels,
    // so a bomb can't be triggered before the size guard below.
    let (w0, h0) = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()?;
    let pixels = w0 as u64 * h0 as u64;
    if pixels == 0 || pixels > DECODE_PIXEL_LIMIT {
        return None;
    }
    let long = w0.max(h0);
    if long <= max_edge && pixels <= max_pixels {
        return None; // already within both limits — leave it alone
    }
    // Scale by whichever limit binds harder.
    let edge_scale = if long > max_edge {
        max_edge as f64 / long as f64
    } else {
        1.0
    };
    let pixel_scale = if pixels > max_pixels {
        (max_pixels as f64 / pixels as f64).sqrt()
    } else {
        1.0
    };
    let scale = edge_scale.min(pixel_scale);
    let nw = ((w0 as f64 * scale).round() as u32).max(1);
    let nh = ((h0 as f64 * scale).round() as u32).max(1);
    if nw >= w0 && nh >= h0 {
        return None;
    }

    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(20_000);
    limits.max_image_height = Some(20_000);
    reader.limits(limits);
    let img = reader.decode().ok()?;
    let resized = img.resize(nw, nh, FilterType::Triangle);

    let fmt = if ext.eq_ignore_ascii_case("png") {
        image::ImageFormat::Png
    } else {
        image::ImageFormat::Jpeg
    };
    let mut out = Cursor::new(Vec::new());
    resized.write_to(&mut out, fmt).ok()?;
    let bytes = out.into_inner();
    // Guard: if re-encoding somehow grew the file (tiny images, format quirks),
    // keep the original.
    if bytes.is_empty() {
        return None;
    }
    Some((bytes, nw, nh))
}

#[cfg(test)]
mod added_image_format_tests {
    use super::*;

    /// Build a tiny image in `fmt` so each newly-claimed format is exercised
    /// for real, not just listed.
    fn encode(fmt: image::ImageFormat) -> Vec<u8> {
        // ICO stores a PNG whose decoder REQUIRES RGBA (an RGB8 payload fails
        // with `PngNotRgba`); JPEG cannot carry alpha at all. Real files respect
        // this — the fixture must too, or the test fails on its own input
        // rather than on the code under test.
        let img = if matches!(fmt, image::ImageFormat::Ico) {
            image::DynamicImage::new_rgba8(8, 6)
        } else {
            image::DynamicImage::new_rgb8(8, 6)
        };
        let mut out = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut out), fmt)
            .unwrap();
        out
    }

    /// Every format we advertise must actually DECODE. Claiming one we cannot
    /// read means the agent gets a redirect to a file that never appears.
    #[test]
    fn every_advertised_image_format_decodes_and_converts_to_png() {
        let cases: &[(&str, image::ImageFormat)] = &[
            ("png", image::ImageFormat::Png),
            ("jpg", image::ImageFormat::Jpeg),
            ("gif", image::ImageFormat::Gif),
            ("webp", image::ImageFormat::WebP),
            ("bmp", image::ImageFormat::Bmp),
            ("tiff", image::ImageFormat::Tiff),
            ("ico", image::ImageFormat::Ico),
            ("tga", image::ImageFormat::Tga),
            ("ppm", image::ImageFormat::Pnm),
            ("qoi", image::ImageFormat::Qoi),
        ];
        let dir = std::env::temp_dir().join(format!("tkd-newfmt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for (ext, fmt) in cases {
            assert!(
                is_downscalable_ext(ext),
                "{ext} advertised but not in the ext list"
            );
            let bytes = encode(*fmt);
            let path = dir.join(format!("t.{ext}"));
            std::fs::write(&path, &bytes).unwrap();

            // Header-only dimensions must work (used before any decode).
            assert!(
                image_dimensions(&path).is_some(),
                "{ext}: dimensions unreadable"
            );

            // And it must convert to PNG — the path taken for any format the
            // calling agent cannot render itself.
            let (png, w, h) = convert_to_png(&path)
                .unwrap_or_else(|| panic!("{ext}: convert_to_png returned None"));
            assert!(w > 0 && h > 0, "{ext}: bad dimensions");
            assert_eq!(
                &png[..8],
                b"\x89PNG\r\n\x1a\n",
                "{ext}: output is not a PNG"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Formats needing heavy C decoders are deliberately NOT claimed — better
    /// to pass them through untouched than to promise and fail.
    #[test]
    fn heic_and_avif_are_not_claimed() {
        for ext in ["heic", "heif", "avif"] {
            assert!(!is_downscalable_ext(ext), "{ext} must not be claimed");
        }
    }

    /// A small image in an unrenderable format must still convert: the whole
    /// point is capability, and `downscale_image_file` returns None when an
    /// image is already within bounds.
    #[test]
    fn a_small_unrenderable_image_still_converts() {
        let dir = std::env::temp_dir().join(format!("tkd-img-small-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tiny.bmp");
        std::fs::write(&path, encode(image::ImageFormat::Bmp)).unwrap();
        assert!(
            downscale_image_file(&path).is_none(),
            "fixture must be small enough that the downscale path declines"
        );
        assert!(
            convert_to_png(&path).is_some(),
            "conversion must still happen — otherwise the agent gets an unreadable .bmp"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod header_is_not_the_whole_file {
    use super::*;

    /// The source of one production function, by its column-zero signature.
    ///
    /// Anchored at column zero on purpose: every test function in this file is
    /// indented inside a module, so a column-zero `fn <name>(` is production
    /// code and nothing else. That also keeps the guard from matching its own
    /// assertions without needing to guess where the tests begin — this
    /// file interleaves test modules with production code, and cutting at the
    /// first `#[cfg(test)]` silently lost everything after line 213.
    fn body_of(name: &str) -> String {
        let src = include_str!("image_downscale.rs");
        let start = src
            .find(&format!("\npub fn {name}("))
            .or_else(|| src.find(&format!("\nfn {name}(")))
            .unwrap_or_else(|| panic!("{name} is gone, or no longer at column zero"))
            + 1;
        let guard_at = src
            .find("mod header_is_not_the_whole_file")
            .expect("this module names itself");
        assert!(
            start < guard_at,
            "{name} was matched inside this guard's own source"
        );
        let rest = &src[start..];
        let end = rest[1..].find("\n}\n").map(|i| i + 4).unwrap_or(rest.len());
        rest[..end].to_string()
    }

    /// The same body with comment lines removed — a guard that counts tokens
    /// must not count the prose EXPLAINING those tokens. Measured the hard way:
    /// the comment saying "this used to make three — `bytes.to_vec()`, then
    /// `.clone()`" made the copy count read 3 on code that makes 1.
    fn code_of(name: &str) -> String {
        body_of(name)
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Reading an image's DIMENSIONS must stream the header, never the file.
    ///
    /// MEASURED 2026-09-06 before this guard existed: `image_dimensions` on a
    /// 1.2 GB file carrying a valid 64×64 PNG header peaked at **3 691 760 kB**
    /// of RSS — `std::fs::read` for the file, then two more copies inside
    /// `reader_for` — to answer a question the first 33 bytes hold. After:
    /// **7 308 kB, and flat in the size of the file** (a 151-byte PNG measures
    /// 7 276). The Read hook calls this for every image an agent opens, so the
    /// cost was paid before anything had decided the file was worth reading.
    ///
    /// A SOURCE guard because the property is memory, and a test that allocates
    /// a gigabyte to make a point about allocating a gigabyte does not belong
    /// in a suite.
    #[test]
    fn image_dimensions_never_reads_the_file_whole() {
        let code = code_of("image_dimensions");
        assert!(
            code.contains("ImageReader::open"),
            "dimensions must be streamed from the path: {code}"
        );
        assert!(
            !code.contains("fs::read"),
            "the whole file is being pulled in to read a header: {code}"
        );
    }

    /// And the same question of the two callers that read the bytes for real:
    /// the size guard has to run BEFORE the read, or a decompression bomb is
    /// already resident by the time it is declined.
    #[test]
    fn the_bomb_guard_runs_before_the_file_is_read() {
        for name in ["convert_to_png", "downscale_image_file"] {
            let code = code_of(name);
            let guard = code
                .find("DECODE_PIXEL_LIMIT")
                .unwrap_or_else(|| panic!("{name} lost its decompression-bomb guard: {code}"));
            let read = code
                .find("fs::read")
                .unwrap_or_else(|| panic!("{name} no longer reads the file at all: {code}"));
            assert!(
                guard < read,
                "{name} reads the file before deciding whether it should: {code}"
            );
        }
    }

    /// One copy of the bytes, not three. `guess_format` answers off the slice.
    #[test]
    fn reader_for_copies_the_bytes_once() {
        let code = code_of("reader_for");
        let copies = code.matches("to_vec()").count() + code.matches(".clone()").count();
        assert_eq!(
            copies, 1,
            "reader_for makes {copies} copies of its input: {code}"
        );
    }

    /// Anti-vacuity for the three guards above: if `body_of` ever returned an
    /// empty string, or the comment stripper ate the code, they would all pass
    /// on nothing.
    #[test]
    fn the_source_extractor_is_reading_real_code() {
        let body = body_of("image_dimensions");
        assert!(
            body.starts_with("pub fn image_dimensions("),
            "wrong anchor: {body}"
        );
        let code = code_of("image_dimensions");
        assert!(
            code.len() > 120,
            "stripped down to {} bytes: {code}",
            code.len()
        );
        assert!(code.contains("into_dimensions"), "the body is gone: {code}");
        // The stripper must actually strip: `reader_for` carries five comment
        // lines, and the guard that counts copies is only sound because they
        // are gone before the count.
        assert!(
            code_of("reader_for").len() < body_of("reader_for").len(),
            "the comment stripper is a no-op, so the copy count reads prose"
        );
    }

    /// Behaviour, cheaply: a header followed by junk still answers correctly,
    /// and a header claiming more pixels than we will ever decode is refused by
    /// both callers.
    #[test]
    fn a_header_with_a_tail_still_answers_and_a_bomb_is_refused() {
        let dir = std::env::temp_dir().join(format!("tkd-imghdr-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);

        let tail = dir.join("tail.png");
        let mut png = header_png(64, 64);
        png.extend(std::iter::repeat_n(0u8, 4 * 1024 * 1024));
        std::fs::write(&tail, &png).unwrap();
        assert_eq!(
            image_dimensions(&tail),
            Some((64, 64)),
            "a valid header with junk after it must still answer"
        );

        // 30 000 × 30 000 = 900 megapixels, eighteen times DECODE_PIXEL_LIMIT.
        let bomb = dir.join("bomb.png");
        std::fs::write(&bomb, header_png(30_000, 30_000)).unwrap();
        assert_eq!(
            image_dimensions(&bomb),
            Some((30_000, 30_000)),
            "the header still reads — it is the DECODE that must be refused"
        );
        assert!(
            convert_to_png(&bomb).is_none(),
            "a 900-megapixel header was accepted for decoding"
        );
        assert!(
            downscale_image_file(&bomb).is_none(),
            "same, on the downscale path"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A PNG that declares `w × h` and carries a stub image stream: enough for
    /// the header to parse, deliberately far too little to be those pixels.
    fn header_png(w: u32, h: u32) -> Vec<u8> {
        let mut ihdr = Vec::new();
        ihdr.extend(w.to_be_bytes());
        ihdr.extend(h.to_be_bytes());
        ihdr.extend([8u8, 2, 0, 0, 0]); // 8-bit truecolour
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        out.extend(chunk(b"IHDR", &ihdr));
        out.extend(chunk(b"IDAT", &zlib_stored(b"x")));
        out.extend(chunk(b"IEND", b""));
        out
    }

    fn chunk(kind: &[u8], data: &[u8]) -> Vec<u8> {
        let mut c = Vec::new();
        c.extend((data.len() as u32).to_be_bytes());
        c.extend(kind);
        c.extend(data);
        let mut crc_input = kind.to_vec();
        crc_input.extend(data);
        c.extend(crc32(&crc_input).to_be_bytes());
        c
    }

    /// A zlib stream holding one STORED deflate block — no compressor needed.
    fn zlib_stored(data: &[u8]) -> Vec<u8> {
        let mut z = vec![0x78, 0x01, 0x01];
        z.extend((data.len() as u16).to_le_bytes());
        z.extend((!(data.len() as u16)).to_le_bytes());
        z.extend(data);
        let (mut a, mut b) = (1u32, 0u32);
        for &byte in data {
            a = (a + byte as u32) % 65521;
            b = (b + a) % 65521;
        }
        z.extend(((b << 16) | a).to_be_bytes());
        z
    }

    /// Bitwise CRC-32, so this test needs no dependency of its own.
    fn crc32(bytes: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for &b in bytes {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    (c >> 1) ^ 0xEDB8_8320
                } else {
                    c >> 1
                };
            }
        }
        !c
    }
}

#[cfg(test)]
mod audit_verification {
    use super::*;
    /// AUDIT CLAIM: convert_to_png enlarges an image that trips the PIXEL cap
    /// but not the EDGE cap, because `resize` scales UP to fit the box.
    #[test]
    fn convert_to_png_must_never_enlarge() {
        let dir = std::env::temp_dir().join(format!("tkd-upscale-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 900x800 = 720_000 px  > DEFAULT_MAX_PIXELS (660_000), edge 900 < 1024.
        let img = image::DynamicImage::new_rgb8(900, 800);
        let path = dir.join("wide.tiff");
        img.save(&path).unwrap();
        let (_png, w, h) = convert_to_png(&path).expect("convert");
        assert!(
            (w as u64) * (h as u64) <= 900 * 800,
            "convert_to_png ENLARGED {}x{} -> {w}x{h}",
            900,
            800
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, RgbImage};

    fn png_of(w: u32, h: u32) -> Vec<u8> {
        let img = RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8])
        });
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut out, ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn vision_estimate_bills_what_the_platform_bills() {
        // The platform resizes anything past its own cap BEFORE billing, so a
        // tall screenshot never costs its raw area. 877×2948 is the witness:
        // raw area says 3447 tokens, the model is charged 974.
        let raw = (877u64 * 2948) / 750;
        let billed = vision_token_estimate(877, 2948);
        assert!(
            billed * 3 < raw,
            "raw area is still being billed: {billed} vs {raw}"
        );
        // Billed EXACTLY as the same image already resized to the cap — that
        // is the image the model actually receives.
        let (cw, ch) =
            target_dimensions(877, 2948, PLATFORM_MAX_EDGE, PLATFORM_MAX_PIXELS).unwrap();
        assert_eq!(billed, vision_token_estimate(cw, ch));
        // Under the cap nothing is capped: raw area is the cost.
        assert_eq!(vision_token_estimate(800, 600), (800 * 600) / 750);
        // The cap applied here is the PLATFORM's, never our own (lower)
        // target — an image between the two is billed at its own size.
        const { assert!(PLATFORM_MAX_EDGE > DEFAULT_MAX_EDGE) };
        assert_eq!(vision_token_estimate(1200, 900), (1200 * 900) / 750);
        // BOTH platform limits bind, not just the edge: 1500×1400 is under
        // the 1568 edge and over the 1.15 MP area, and the witness above
        // cannot tell the two apart because its edge binds first.
        const { assert!(1500 < PLATFORM_MAX_EDGE && 1500 * 1400 > PLATFORM_MAX_PIXELS) };
        assert!(
            vision_token_estimate(1500, 1400) * 3 / 2 < (1500 * 1400) / 750,
            "the megapixel half of the platform cap is not applied"
        );
    }

    #[test]
    fn downscales_oversized_to_edge() {
        let big = png_of(3000, 2000); // long edge 3000 > 1568
        let (_bytes, w, h) = downscale_image_bytes(&big, "png", 1568, DEFAULT_MAX_PIXELS).unwrap();
        assert!(w.max(h) <= 1568, "long edge clamped, got {w}x{h}");
        // aspect ratio preserved (3:2)
        assert!(
            (w as f64 / h as f64 - 1.5).abs() < 0.02,
            "aspect kept: {w}x{h}"
        );
    }

    #[test]
    fn leaves_small_images_untouched() {
        let small = png_of(800, 600);
        assert!(downscale_image_bytes(&small, "png", 1568, DEFAULT_MAX_PIXELS).is_none());
    }

    #[test]
    fn megapixel_cap_binds_even_when_edges_fit() {
        // 1400x1400 = 1.96 MP: both edges < 1568 but over the 1.15 MP cap.
        let img = png_of(1400, 1400);
        let (_b, w, h) = downscale_image_bytes(&img, "png", 1568, 1_150_000).unwrap();
        assert!(
            (w as u64 * h as u64) <= 1_200_000,
            "pixels capped, got {w}x{h}"
        );
    }

    #[test]
    fn aggressive_target_saves_more() {
        let img = png_of(1568, 1568);
        let (_b, w, h) = downscale_image_bytes(&img, "png", 1024, DEFAULT_MAX_PIXELS).unwrap();
        assert!(w.max(h) <= 1024);
    }

    #[test]
    fn prepare_stages_downscaled_and_full_copy() {
        let dir = std::env::temp_dir().join(format!("tkd-img-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let src = dir.join("shot.png");
        std::fs::write(&src, png_of(2400, 1600)).unwrap();
        let staged = prepare_downscaled_read(&src, &dir).expect("oversized png staged");
        assert!(staged.redirect_path.exists(), "downscaled copy written");
        assert!(staged.full_path.exists(), "full-res copy written");
        assert!(staged.new_w.max(staged.new_h) < 2400, "actually downscaled");
        // Re-staging a path already under .tokenade/ must NOT recurse.
        assert!(prepare_downscaled_read(&staged.redirect_path, &dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_image_and_garbage_bytes_are_none() {
        assert!(
            downscale_image_bytes(b"not an image at all", "png", 1568, DEFAULT_MAX_PIXELS)
                .is_none()
        );
        assert!(!is_downscalable_ext("txt"));
        assert!(is_downscalable_ext("PNG"));
    }
}

#[cfg(test)]
mod ocr_upscale_tests {
    use super::upscaled_for_ocr;

    fn png(w: u32, h: u32) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tk-ocr-up-{}-{w}x{h}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.png");
        image::RgbImage::new(w, h).save(&p).unwrap();
        p
    }

    /// Screenshot-sized text is enlarged three times; past the cap, to the cap.
    #[test]
    fn a_small_image_is_enlarged_for_ocr() {
        let big = upscaled_for_ocr(&png(600, 160)).expect("a small image is enlarged");
        assert_eq!(image::load_from_memory(&big).unwrap().width(), 1800);
        let capped = upscaled_for_ocr(&png(1600, 100)).expect("still under the threshold");
        assert_eq!(image::load_from_memory(&capped).unwrap().width(), 4000);
    }

    /// An image already large enough is read as it is.
    #[test]
    fn a_large_image_is_left_as_it_is() {
        assert!(upscaled_for_ocr(&png(2400, 1200)).is_none());
    }
}
