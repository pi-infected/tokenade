// The tests of `crate::file_search`, moved out of the production file.
//
// Mounted by `include!`: the tokens are spliced INTO `file_search`, so every
// `super::` keeps its meaning. Split by `srcsweep::production_lines`.

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;
    use std::sync::Mutex;

    fn io_lock() -> &'static Mutex<()> {
        static LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    // ── identifier expansion for the per-file BM25 arm ──

    // ── brace-balanced extraction (expand_to_balanced_region) ──

    #[test]
    fn expand_returns_full_enclosing_function() {
        let src = "\
fn outer() {
    let a = 1;
    let b = 2;
    let c = a + b;
    println!(\"{}\", c);
}
";
        // Window lands mid-body (lines 3..4). Expect expansion to 1..6.
        let (s, e, snip) = expand_to_balanced_region(src, 3, 4, EXTRACT_MAX_LINES).unwrap();
        assert_eq!((s, e), (1, 6), "should expand to the whole fn body");
        assert!(snip.starts_with("fn outer() {"));
        assert!(snip.trim_end().ends_with('}'));
    }

    #[test]
    fn expand_noop_on_prose_without_brackets() {
        let text = "\
first line of prose
second line of prose
third line of prose
fourth line of prose
";
        // No brackets anywhere → nothing to balance → keep the window.
        assert!(expand_to_balanced_region(text, 2, 3, EXTRACT_MAX_LINES).is_none());
    }

    #[test]
    fn expand_noop_when_block_exceeds_cap() {
        let mut src = String::from("fn big() {\n");
        for i in 0..200 {
            src.push_str(&format!("    let v{i} = {i};\n"));
        }
        src.push_str("}\n");
        // Enclosing block is ~202 lines > cap → fall back to window.
        assert!(expand_to_balanced_region(&src, 50, 51, EXTRACT_MAX_LINES).is_none());
    }

    #[test]
    fn expand_ignores_braces_in_strings_and_comments() {
        let src = "\
fn f() {
    let s = \"} not a real close {\";
    // } also not real
    let x = 1;
}
";
        let (s, e, _) = expand_to_balanced_region(src, 4, 4, EXTRACT_MAX_LINES).unwrap();
        assert_eq!((s, e), (1, 5), "string/comment braces must not unbalance");
    }

    /// RAII guard: deletes the tmp db on drop. Without this every test
    /// run leaks a ~1.6 MB SQLite DB into tmpfs (RAM-backed). A 47-DB
    /// pile-up was observed in the wild.
    pub(super) struct DbGuard {
        path: std::path::PathBuf,
        _io: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for DbGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.path);
            // SAFETY: env var is process-wide; io_lock serialises.
            unsafe {
                std::env::remove_var("TOKENADE_FILE_SEARCH_DB");
            }
        }
    }

    fn isolated_db() -> DbGuard {
        let guard = io_lock().lock().unwrap_or_else(|p| p.into_inner());
        let p = std::env::temp_dir().join(format!(
            "tokenade-file-search-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&p);
        // SAFETY: env var is process-wide; io_lock serialises tests.
        unsafe {
            std::env::set_var("TOKENADE_FILE_SEARCH_DB", &p);
        }
        DbGuard {
            path: p,
            _io: guard,
        }
    }

    fn seed(conn: &Connection, hash: &str, snippet: &str) {
        conn.execute(
            "INSERT INTO file_chunks (content_hash, chunk_id, line_start, line_end, snippet)
             VALUES (?1, 0, 1, 2, ?2)",
            params![hash, snippet],
        )
        .unwrap();
    }

    #[test]
    fn purge_containing_drops_the_file_that_carries_the_needle_and_no_other() {
        let _g = isolated_db();
        {
            let conn = open().unwrap();
            seed(&conn, "leaky", "token = hunter2-XYZ");
            seed(&conn, "clean", "pub fn deploy() {}");
        }
        assert_eq!(purge_containing("hunter2-XYZ", true), (1, 0), "dry run");
        assert_eq!(
            open()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM file_chunks", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            2,
            "a dry run removes nothing"
        );
        assert_eq!(purge_containing("hunter2-XYZ", false), (1, 1));
        let left: Vec<String> = {
            let conn = open().unwrap();
            let mut st = conn
                .prepare("SELECT content_hash FROM file_chunks")
                .unwrap();
            let rows = st
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .filter_map(|r| r.ok())
                .collect();
            rows
        };
        assert_eq!(left, vec!["clean".to_string()]);
    }

    #[test]
    fn a_purged_secret_is_not_still_readable_in_the_file() {
        let _g = isolated_db();
        // Enough rows to fill real pages: a delete hands them to SQLite's own
        // freelist, where the text stays readable until the file is rewritten.
        {
            let conn = open().unwrap();
            for i in 0..300 {
                conn.execute(
                    "INSERT INTO file_chunks (content_hash, chunk_id, line_start, line_end, snippet)
                     VALUES ('leaky', ?1, 1, 2, ?2)",
                    params![i, format!("filler {i} token = hunter2-XYZ padding padding padding")],
                )
                .unwrap();
            }
        }
        let path = db_path();
        assert!(
            std::fs::read(&path)
                .unwrap()
                .windows(13)
                .any(|w| w == b"hunter2-XYZ p"),
            "the seed must be in the file, or the test proves nothing"
        );
        assert_eq!(purge_containing("hunter2-XYZ", false), (1, 1));
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            !bytes.windows(11).any(|w| w == b"hunter2-XYZ"),
            "scrubbed, and `grep` still finds it"
        );
    }

    #[test]
    fn purge_redactable_drops_only_what_the_rules_recognise() {
        let _g = isolated_db();
        {
            let conn = open().unwrap();
            seed(
                &conn,
                "leaky",
                "aws_secret_access_key = AKIAIOSFODNN7EXAMPLE",
            );
            seed(&conn, "clean", "pub fn deploy() {}");
        }
        assert_eq!(purge_redactable(true), 1);
        assert_eq!(purge_redactable(false), 1);
        assert_eq!(purge_redactable(false), 0, "nothing left to recognise");
        assert_eq!(
            open()
                .unwrap()
                .query_row("SELECT COUNT(*) FROM file_chunks", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    // ── embedding-model invalidation ──

    /// Seed one cached chunk + its vector. Returns the row count actually stored.
    fn seed_chunk(conn: &Connection) {
        conn.execute(
            "INSERT INTO file_chunks (content_hash, chunk_id, line_start, line_end, snippet)
             VALUES ('abc', 0, 1, 2, 'fn f() {}')",
            [],
        )
        .unwrap();
        let v: Vec<f32> = vec![0.1; crate::embedding::DIM];
        let bytes = crate::sqlite_vec_util::vec_f32_as_bytes(&v);
        conn.execute(
            "INSERT INTO file_chunk_vec(rowid, embedding) VALUES(1, ?1)",
            params![bytes],
        )
        .unwrap();
    }

    fn chunk_count(conn: &Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM file_chunks", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn same_model_keeps_the_cache() {
        let _g = isolated_db();
        let conn = open().unwrap();
        seed_chunk(&conn);
        drop(conn);
        // Reopen with the SAME embedder: re-indexing every file on every run
        // would be a real cost, so an unchanged fingerprint must be a no-op.
        let conn = open().unwrap();
        assert_eq!(chunk_count(&conn), 1, "unchanged model must not wipe");
    }

    #[test]
    fn model_change_wipes_the_cache() {
        let _g = isolated_db();
        let conn = open().unwrap();
        seed_chunk(&conn);
        // Simulate the swap the dim check cannot see: potion-code-16M and
        // potion-multilingual-128M are both 256-dim, so only the fingerprint
        // separates them.
        conn.execute(
            "UPDATE embed_model SET fingerprint = 'some-other-model-v1' WHERE id = 1",
            [],
        )
        .unwrap();
        drop(conn);
        let conn = open().unwrap();
        assert_eq!(chunk_count(&conn), 0, "stale vectors must be dropped");
        let vecs: i64 = conn
            .query_row("SELECT COUNT(*) FROM file_chunk_vec", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            vecs, 0,
            "vector table must be dropped too, not just orphaned"
        );
        // And the new identity is recorded, so the next open is a no-op.
        let fp: String = conn
            .query_row(
                "SELECT fingerprint FROM embed_model WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(fp, crate::embedding::model_fingerprint());
    }

    #[test]
    fn unknown_provenance_is_treated_as_a_mismatch() {
        let _g = isolated_db();
        let conn = open().unwrap();
        seed_chunk(&conn);
        // A DB written before fingerprinting existed: the vectors inside came
        // from some model we cannot name, so they cannot be trusted.
        conn.execute("DROP TABLE embed_model", []).unwrap();
        drop(conn);
        let conn = open().unwrap();
        assert_eq!(
            chunk_count(&conn),
            0,
            "un-attributed vectors must be dropped"
        );
    }

    #[test]
    fn chunking_overlaps_correctly() {
        let mut text = String::new();
        for i in 0..30 {
            let _ = writeln!(text, "line {i}");
        }
        let chunks = chunk_lines(&text);
        // 30 lines, 12 per chunk with 2 overlap (step=10): expect 3 chunks
        // [1-12], [11-22], [21-30]
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].0, 1);
        assert_eq!(chunks[0].1, 12);
        assert_eq!(chunks[1].0, 11);
        assert_eq!(chunks[1].1, 22);
        assert_eq!(chunks[2].0, 21);
        assert_eq!(chunks[2].1, 30);
    }

    #[test]
    fn chunking_empty_yields_nothing() {
        assert!(chunk_lines("").is_empty());
    }

    #[test]
    fn chunking_short_file_yields_one_chunk() {
        let chunks = chunk_lines("a\nb\nc\n");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].0, 1);
        assert_eq!(chunks[0].1, 3);
    }

    // ── L121 language autodetect from extension ──

    #[test]
    fn language_from_path_recognises_common_extensions() {
        use std::path::Path;
        assert_eq!(language_from_path(Path::new("foo.rs")), Some("rust"));
        assert_eq!(language_from_path(Path::new("foo.py")), Some("python"));
        assert_eq!(language_from_path(Path::new("foo.js")), Some("javascript"));
        assert_eq!(language_from_path(Path::new("foo.tsx")), Some("tsx"));
        assert_eq!(language_from_path(Path::new("FOO.RS")), Some("rust")); // case
        assert_eq!(language_from_path(Path::new("foo.cpp")), Some("cpp"));
        assert_eq!(language_from_path(Path::new("foo.hpp")), Some("cpp"));
        assert_eq!(language_from_path(Path::new("foo.kts")), Some("kotlin"));
    }

    #[test]
    fn language_from_path_rejects_unknown() {
        use std::path::Path;
        assert_eq!(language_from_path(Path::new("foo.zzz")), None);
        assert_eq!(language_from_path(Path::new("Makefile")), None);
        assert_eq!(language_from_path(Path::new("foo")), None);
    }

    #[test]
    fn chunk_file_uses_ast_when_extension_known() {
        use std::path::Path;
        let src = "fn foo() { 1 + 2 }\n";
        let ast = chunk_file(Path::new("foo.rs"), src);
        // AST chunker emits at least one chunk for a non-empty Rust file.
        assert!(!ast.is_empty());
        assert!(ast.iter().any(|(_, _, s)| s.contains("fn foo")));
    }

    #[test]
    fn chunk_file_falls_back_to_lines_for_unknown() {
        use std::path::Path;
        let src = "a\nb\nc\n";
        let by_file = chunk_file(Path::new("foo.unknown"), src);
        let by_lines = chunk_lines(src);
        assert_eq!(by_file, by_lines);
    }

    // ── L114 AST chunking ──

    #[test]
    fn ast_chunk_empty_yields_nothing() {
        assert!(chunk_ast("", "rust").is_empty());
        assert!(chunk_ast("   \n", "rust").is_empty());
    }

    #[test]
    fn ast_chunk_unknown_language_falls_back_to_lines() {
        let s = "line1\nline2\nline3\n";
        let line_out = chunk_lines(s);
        let ast_out = chunk_ast(s, "klingon");
        assert_eq!(ast_out, line_out);
    }

    #[test]
    fn ast_chunk_keeps_function_in_one_chunk() {
        // A function body bigger than 12 lines (line chunker would
        // split it) but smaller than AST_CHUNK_BYTES (AST chunker
        // packs it whole).
        let s = "fn foo() {\n    let a = 1;\n    let b = 2;\n    let c = 3;\n    let d = 4;\n    let e = 5;\n    let f = 6;\n    let g = 7;\n    let h = 8;\n    let i = 9;\n    let j = 10;\n    let k = 11;\n    a + b\n}\n";
        let chunks = chunk_ast(s, "rust");
        // Whole function should be in a single chunk (it's <1 KB).
        let combined: String = chunks.iter().map(|(_, _, sn)| sn.as_str()).collect();
        assert!(combined.contains("fn foo()"));
        assert!(combined.contains("a + b"));
        // No split between `fn foo() {` and `}` — verify the chunk
        // boundary contains the function entirely.
        let fn_chunk = chunks
            .iter()
            .find(|(_, _, sn)| sn.contains("fn foo()"))
            .expect("fn foo() chunk");
        assert!(
            fn_chunk.2.contains("a + b"),
            "function body should live in same chunk as signature"
        );
    }

    #[test]
    fn ast_chunk_splits_giant_module() {
        // Build a Rust file with many short functions totalling >> AST_CHUNK_BYTES.
        let mut src = String::new();
        for i in 0..200 {
            let _ = writeln!(src, "fn func_{i}() {{ println!(\"{i}\"); }}");
        }
        let chunks = chunk_ast(&src, "rust");
        assert!(
            chunks.len() > 1,
            "expected multiple chunks for >>1 KB input"
        );
        // Every chunk should be under 1.5× the target (allow some
        // slack for the merge_adjacent pass).
        for (_, _, snippet) in &chunks {
            assert!(
                snippet.len() <= AST_CHUNK_BYTES * 2,
                "chunk too big: {} bytes",
                snippet.len()
            );
        }
    }

    #[test]
    fn ast_chunk_python_class() {
        let src = "class Foo:\n    def __init__(self):\n        self.x = 1\n        self.y = 2\n\n    def method(self):\n        return self.x + self.y\n";
        let chunks = chunk_ast(src, "python");
        assert!(!chunks.is_empty());
        let combined: String = chunks.iter().map(|(_, _, s)| s.as_str()).collect();
        assert!(combined.contains("class Foo"));
        assert!(combined.contains("def __init__"));
    }

    #[test]
    fn evict_stale_drops_old_chunks() {
        let _guard = isolated_db();
        let conn = open().unwrap();
        // Insert a hash with one chunk + matching vec row, all stale.
        let ancient = (crate::platform::now_unix_secs() - (CACHE_TTL_SECS + 60)) as i64;
        conn.execute(
            "INSERT INTO file_chunks(content_hash, chunk_id, line_start, line_end, snippet, last_accessed) \
             VALUES('staleHash', 0, 1, 5, 'old content', ?1)",
            params![ancient],
        )
        .unwrap();
        let rowid: i64 = conn
            .query_row(
                "SELECT rowid FROM file_chunks WHERE content_hash='staleHash'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        // Embedding is `DIM` zeros — match the current embedder so the
        // vec0 schema validates. Was 384 for E5Small, now 256 for
        // potion-code-16M (L112).
        let zeros: Vec<u8> = vec![0u8; crate::embedding::DIM * 4];
        conn.execute(
            "INSERT INTO file_chunk_vec(rowid, embedding) VALUES(?1, ?2)",
            params![rowid, zeros],
        )
        .unwrap();
        let removed = evict_stale().unwrap();
        assert_eq!(removed, 1);
        assert!(!is_indexed("staleHash"));
    }

    #[test]
    fn hash_matches_dedup_ref() {
        // The content hash must round-trip through the §ref:HASH§
        // marker convention so callers can chain expand_ref + file_search.
        let content = "the file body";
        let h1 = hash_file(content);
        let h2 = crate::dedup_ref::hash_content(content.as_bytes());
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 12);
    }

    // --- content-based language detection ---------------------------

    #[test]
    fn shebang_resolves_extensionless_script() {
        let p = std::path::Path::new("deploy");
        let src = "#!/usr/bin/env python3\nimport sys\n\ndef main():\n    print(sys.argv)\n";
        assert_eq!(detect_language(p, src), Some("python"));
        // The other shebang shapes we claim to handle.
        assert_eq!(
            language_from_shebang("#!/usr/bin/python3.11\n"),
            Some("python")
        );
        assert_eq!(
            language_from_shebang("#!/usr/bin/env node\n"),
            Some("javascript")
        );
        assert_eq!(
            language_from_shebang("#!/usr/bin/env -S ruby -w\n"),
            Some("ruby")
        );
        // Interpreters we have no grammar for must stay honest.
        assert_eq!(language_from_shebang("#!/bin/bash\n"), None);
        assert_eq!(language_from_shebang("#!/usr/bin/perl\n"), None);
    }

    #[test]
    fn well_known_filename_resolves_by_name() {
        let src = "source 'https://rubygems.org'\n\ngem 'rails', '~> 7.0'\n";
        assert_eq!(
            detect_language(std::path::Path::new("Gemfile"), src),
            Some("ruby")
        );
        assert_eq!(
            language_from_file_name(std::path::Path::new("/a/b/Rakefile")),
            Some("ruby")
        );
        assert_eq!(
            language_from_file_name(std::path::Path::new("SConstruct")),
            Some("python")
        );
        // We vendor no grammar for these, so they must NOT resolve.
        assert_eq!(
            language_from_file_name(std::path::Path::new("Makefile")),
            None
        );
        assert_eq!(
            language_from_file_name(std::path::Path::new("Dockerfile")),
            None
        );
    }

    #[test]
    fn parse_trial_resolves_clear_python_without_extension() {
        // No extension, no shebang, no known name — trial only.
        let src = "\
import os
from collections import defaultdict


class Counter:
    def __init__(self, name):
        self.name = name
        self.seen = defaultdict(int)

    def add(self, key):
        self.seen[key] += 1
        return self.seen[key]


def main():
    c = Counter(os.environ.get('NAME', 'x'))
    for line in open('data.txt'):
        c.add(line.strip())
";
        assert_eq!(language_by_parse_trial(src), Some("python"));
        assert_eq!(
            detect_language(std::path::Path::new("snippet"), src),
            Some("python")
        );
    }

    #[test]
    fn trial_returns_none_rather_than_guessing() {
        // THE important case: a wrong guess is worse than no guess.

        // 1. English prose. Parses with few errors under permissive
        //    grammars but yields zero structural nodes.
        let prose = "The quick brown fox jumps over the lazy dog. \
            This paragraph exists to look vaguely like tokens without \
            being code in any language at all.";
        assert_eq!(language_by_parse_trial(prose), None);

        // 2. Config / data, not code.
        let ini = "[server]\nhost = localhost\nport = 8080\ntimeout = 30\nretries = 5\n";
        assert_eq!(language_by_parse_trial(ini), None);

        // 3. Too short to judge — every grammar ties.
        assert_eq!(language_by_parse_trial("x = 1\n"), None);

        // 4. Over the cost ceiling: skipped outright, even though the
        //    content is unambiguous Python.
        let big = format!("def f():\n    return 1\n{}", "# pad\n".repeat(20_000));
        assert!(big.len() > TRIAL_MAX_BYTES);
        assert_eq!(language_by_parse_trial(&big), None);

        // And the whole pipeline degrades to the line window.
        assert_eq!(detect_language(std::path::Path::new("notes"), prose), None);
        assert!(!chunk_file(std::path::Path::new("notes"), prose).is_empty());
    }

    #[test]
    fn known_extension_still_resolves_unchanged() {
        // No regression: the extension path wins and never reaches the
        // shebang / filename / trial logic.
        for (name, want) in [
            ("a.rs", "rust"),
            ("a.py", "python"),
            ("a.mjs", "javascript"),
            ("a.tsx", "tsx"),
            ("a.go", "go"),
            ("a.hpp", "cpp"),
            ("a.rb", "ruby"),
            ("a.css", "css"),
        ] {
            let p = std::path::Path::new(name);
            assert_eq!(language_from_path(p), Some(want), "{name}");
            assert_eq!(detect_language(p, ""), Some(want), "{name}");
        }
        assert_eq!(
            language_from_path(std::path::Path::new("a.unknownext")),
            None
        );

        // A .py file whose content is misleading still resolves by
        // extension — content is only consulted when the path is mute.
        let p = std::path::Path::new("weird.py");
        assert_eq!(detect_language(p, "#!/usr/bin/env node\n"), Some("python"));
    }
}
