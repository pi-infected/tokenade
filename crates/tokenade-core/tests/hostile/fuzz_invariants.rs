//! Invariant fuzzer over the WHOLE auto-compaction router with REALISTIC
//! format corpora — the missing piece next to `proptest_compactors.rs`
//! (random ASCII never triggers the format detectors, so the per-format
//! compactors were never exercised adversarially).
//!
//! For every generated document (single-format, mixed multi-format with and
//! without `=== section ===` markers, mutated with CRLF / ANSI / multi-byte
//! UTF-8 / mid-line truncation) we assert three invariants through
//! `auto::compact`:
//!
//!   1. NO PANIC — any panic fails the test (the web.rs CJK slice, tabular
//!      slice_col and claude_md strip panics were all this class).
//!   2. BOUNDED INFLATION — compacted body ≤ input tokens + small banner
//!      slack (compaction must never cost more than it saves).
//!   3. NO SILENT LOSS — a unique unindented needle line planted in the doc
//!      either SURVIVES in the output or the output DISCLOSES elision
//!      (`elided` / `more` / `truncated` / `dropped` / `§ref:` / `folded` /
//!      `unparsed`). Silent destruction (the tar-listing / dns / pip / tsc
//!      field-bug family) is the one unforgivable failure mode.
//!
//! Deterministic: xorshift seeded per iteration; failures print the seed.
//! Iterations: 400 by default, override with TOKENADE_FUZZ_ITERS.

use tokenade_core::auto;

// ---------------------------------------------------------------- rng

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn chance(&mut self, pct: u64) -> bool {
        self.next() % 100 < pct
    }
}

// ------------------------------------------------------ format corpora

fn word(rng: &mut Rng) -> String {
    const W: &[&str] = &[
        "alpha", "core", "réseau", "数据", "server", "état", "buffer", "ключ", "handler", "índice",
        "queue", "café",
    ];
    W[rng.below(W.len())].to_string()
}

