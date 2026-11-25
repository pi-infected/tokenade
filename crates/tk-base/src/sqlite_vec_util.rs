//! Small sqlite-vec helpers shared by anyone touching the vector
//! tables. Extracted from the (deleted) `memory` module so that the
//! semantic-search path keeps working after the custom memory system
//! was retired in favour of Claude Code's native auto-memory.
//!
//! There are exactly two helpers:
//!   * [`register_sqlite_vec_extension`] — wires sqlite-vec into every
//!     new rusqlite `Connection`. Idempotent via `Once`.
//!   * [`vec_f32_as_bytes`] — re-interprets a `Vec<f32>` as the
//!     little-endian byte buffer sqlite-vec stores under the hood.

use std::sync::Once;

/// Re-interpret a `Vec<f32>` as a flat little-endian byte slice for the
/// sqlite-vec wire format. f32 has no padding (4 bytes per element);
/// little-endian is the universal format that sqlite-vec expects across
/// both x86 and ARM platforms. Uses `zerocopy` to avoid hand-rolling
/// unsafe transmutes.
pub fn vec_f32_as_bytes(v: &[f32]) -> Vec<u8> {
    use zerocopy::AsBytes;
    v.as_bytes().to_vec()
}

/// Register sqlite-vec as an SQLite auto-extension. Idempotent — the
/// `Once` guard means we only call `sqlite3_auto_extension` once per
/// process.
///
/// `sqlite3_auto_extension` registers a function pointer that SQLite
/// calls when each new `Connection` is opened, so this works for every
/// SQLite connection without us having to thread the extension through
/// each `open` site.
pub fn register_sqlite_vec_extension() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: sqlite-vec's `sqlite3_vec_init` function follows the
        // standard sqlite3_auto_extension contract:
        //   fn(*mut sqlite3, *mut *mut c_char, *const sqlite3_api_routines) -> c_int
        // We transmute its `*const ()` pointer to the exact signature
        // rusqlite's FFI expects. Both pointers point at the same entry
        // symbol the C runtime would call.
        type AutoExtFn = unsafe extern "C" fn(
            *mut rusqlite::ffi::sqlite3,
            *mut *mut std::os::raw::c_char,
            *const rusqlite::ffi::sqlite3_api_routines,
        ) -> std::os::raw::c_int;
        // SAFETY: `sqlite3_vec_init` has exactly the entry-point signature `AutoExtFn` spells
        // out, the one sqlite3_auto_extension expects.
        unsafe {
            let init_ptr = sqlite_vec::sqlite3_vec_init as *const ();
            let init_fn: AutoExtFn = std::mem::transmute(init_ptr);
            rusqlite::ffi::sqlite3_auto_extension(Some(init_fn));
        }
    });
}