fn gen_tar(rng: &mut Rng, n: usize) -> String {
    (0..n)
        .map(|i| {
            format!(
                "-rw-r--r-- user/group {} 2024-0{}-01 12:0{} src/{}_{i}.rs",
                rng.below(90000),
                1 + rng.below(9),
                rng.below(10),
                word(rng)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn gen_ls(rng: &mut Rng, n: usize) -> String {
    let mut out = vec![format!("total {}", rng.below(999))];
    out.push("drwxr-xr-x 2 root root 4096 2024-01-01 10:00 .".into());
    out.push("drwxr-xr-x 8 root root 4096 2024-01-01 10:00 ..".into());
    for i in 0..n {
        out.push(format!(
            "-rw-r--r-- 1 user user {} 2024-01-01 10:0{} {}_{i}.log",
            rng.below(999999),
            rng.below(10),
            word(rng)
        ));
    }
    out.join("\n")
}

fn gen_dns(rng: &mut Rng, n: usize) -> String {
    let mut out = vec![
        ";; ->>HEADER<<- opcode: QUERY, status: NOERROR, id: 1234".to_string(),
        ";; ANSWER SECTION:".to_string(),
    ];
    for i in 0..n {
        out.push(format!(
            "host{i}.example.com. {} IN A 10.0.{}.{}",
            rng.below(3600),
            rng.below(255),
            rng.below(255)
        ));
    }
    out.push(";; Query time: 23 msec".into());
    out.join("\n")
}

fn gen_pip(rng: &mut Rng, n: usize) -> String {
    let mut out = Vec::new();
    for i in 0..n {
        out.push(format!("Collecting pkg{i}=={}.0.1", rng.below(9)));
        out.push(format!("  Downloading pkg{i}-1.0-py3-none-any.whl (25 kB)"));
    }
    out.push(format!(
        "Successfully installed {}",
        (0..n)
            .map(|i| format!("pkg{i}-1.0"))
            .collect::<Vec<_>>()
            .join(" ")
    ));
    out.join("\n")
}

fn gen_pytest(rng: &mut Rng, n: usize) -> String {
    let mut out = vec![
        "============================= test session starts =============================="
            .to_string(),
    ];
    for i in 0..n {
        out.push(format!(
            "tests/test_{}.py::test_case_{i} {}",
            word(rng),
            if rng.chance(80) { "PASSED" } else { "FAILED" }
        ));
    }
    out.push(format!(
        "=========== {} passed, 1 failed in 2.1s ===========",
        n
    ));
    out.join("\n")
}

fn gen_gitlog(rng: &mut Rng, n: usize) -> String {
    (0..n)
        .map(|i| {
            format!(
                "commit {:040x}\nAuthor: Dev {} <d@example.com>\nDate:   Mon Jan {} 10:00:00 2026 +0100\n\n    feat: {} change {i}\n",
                rng.next(),
                word(rng),
                1 + rng.below(28),
                word(rng)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn gen_df(rng: &mut Rng, n: usize) -> String {
    let mut out = vec!["Filesystem      Size  Used Avail Use% Mounted on".to_string()];
    for i in 0..n {
        out.push(format!(
            "/dev/sda{i}       {}G   {}G   {}G  {}% /mnt/{}",
            100 + rng.below(900),
            rng.below(100),
            rng.below(100),
            rng.below(100),
            word(rng)
        ));
    }
    out.join("\n")
}

fn gen_ss(rng: &mut Rng, n: usize) -> String {
    let mut out =
        vec!["Netid State  Recv-Q Send-Q Local Address:Port  Peer Address:Port".to_string()];
    for _ in 0..n {
        out.push(format!(
            "tcp   LISTEN 0      4096   0.0.0.0:{}       0.0.0.0:*",
            1024 + rng.below(60000)
        ));
    }
    out.join("\n")
}

/// `netstat -an`, HEADERLESS and with VARIED endpoints.
///
/// `gen_ss` above is the same family but uniform: every row LISTEN on
/// 0.0.0.0, so a template fold speaks for the whole document and nothing is
/// lost by summarising it. This one is the population that actually carries
/// information in the payload — forty distinct peers — and it is the shape
/// the two doors were measured disagreeing on (2026-08-27, content door
/// 1 128 tokens to 22).
fn gen_netstat(rng: &mut Rng, n: usize) -> String {
    let states = ["ESTABLISHED", "TIME_WAIT", "CLOSE_WAIT", "LISTEN"];
    (0..n)
        .map(|_| {
            format!(
                "tcp  0  0  10.{}.{}.{}:{}  172.{}.{}.{}:{}  {}",
                rng.below(255),
                rng.below(255),
                rng.below(255),
                1024 + rng.below(60000),
                rng.below(32),
                rng.below(255),
                rng.below(255),
                rng.below(60000),
                states[rng.below(states.len())]
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn gen_journal(rng: &mut Rng, n: usize) -> String {
    (0..n)
        .map(|i| {
            format!(
                "juil. 03 10:0{}:00 host systemd[1]: Started {} unit {i}.",
                rng.below(10),
                word(rng)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn gen_accesslog(rng: &mut Rng, n: usize) -> String {
    (0..n)
        .map(|i| {
            format!(
                r#"10.0.0.{} - - [03/Jul/2026:10:00:{:02} +0100] "GET /api/v1/item/{i} HTTP/1.1" {} 512 "-" "curl/8.0""#,
                rng.below(255),
                rng.below(60),
                [200u32, 404, 500][rng.below(3)]
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn gen_prometheus(rng: &mut Rng, n: usize) -> String {
    let mut out = vec![
        "# HELP http_requests_total Total requests".to_string(),
        "# TYPE http_requests_total counter".to_string(),
    ];
    for i in 0..n {
        out.push(format!(
            "http_requests_total{{code=\"{}\",path=\"/p{i}\"}} {}",
            [200u32, 500][rng.below(2)],
            rng.below(100000)
        ));
    }
    out.join("\n")
}

fn gen_csv(rng: &mut Rng, n: usize) -> String {
    let mut out = vec!["id,name,size,status".to_string()];
    for i in 0..n {
        out.push(format!(
            "{i},{},{},{}",
            word(rng),
            rng.below(9999),
            rng.below(2)
        ));
    }
    out.join("\n")
}

fn gen_json(rng: &mut Rng, n: usize) -> String {
    let items: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"id":{i},"name":"{}","size":{}}}"#,
                word(rng),
                rng.below(999)
            )
        })
        .collect();
    format!(r#"{{"items":[{}],"total":{n}}}"#, items.join(","))
}

fn gen_yaml(rng: &mut Rng, n: usize) -> String {
    let mut out = vec!["config:".to_string()];
    for i in 0..n {
        out.push(format!("  key_{i}: {}", word(rng)));
    }
    out.join("\n")
}

fn gen_strace(rng: &mut Rng, n: usize) -> String {
    let mut out: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"openat(AT_FDCWD, "/etc/{}{i}", O_RDONLY) = {}"#,
                word(rng),
                rng.below(30)
            )
        })
        .collect();
    out.push("+++ exited with 0 +++".into());
    out.join("\n")
}

fn gen_vmstat(rng: &mut Rng, n: usize) -> String {
    let mut out = vec![
        "procs -----------memory---------- ---swap-- -----io---- -system-- ------cpu-----"
            .to_string(),
        " r  b   swpd   free   buff  cache   si   so    bi    bo   in   cs us sy id wa st"
            .to_string(),
    ];
    for _ in 0..n {
        out.push(format!(
            " {}  0      0 {} 123456 789012    0    0     {}     {}  100  200  5  2 92  1  0",
            rng.below(4),
            100000 + rng.below(900000),
            rng.below(100),
            rng.below(100)
        ));
    }
    out.join("\n")
}

fn gen_cargo(rng: &mut Rng, n: usize) -> String {
    let mut out: Vec<String> = (0..n)
        .map(|i| format!("   Compiling {}_{i} v0.{}.0", word(rng), rng.below(9)))
        .collect();
    out.push("    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.32s".into());
    out.join("\n")
}

fn gen_prose(rng: &mut Rng, n: usize) -> String {
    (0..n)
        .map(|_| {
            format!(
                "Le module {} traite la file {} avant l'étape {}.",
                word(rng),
                word(rng),
                word(rng)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn gen_docker_build(rng: &mut Rng, n: usize) -> String {
    let mut out = vec![
        "#1 [internal] load build definition from Dockerfile".to_string(),
        "#1 DONE 0.0s".to_string(),
    ];
    for i in 0..n {
        out.push(format!(
            "#{} [{}/{}] RUN apt-get install -y {}",
            i + 2,
            i + 1,
            n,
            word(rng)
        ));
        out.push(format!(
            "#{} sha256:{:016x} 25.1MB / 25.1MB done",
            i + 2,
            rng.next()
        ));
        out.push(format!(
            "#{} DONE {}.{}s",
            i + 2,
            rng.below(9),
            rng.below(9)
        ));
    }
    out.push("#9 exporting to image".into());
    out.join("\n")
}

fn gen_kubectl(rng: &mut Rng, n: usize) -> String {
    let mut out = vec!["NAME                     READY   STATUS    RESTARTS   AGE".to_string()];
    for i in 0..n {
        out.push(format!(
            "{}-{i}-{:08x}   1/1     Running   {}          {}d",
            word(rng),
            rng.next() as u32,
            rng.below(5),
            rng.below(90)
        ));
    }
    out.join("\n")
}

fn gen_unzip(rng: &mut Rng, n: usize) -> String {
    let mut out = vec![
        "Archive:  bundle.zip".to_string(),
        "  Length      Date    Time    Name".to_string(),
        "---------  ---------- -----   ----".to_string(),
    ];
    for i in 0..n {
        out.push(format!(
            "     {:>4}  2024-01-01 12:0{}   src/{}_{i}.py",
            rng.below(9999),
            rng.below(10),
            word(rng)
        ));
    }
    out.push("---------                     -------".into());
    out.push(format!(
        "   {:>6}                     {n} files",
        rng.below(99999)
    ));
    out.join("\n")
}

fn gen_netstat_legacy(rng: &mut Rng, n: usize) -> String {
    let mut out = vec![
        "Proto Recv-Q Send-Q Local Address           Foreign Address         State".to_string(),
    ];
    for _ in 0..n {
        out.push(format!(
            "tcp        0      0 127.0.0.1:{:<5}          0.0.0.0:*               LISTEN",
            1024 + rng.below(60000)
        ));
    }
    out.join("\n")
}

fn gen_tsc_diag(rng: &mut Rng, n: usize) -> String {
    let mut out: Vec<String> = (0..n)
        .map(|i| {
            format!(
                "src/{}_{i}.ts({},{}): error TS23{:02}: Type 'number' is not assignable to type 'string'.",
                word(rng),
                1 + rng.below(400),
                1 + rng.below(80),
                rng.below(99)
            )
        })
        .collect();
    out.push(format!("Found {n} errors."));
    out.join("\n")
}

type Gen = fn(&mut Rng, usize) -> String;
/// SOURCE CODE, which this fuzzer never generated and which is what the
/// router keeps destroying: a Python module read as a kernel ring buffer, a
/// TypeScript module read as an AVCHD clip, a Rust file read as an archive
/// listing, a source file read as GraphQL SDL. The
/// three shapes below carry, on purpose, the exact prose that has fooled a
/// detector before — a numbered bibliography, a slash-comment banner, a
/// `schema {`-shaped line — because those are the lines a compactor drops.
fn gen_python(rng: &mut Rng, n: usize) -> String {
    let mut out = vec![
        "\"\"\"Module de test. La bibliographie ci-dessous a déjà valu un dmesg.".to_string(),
        String::new(),
        "References".to_string(),
        "==========".to_string(),
        String::new(),
        "[1] Holt, D., Eick, B., O'Brien, E. Handbook of Computational Group Theory".to_string(),
        "[2] John J. Cannon; Lucien A. Dimino; George Havas; Jane M. Watson".to_string(),
        "[3] PROC. SECOND INTERNAT. CONF. THEORY OF GROUPS, CANBERRA 1973".to_string(),
        "\"\"\"".to_string(),
        "# -*- coding: utf-8 -*-".to_string(),
        "from __future__ import annotations".to_string(),
        "import os  # noqa: F401".to_string(),
    ];
    for i in 0..n {
        out.push(format!("def {}_{i}(arg: str) -> int:", word(rng)));
        out.push(format!(
            "    \"\"\"Retourne la taille de {}.\"\"\"",
            word(rng)
        ));
        out.push(format!(
            "    value = len(arg) + {i}  # type: ignore[operator]"
        ));
        out.push("    return value".to_string());
        out.push(String::new());
    }
    out.join("\n")
}

fn gen_rust(rng: &mut Rng, n: usize) -> String {
    let mut out = vec![
        "// SPDX-License-Identifier: Apache-2.0".to_string(),
        "//".to_string(),
        "//! Un en-tête de licence en commentaires-barres, qui a déjà été lu".to_string(),
        "//! comme une liste de chemins.".to_string(),
        "use std::collections::HashMap;".to_string(),
        "use std::fmt;".to_string(),
    ];
    for i in 0..n {
        out.push(format!(
            "pub fn {}_{i}(input: &str) -> Option<usize> {{",
            word(rng)
        ));
        out.push(format!("    if input.contains(\"{}\") {{", word(rng)));
        out.push(format!("        return Some({i});"));
        out.push("    }".to_string());
        out.push("    None".to_string());
        out.push("}".to_string());
        out.push(String::new());
    }
    out.join("\n")
}

fn gen_graphql(rng: &mut Rng, n: usize) -> String {
    let mut out = vec![
        "directive @auth(requires: Role = ADMIN) on OBJECT | FIELD_DEFINITION".to_string(),
        "schema {".to_string(),
        "  query: Query".to_string(),
        "  mutation: Mutation".to_string(),
        "}".to_string(),
    ];
    for i in 0..n {
        out.push(format!("type {}_{i} {{", word(rng)));
        out.push(format!("  {}: String!", word(rng)));
        out.push(format!("  {}: [Int!]!", word(rng)));
        out.push("}".to_string());
        out.push(String::new());
    }
    out.join("\n")
}

/// The three shapes the source-code pass named as still uncovered, written
/// before the easy ones: markdown (the router's most common CONTENT input),
/// ndjson (whose own pathological-reduction event is in the debug log), and a
/// bare path listing — `tar` was covered but `archive::Format::Plain`, the
/// branch that had to be narrowed, was not.
fn gen_markdown(rng: &mut Rng, n: usize) -> String {
    let mut out = vec![
        "# Titre du document".to_string(),
        String::new(),
        "Un paragraphe d'introduction qui explique la suite.".to_string(),
        String::new(),
    ];
    for i in 0..n {
        out.push(format!("## Section {i} — {}", word(rng)));
        out.push(String::new());
        out.push(format!(
            "Le module {} traite la file {} avant l'étape {}.",
            word(rng),
            word(rng),
            word(rng)
        ));
        out.push(format!("- premier point sur {}", word(rng)));
        out.push(format!("- deuxième point sur {}", word(rng)));
        out.push(String::new());
        if i % 3 == 0 {
            out.push("```bash".to_string());
            out.push(format!("{} --flag {}", word(rng), i));
            out.push("```".to_string());
            out.push(String::new());
        }
    }
    out.join("\n")
}

fn gen_ndjson(rng: &mut Rng, n: usize) -> String {
    (0..n)
        .map(|i| {
            format!(
                "{{\"ts\":\"2026-08-27T0{}:00:0{}Z\",\"level\":\"info\",\"svc\":\"{}\",\"msg\":\"{} {}\",\"n\":{i}}}",
                i % 10,
                i % 10,
                word(rng),
                word(rng),
                word(rng)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn gen_path_listing(rng: &mut Rng, n: usize) -> String {
    (0..n)
        .map(|i| format!("src/{}/{}/{}_{i}.rs", word(rng), word(rng), word(rng)))
        .collect::<Vec<_>>()
        .join("\n")
}

const GENS: &[(&str, Gen)] = &[
    ("tar", gen_tar),
    ("ls", gen_ls),
    ("dns", gen_dns),
    ("pip", gen_pip),
    ("pytest", gen_pytest),
    ("gitlog", gen_gitlog),
    ("df", gen_df),
    ("ss", gen_ss),
    ("netstat", gen_netstat),
    ("journal", gen_journal),
    ("accesslog", gen_accesslog),
    ("prometheus", gen_prometheus),
    ("csv", gen_csv),
    ("json", gen_json),
    ("yaml", gen_yaml),
    ("strace", gen_strace),
    ("vmstat", gen_vmstat),
    ("cargo", gen_cargo),
    ("prose", gen_prose),
    ("docker_build", gen_docker_build),
    ("kubectl", gen_kubectl),
    ("unzip", gen_unzip),
    ("netstat_legacy", gen_netstat_legacy),
    ("tsc", gen_tsc_diag),
    ("python", gen_python),
    ("rust", gen_rust),
    ("graphql", gen_graphql),
    ("markdown", gen_markdown),
    ("ndjson", gen_ndjson),
    ("path_listing", gen_path_listing),
];

// ------------------------------------------------------------ mutations

fn mutate(rng: &mut Rng, doc: String) -> String {
    let mut doc = doc;
    if rng.chance(20) {
        doc = doc.replace('\n', "\r\n");
    }
    if rng.chance(15) {
        // ANSI colour on ~10% of lines.
        doc = doc
            .lines()
            .map(|l| {
                if l.len() % 10 == 0 {
                    format!("\x1b[31m{l}\x1b[0m")
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    if rng.chance(15) {
        // Truncate mid-line (and possibly mid-codepoint boundary-safe input,
        // the COMPACTOR must not panic regardless).
        let cut = doc.len() / 2 + rng.below(doc.len() / 2 + 1);
        let mut c = cut.min(doc.len());
        while c > 0 && !doc.is_char_boundary(c) {
            c -= 1;
        }
        doc.truncate(c);
    }
    if rng.chance(10) {
        doc.push('\n');
        doc.push_str(&"x".repeat(3000)); // huge single line
    }
    doc
}

// ------------------------------------------------------------ the test

const DISCLOSURE_MARKERS: &[&str] = &[
    "elided",
    "more",
    "truncated",
    "dropped",
    "§ref:",
    "folded",
    "unparsed",
    "entries",
    "kept",
    "[tokenade",
];

#[test]
fn fuzz_router_invariants() {
    let iters: u64 = std::env::var("TOKENADE_FUZZ_ITERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(400);
    let mut failures: Vec<String> = Vec::new();

    for seed in 0..iters {
        let mut rng = Rng::new(seed + 1);
        let sections = 1 + rng.below(3);
        let with_markers = rng.chance(50);
        let needle = format!("NEEDLE-{:016x} unique diagnostic marker", rng.next());

        let mut parts: Vec<String> = Vec::new();
        for s in 0..sections {
            let (name, g) = GENS[rng.below(GENS.len())];
            let n = 3 + rng.below(40);
            let body = g(&mut rng, n);
            if with_markers {
                parts.push(format!("=== {name}-{s} ===\n{body}"));
            } else {
                parts.push(body);
            }
        }
        // Plant the needle as a full unindented line at a random boundary.
        //
        // NEGATIVE, measured 2026-08-27, so nobody re-tries it: planting it IN
        // BAND — appended to a line a generator produced — reaches compactors
        // that format purity otherwise keeps unreachable, and that part of the
        // idea is right. But free text appended to a row is not a document any
        // tool emits, and a POSITIONAL table (`netstat`, `ss`, `tabular`)
        // parses by column, so the junk is dropped correctly and reported as a
        // silent loss. Eleven reds, ten of them exactly that; real
        // `netstat -anp` keeps its whole `PID/Program name` column, verified
        // against the documented shape. A realistic in-band needle has to
        // occupy a FIELD the generator's own format defines, which is
        // per-generator work, not one line here.
        let pos = rng.below(parts.len() + 1);
        parts.insert(pos, needle.clone());
        // Invariant 4 payload (scrooge-mode steal): an ERROR-class line whose
        // decisive spans (path:line, error code) must survive VERBATIM — for
        // errors, elision disclosure is NOT enough; the agent needs the
        // actual location to act.
        let err_needle = match rng.below(3) {
            0 => format!(
                "error[E{:04}]: cannot find value `x{:x}` in /src/mod_{:x}.rs:{}",
                rng.below(9999),
                rng.next() & 0xffff,
                rng.next() & 0xfff,
                1 + rng.below(900)
            ),
            // NOT a frame line (`File "…", line N`): middle frames are
            // legitimately elidable with disclosure. The exception line
            // itself is the non-negotiable one.
            1 => format!(
                "ValueError: cannot process /app/pkg_{:x}/data_{}.json",
                rng.next() & 0xfff,
                rng.below(99)
            ),
            _ => format!(
                "FAILED tests/test_core_{:x}.py::test_case_{} - AssertionError",
                rng.next() & 0xfff,
                rng.below(99)
            ),
        };
        let with_err = rng.chance(60);
        if with_err {
            let epos = rng.below(parts.len() + 1);
            parts.insert(epos, err_needle.clone());
        }
        let doc = mutate(&mut rng, parts.join("\n"));
        if !doc.contains(&needle) {
            continue; // truncation mutation ate it — nothing to assert
        }

        // Invariant 1: no panic (a panic aborts the test run with the seed
        // printed by the harness via this catch).
        let result = std::panic::catch_unwind(|| auto::compact(&doc));
        let (kind, body, _sav) = match result {
            Ok(r) => r,
            Err(_) => {
                failures.push(format!("seed {seed}: PANIC in auto::compact"));
                continue;
            }
        };

        // Invariant 2: bounded inflation.
        let before = tokenade_core::tokenizer::estimate(&doc);
        let after = tokenade_core::tokenizer::estimate(&body);
        if after > before.saturating_mul(13) / 10 + 128 {
            failures.push(format!(
                "seed {seed}: INFLATION kind={kind:?} before={before} after={after}"
            ));
        }

        // Invariant 3: no silent loss of the needle.
        let needle_id = &needle[..23]; // "NEEDLE-<16hex>"
        if !body.contains(needle_id) {
            let disclosed = DISCLOSURE_MARKERS.iter().any(|m| body.contains(m));
            if !disclosed {
                failures.push(format!(
                    "seed {seed}: SILENT LOSS kind={kind:?} needle gone, no disclosure"
                ));
            }
        }

        // Invariant 6: THE OTHER DOOR. Everything above exercises
        // `auto::compact`, the CONTENT path. The same bytes arriving as a
        // command's output go through `filter::filter_command` instead, and
        // the two doors have diverged before — that divergence is the parity
        // gap, and it recurred more than once. A fuzzer that only knocks on
        // one of them cannot see it.
        //
        // The program name is deliberately one no compactor claims, so the
        // route is the generic/auto fallback every unknown command takes.
        let argv = vec!["fuzzcmd".to_string()];
        match std::panic::catch_unwind(|| tokenade_core::filter::filter_command(&argv, &doc)) {
            Err(_) => failures.push(format!("seed {seed}: PANIC in filter_command")),
            Ok(r) => {
                // Invariant 7: on a SINGLE-FORMAT document, the command
                // door is never WORSE than the content door.
                //
                // The two are allowed to differ — `Unclaimed::Keep` serves an
                // unclaimed DOCUMENT whole on purpose, so the content
                // side is legitimately the larger one, sometimes by 5x. What is
                // NOT allowed is the other direction: a candidate wired to one
                // path and not the other is the parity-gap defect, and it
                // recurred twice.
                //
                // Restricted to `sections == 1`, and the restriction is the
                // whole point. On a MIXED document the two doors legitimately
                // pick different candidates, and the smaller output is then the
                // one that threw more away — measured on seed 3556, where the
                // content door's metrics compactor came back 199 tokens lighter
                // than a template fold that had KEPT the document's docker
                // section. Fewer tokens is not better; asserting on a mixed doc
                // would be asserting that destruction wins.
                let content_door = {
                    let (_l, out, sav) = tokenade_core::filter::structured_or_generic(
                        &doc,
                        before,
                        kind,
                        body.clone(),
                        tokenade_core::Savings::new(
                            before,
                            tokenade_core::tokenizer::estimate(&body),
                        ),
                        tokenade_core::filter::Unclaimed::Keep,
                    );
                    let _ = sav;
                    tokenade_core::tokenizer::estimate(&out)
                };
                let command_door = tokenade_core::tokenizer::estimate(&r.output);
                if sections == 1 && command_door > content_door + 32 {
                    failures.push(format!(
                        "seed {seed}: DOOR REGRESSION kind={kind:?} filter={} content={content_door} command={command_door}",
                        r.filter
                    ));
                }
                if !r.output.contains(needle_id) {
                    let disclosed = DISCLOSURE_MARKERS.iter().any(|m| r.output.contains(m));
                    if !disclosed {
                        failures.push(format!(
                            "seed {seed}: SILENT LOSS (command door) filter={} needle gone, no disclosure",
                            r.filter
                        ));
                    }
                }
                // Invariant 4, at the OTHER door.
                //
                // "For errors, elision disclosure is NOT enough; the agent
                // needs the actual location to act" is this fuzzer's own
                // fourth invariant, and it has only ever been asked of
                // `auto::compact`. The appendix that enforces it,
                // `dropped_error_lines`, has exactly ONE caller —
                // `auto::guarded` — so a command with a DEDICATED compactor
                // never passes through it at all.
                if with_err && doc.contains(&err_needle) && !r.output.contains(&err_needle) {
                    failures.push(format!(
                        "seed {seed}: ERROR LINE LOST (command door) filter={}: {err_needle}",
                        r.filter
                    ));
                }
            }
        }

        // Invariant 8: the PIPED command door.
        //
        // Until piped commands were routed, an argv carrying a `|` never
        // reached a compactor at all — it was served byte for byte — so no
        // fuzz generator could reach this route and none did. It is a live
        // route now, over 6 465 events and 1.5M tokens of real traffic, and a
        // route no instrument knocks on is where the next silent loss lives
        // (Invariant 6 made exactly this argument about the command door
        // itself).
        //
        // Two claims, and the second is the load-bearing one:
        //   * losing the needle without disclosure is a silent loss here as
        //     anywhere;
        //   * the piped answer must MATCH the unpiped one. That equality is
        //     the entire justification for routing pipes — a pipeline ending in `head`
        //     is the case `head` alone already had — so it is asserted rather
        //     than described, on every generated document.
        let piped = vec![
            "fuzzcmd".to_string(),
            "|".to_string(),
            "head".to_string(),
            "-500".to_string(),
        ];
        let unpiped = vec!["head".to_string(), "-500".to_string()];
        match std::panic::catch_unwind(|| {
            (
                tokenade_core::filter::filter_command(&piped, &doc),
                tokenade_core::filter::filter_command(&unpiped, &doc),
            )
        }) {
            Err(_) => failures.push(format!("seed {seed}: PANIC in filter_command (piped)")),
            Ok((rp, ru)) => {
                if !rp.output.contains(needle_id) {
                    let disclosed = DISCLOSURE_MARKERS.iter().any(|m| rp.output.contains(m));
                    if !disclosed {
                        failures.push(format!(
                            "seed {seed}: SILENT LOSS (piped door) filter={} needle gone, no disclosure",
                            rp.filter
                        ));
                    }
                }
                if rp.output != ru.output {
                    failures.push(format!(
                        "seed {seed}: PIPE DIVERGENCE piped={} ({} tok) unpiped={} ({} tok)",
                        rp.filter, rp.savings.tokens_after, ru.filter, ru.savings.tokens_after
                    ));
                }
            }
        }

        // Invariant 5 (audit C3): a MIXED document (prose paragraphs around
        // a table-ish region, no `=== ===` section markers) must never be
        // committed to a destructive STRUCTURED rewriter — prose lines must
        // survive somewhere in the output.
        if rng.chance(25) {
            let prose = format!(
                "Deployment notes {}: the rollout was gradual and reviewed.",
                rng.below(1000)
            );
            let tbl: String = (0..12)
                .map(|i| format!("{i},{},{}\n", rng.below(100), rng.below(100)))
                .collect();
            let mixed = format!("{prose}\n{tbl}{prose} tail marker {}", rng.below(1000));
            if let Ok((_k, out, _s)) = std::panic::catch_unwind(|| auto::compact(&mixed)) {
                if !out.contains("Deployment notes") && !out.contains("tail marker") {
                    failures.push(format!(
                        "seed {seed}: CROSS-FORMAT rewrite dropped ALL prose from a mixed doc"
                    ));
                }
            }
        }

        // Invariant 4b (C11): SUMMARY/TALLY lines are as decisive as errors
        // (npm-test decapitation class: jest prints failures at the END).
        let tally = format!(
            "Tests: {} failed, {} passed, {} total",
            rng.below(9),
            10 + rng.below(90),
            20 + rng.below(90)
        );
        // reuse err_needle machinery: plant independently
        let with_tally = rng.chance(40);
        let doc = if with_tally {
            format!("{doc}\n{tally}")
        } else {
            doc
        };
        let result2 = std::panic::catch_unwind(|| auto::compact(&doc));
        let (kind, body, _sav) = match result2 {
            Ok(r) => r,
            Err(_) => {
                failures.push(format!("seed {seed}: PANIC in auto::compact (tally pass)"));
                continue;
            }
        };
        if with_tally && doc.contains(&tally) && !body.contains(&tally) {
            failures.push(format!("seed {seed}: TALLY LOST kind={kind:?}: {tally}"));
        }

        // Invariant 4: ERROR lines survive verbatim (mutations may have
        // clipped the planted line — only assert when it reached the doc
        // intact). Disclosure does NOT excuse dropping an error location.
        if with_err && doc.contains(&err_needle) && !body.contains(&err_needle) {
            failures.push(format!(
                "seed {seed}: ERROR LINE LOST kind={kind:?}: {err_needle}"
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} invariant failure(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The needle in a FIELD, across every generator.
///
/// `fuzz_router_invariants` plants its needle as a line of its own, and a
/// measurement showed why that is not enough: a foreign line changes what the
/// document detects as, so every compactor demanding format purity is
/// unreachable in the presence of the very marker whose survival is asserted.
/// Planting it in a field by hand for five formats found `strace` destroying
/// its paths.
///
/// This does it for all of them, mechanically and realistically: substitute
/// the needle for the longest name-like token on a MIDDLE line. Same column
/// count, same shape, one unusual value — which is what a real document is.
///
/// It found five compactors dropping that value with nothing in the output to
/// say a line went away — `ss`, `netstat`, `netstat_legacy` (one compactor,
/// three shapes), `strace` and `ndjson`. Each of them already ended some OTHER
/// list with `… N more … elided`; the rule was spelled in one block of the
/// same function and not in its neighbour.
#[test]
fn a_field_value_is_never_lost_without_disclosure() {
    const MARKERS: &[&str] = &[
        "elided",
        "more",
        "truncated",
        "dropped",
        "\u{a7}ref:",
        "folded",
        "unparsed",
        "entries",
        "kept",
        "[tokenade",
        "not shown",
        "omitted",
        "[stashed",
        "[disclose",
    ];
    let needle = "zqxmarker7714";
    let mut failures: Vec<String> = Vec::new();
    let mut reached = 0usize;
    for (name, g) in GENS {
        let mut rng = Rng::new(99);
        let doc = g(&mut rng, 40);
        let lines: Vec<&str> = doc.lines().collect();
        // Walk OUTWARD from the middle for a line that actually carries a
        // name-like field, instead of insisting on the middle line itself.
        //
        // Four of the twenty-nine were unreachable without this and the guard
        // still passed: `gitlog` (its middle line is the blank one inside a
        // commit, or a `Date:` of pure digits), `json` (one enormous line, so
        // "fewer than six lines" skipped it), `vmstat` (every column numeric)
        // and `rust`. A generator the plant cannot reach is a generator the
        // guard does not test, and it reports that as a clean sweep.
        let mid = lines.len() / 2;
        let mut found: Option<(usize, String)> = None;
        for step in 0..lines.len() {
            let li = if step % 2 == 0 {
                mid.saturating_add(step / 2)
            } else {
                mid.saturating_sub(step.div_ceil(2))
            };
            if li >= lines.len() {
                continue;
            }
            let target = lines[li];
            if let Some(tok) = target
                .split(|c: char| {
                    c.is_whitespace()
                        || matches!(c, '"' | ',' | '(' | ')' | '=' | ':' | '[' | ']' | '{' | '}')
                })
                .filter(|t| t.len() >= 4 && t.chars().any(|c| c.is_ascii_alphabetic()))
                .max_by_key(|t| t.len())
            {
                found = Some((li, target.replacen(tok, needle, 1)));
                break;
            }
        }
        let Some((li, planted)) = found else { continue };
        let mut ls = lines.clone();
        ls[li] = &planted;
        let doc2 = ls.join("\n");
        if !doc2.contains(needle) {
            continue;
        }
        reached += 1;
        let (kind, out, _s) = auto::compact(&doc2);
        let cmd = tokenade_core::filter::filter_command(&["fuzzcmd".to_string()], &doc2);
        // THE THIRD DOOR. `compact_with_path_hint` is not `compact` with an
        // extra argument: a measurement showed that the path hint WINS OUTRIGHT
        // rather than competing with content detection, because a filename is
        // an assertion by the user. That makes it a different router, and no
        // invariant here has ever knocked on it.
        //
        // The name is the plausible one for each corpus, which is the whole
        // point — a hint only decides anything when somebody would really have
        // written that name.
        let hint = match *name {
            "csv" => "data.csv",
            "json" => "config.json",
            "yaml" => "compose.yaml",
            "markdown" => "README.md",
            "ndjson" => "events.ndjson",
            "python" => "mod.py",
            "rust" => "lib.rs",
            "gitlog" | "journal" | "accesslog" | "docker_build" | "cargo" => "run.log",
            _ => "out.txt",
        };
        let (hkind, hout, _hs) = auto::compact_with_path_hint(&doc2, hint);
        if hkind != kind {
            let b = tokenade_core::tokenizer::estimate(&doc2);
            eprintln!(
                "HINTDIFF {name:<14} content={kind:?}->{} hinted={hkind:?}->{}  (raw {b})",
                tokenade_core::tokenizer::estimate(&out),
                tokenade_core::tokenizer::estimate(&hout)
            );
        }
        for (door, o) in [
            ("content", &out),
            ("command", &cmd.output),
            ("path-hint", &hout),
        ] {
            if !o.contains(needle) && !MARKERS.iter().any(|m| o.contains(m)) {
                failures.push(format!(
                    "{name} ({kind:?}) {door} door: field value gone, no disclosure"
                ));
            }
        }
    }
    // The instrument must be doing work: a planting that stopped reaching the
    // generators would report a clean sweep.
    assert!(
        reached >= 29,
        "only {reached} of {} generators were reachable — the plant is broken, \
         not the compactors clean",
        GENS.len()
    );
    assert!(
        failures.is_empty(),
        "{} silent field losses:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
