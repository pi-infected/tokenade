//! **Does a fold send the agent looking for what it removed?**
//!
//! All the ratchets in this repo measure the TEXT — fidelity, needles
//! found, fabrications. None measures the CONSEQUENCE: does the agent that receives
//! the folded output then do the right thing, or does it retrace its steps?
//!
//! The question is not answered by reading markdown. It is answered by reading
//! what the agent DID afterwards, and that trace exists: the Claude Code
//! transcripts, `~/.claude/projects/**/*.jsonl`. **758 files, 1.3 GB, 37
//! projects** on the measuring machine as of 2026-09-11 — real sessions, on
//! real repos, most of which are not this one.
//!
//! **The joinable signal, and why it is exact.** Each fold prints a
//! banner carrying a HASH (`tokenade expand-ref a09ff10b5d8a`). When the agent
//! is not satisfied with it, it runs `tokenade expand-ref <that hash>`. The hash JOINS
//! both ends: we know which fold, precisely, sent the agent looking —
//! and the banner also names the FAMILY that produced it (`[tokenade:said-once]`,
//! `[disclose:summary kind=md]`, `[tokenade:template-fold]`…). The return rate
//! PER FAMILY can therefore be computed, and it is what says which fold costs the
//! agent rather than making it gain.
//!
//! **What this rate is NOT.** `expand-ref` is the intended recovery
//! path; taking it proves that it works, not that there is a fault. A high
//! rate on a family says that this family folds what the agent needs —
//! nothing more, and that is already a lot. The truly costly case, where
//! the agent does NOT come back and carries on with amputated information, leaves no
//! such trace: it will take another signal, and it is written here so as not to
//! confuse the two.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// This file is the READER: open the transcripts, pull the blocks out of them, and
// recognise a fold banner. It MEASURES nothing. The four children each ask
// a distinct question about what the agent did next, and
// `pub use` puts them back at the `transcripts::` path where the bench already calls them.
mod families;
mod recovery;
mod refusal;
mod repeats;
pub use families::*;
pub use recovery::*;
pub use refusal::*;
pub use repeats::*;

/// The transcripts directory. `TOKENADE_TRANSCRIPT_ROOT` redirects it — that is
/// how the canary makes it read a fabricated tree.
pub fn transcript_root() -> PathBuf {
    std::env::var_os("TOKENADE_TRANSCRIPT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default())
                .join(".claude")
                .join("projects")
        })
}

/// The family a banner announces, as it writes it.
///
/// `[tokenade:rs]`, `[disclose:summary kind=md]`, `[tokenade:exec-dedup]` — the
/// name comes from the banner, never from a list of known families (a
/// list of names encountered is not the specification, and a new family
/// must show up on its own).
fn family_of(text: &str, at: usize) -> String {
    // **The banner that CONTAINS the hash, and it alone.**
    //
    // Two laxer attributions were tried and they BOTH
    // LIE. "The closest marker before the hash" gave
    // `tokenade:toml` **58.8 %** returns — yet that fold only removes
    // comments and prints no hash: it was simply the last
    // marker before ANOTHER fold's banner, in an output that carried
    // several. "The first marker of the line" left 616 folds nameless.
    //
    // The only exact attachment is the bracket that opens the banner where the hash
    // is written: we walk back counting closed brackets.
    let head = &text.as_bytes()[..at];
    let mut depth = 0i32;
    let mut open = None;
    for i in (0..head.len()).rev() {
        match head[i] {
            b']' => depth += 1,
            b'[' => {
                if depth == 0 {
                    open = Some(i);
                    break;
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    let Some(i) = open else {
        return "(no banner)".to_string();
    };
    let rest = &text[i + 1..];
    if let Some(k) = rest.strip_prefix("disclose:summary") {
        let kind = k
            .split("kind=")
            .nth(1)
            .and_then(|k| {
                k.split(|c: char| !c.is_ascii_alphanumeric() && c != '.' && c != '_')
                    .next()
            })
            .filter(|k| !k.is_empty())
            .unwrap_or("?");
        return format!("disclose:{kind}");
    }
    let Some(rest) = rest.strip_prefix("tokenade") else {
        // `[stashed: …]` is the product's only other footer that carries a
        // hash. Everything else is some bracket in the preceding prose
        // — a bracketed sentence, a table row — and accepting it
        // fabricated "families" like `qualification-rate-is-not-y` at
        // 100 % returns over three folds. The list is short and EXPLICIT
        // rather than guessed: a new footer must be added here, and its absence
        // shows in the count of discarded mentions.
        return if rest.starts_with("stashed:") {
            "stashed".to_string()
        } else {
            "(no banner)".to_string()
        };
    };
    let rest = rest.strip_prefix(':').unwrap_or(rest);
    // The GENERIC form is written `[tokenade: 3079 tokens folded away …`: its
    // "name" would be a count, so each fold would make its own family.
    // The colon is PART of the name: the ledger writes `auto:grep`,
    // `auto:html`, `disclose:stash`. Stopping at the first `:` would file fifteen
    // compactors under a single `tokenade:auto`.
    let name: String = rest
        .chars()
        .take_while(|c| {
            c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.' || *c == ':'
        })
        .collect();
    if name.is_empty() || name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return "tokenade:generic".to_string();
    }
    format!("tokenade:{name}")
}

/// The hashes a banner text announces, with the family that produced them.
fn folds_in(text: &str, out: &mut Vec<(String, String)>) {
    if is_resume_mask(text) {
        return;
    }
    let needle = "expand-ref ";
    let mut from = 0usize;
    while let Some(i) = text[from..].find(needle) {
        let at = from + i;
        let rest = &text[at + needle.len()..];
        let hash: String = rest.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        from = at + needle.len();
        if hash.len() < 8 {
            continue;
        }
        out.push((hash, family_of(text, at)));
    }
}

/// **Does this text carry a fold?** The question was written three times,
/// identically and from memory, in three different measurements — the very form of
/// duplication that once made the walk lie. It lives here.
///
/// Broad on purpose: every tokenade banner counts, whether it announces a hash or
/// not. [`carries_fold_with_hash`] asks the STRICT version — the one that requires
/// a way back — and `exec_errors` reports both, so we know whether
/// the answer depends on the definition.
pub fn carries_fold(text: &str) -> bool {
    !is_resume_mask(text)
        && (text.contains("tokens folded away")
            || text.contains("[tokenade:")
            || text.contains("[disclose:summary"))
}

/// **A resume mask is not a delivered fold.** `session_compact` rewrites
/// a transcript's old results AT the time of `/resume` — including the blocks
/// that a command rerun later had made obsolete. Read as folds, they
/// made up half the denominator (2,842 `elided` "folds" out of 5,284,
/// 2026-10-05) and every rerun that JUSTIFIED the mask counted as a
/// rerun caused by a fold. The agent did not receive them at that moment.
pub fn is_resume_mask(text: &str) -> bool {
    let t = text.trim();
    t.starts_with("[tokenade:elided")
        && t.contains("on cold resume")
        && t.ends_with(']')
        && !t[1..].contains("\n[")
}

/// The fold that offers a way back: a banner carrying a hash.
pub fn carries_fold_with_hash(text: &str) -> bool {
    let mut out = Vec::new();
    folds_in(text, &mut out);
    out.iter().any(|(_, fam)| fam != "(no banner)")
}

/// The text of an event, whatever the wrapping — a `tool_result` carries
/// its content sometimes as a string, sometimes as a list of blocks.
fn text_of(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(a) => a.iter().map(text_of).collect::<Vec<_>>().join("\n"),
        serde_json::Value::Object(o) => o
            .get("text")
            .or_else(|| o.get("content"))
            .map(text_of)
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// **A single walk, and a single block decoder.**
///
/// Three measurements read the same 1.3 GB — the folds, the replay control, the outcome
/// of `--grep`s — and each one decoded an event's envelope its own way:
/// `message.content` sometimes an object sometimes a list, a block whose text lives under
/// `text` or under `content`. Three copies of one mechanism, so three
/// places where the same defect can come back. It lives here.
///
/// `f` receives, for each block: the project name, whether it is a Bash command
/// (with its command line) or RECEIVED output (with its text), and an
/// end-of-file flag for the measurements that aggregate per transcript.
/// What a received block IS. The type alone is not enough: system reminders
/// arrive as `text` blocks in `user` messages, so neither agent
/// prose nor a tool result. Confusing them with either skews
/// any measurement that looks at "what follows a result".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Recv {
    /// The result of a tool call.
    Result,
    /// What the agent wrote itself.
    AssistantProse,
    /// Everything else — system reminder, user message, meta.
    Other,
}

pub enum Block<'a> {
    /// A command, with the identifier under which its result will come back.
    Bash { id: Option<String>, cmd: &'a str },
    /// Received output. `id` is that of the `tool_use` it answers when
    /// it is one. `is_assistant_prose` distinguishes what the agent RECEIVED from what
    /// it WROTE — and it requires the ROLE, not just the block type: the
    /// system reminders also arrive as `text` blocks, in `user`
    /// messages, and counting them as agent prose drowned the measurement under
    /// 24,765 "occasions" that reacted to nothing.
    Received {
        id: Option<String>,
        text: String,
        kind: Recv,
        /// The flag the transcript writes itself (`is_error`): did the tool
        /// FAIL? It cannot be guessed from the text — many outputs
        /// contain the word "error" without being a failure.
        is_error: bool,
    },
}

/// An event older than the `TOKENADE_BENCH_SINCE` bound? ISO 8601
/// timestamps compare as strings; an event without a timestamp is
/// kept (it cannot be dated, nor excluded).
pub fn before_since(ts: &str, since: Option<&str>) -> bool {
    since.is_some_and(|s| !ts.is_empty() && ts < s)
}

pub fn for_each_block(
    root: &Path,
    mut f: impl FnMut(&str, &Path, &str, Block<'_>),
    mut end: impl FnMut(),
) {
    let mut files = Vec::new();
    walk(root, &mut files);
    files.sort();
    let since = std::env::var("TOKENADE_BENCH_SINCE")
        .ok()
        .filter(|s| !s.is_empty());
    for path in &files {
        let project = path
            .parent()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let Some(body) = read_transcript(path) else {
            continue;
        };
        if is_our_own_echo(&body) {
            continue;
        }
        for line in body.lines() {
            let Ok(ev) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            // The event's timestamp, as the transcript writes it. It
            // answers the one question a failure count does not:
            // was a hash not found AFTER the store sweep (which
            // is the rule) or BEFORE (which is a fold that promises
            // bytes it did not write).
            let role = ev
                .get("message")
                .and_then(|m| m.get("role"))
                .and_then(|r| r.as_str())
                .unwrap_or("")
                .to_string();
            let ts = ev
                .get("timestamp")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            // `TOKENADE_BENCH_SINCE=2026-10-03`: only what the current version
            // saw. Without a bound, a "worst folds" ranking listed three cases
            // fixed since — the measurement mixed five versions.
            if before_since(&ts, since.as_deref()) {
                continue;
            }
            let content = ev
                .get("message")
                .and_then(|m| m.get("content"))
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let blocks = match &content {
                serde_json::Value::Array(a) => a.clone(),
                other => vec![other.clone()],
            };
            for b in &blocks {
                // **`tool_use` is not `Bash`.** Generalising the walk by
                // dropping this test made every `Read`, `Edit` and
                // `Grep` pass for a command: the replay rate rose from
                // 0.25 % to 60.73 % — an absurd figure, so spotted, but the
                // canary said nothing because its fixture used only Bash.
                // It sets one now.
                let is_bash = b.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                    && b.get("name").and_then(|t| t.as_str()) == Some("Bash");
                let id = |k: &str| b.get(k).and_then(|i| i.as_str()).map(|i| i.to_string());
                if is_bash {
                    let cmd = b
                        .get("input")
                        .and_then(|i| i.get("command"))
                        .and_then(|c| c.as_str())
                        .unwrap_or("");
                    f(&project, path, &ts, Block::Bash { id: id("id"), cmd });
                } else if b.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                    // Another tool: neither a command nor received output.
                    continue;
                } else {
                    f(
                        &project,
                        path,
                        &ts,
                        Block::Received {
                            id: id("tool_use_id"),
                            text: text_of(b),
                            is_error: b.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false),
                            kind: match b.get("type").and_then(|t| t.as_str()) {
                                Some("tool_result") => Recv::Result,
                                Some("text") if role == "assistant" => Recv::AssistantProse,
                                _ => Recv::Other,
                            },
                        },
                    );
                }
            }
        }
        end();
    }
}

/// Does the local store hold the bytes of this hash? It is the trace that a fold
/// did take place, even when its banner is in no transcript read.
fn stash_holds(hash: &str) -> bool {
    let dir = tokenade_core::disclose::stash_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return false;
    };
    rd.flatten()
        .any(|e| e.file_name().to_string_lossy().starts_with(hash))
}

/// A call that REPLAYS the command without compaction. `tokenade raw` and its two
/// aliases — the name comes from the product's help, not from memory.
pub fn is_raw_call(cmd: &str) -> bool {
    ["tokenade raw ", "tokenade bypass ", "tokenade noproxy "]
        .iter()
        .any(|p| cmd.contains(p))
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "jsonl") {
            if is_being_written(&p) {
                continue;
            }
            out.push(p);
        }
    }
}

/// **An instrument does not read a transcript that is being WRITTEN.**
///
/// The current session contains the OUTPUT of the previous measurement: the hashes
/// the instrument just printed read back as recoveries.
///
/// The first spelling of this rule read `TOKENADE_SESSION` — and that
/// variable **never reaches the process**: the launcher scrubs it before
/// running the command (`python3 -c "print(os.environ.get(...))"` returns
/// `None`). The rule was therefore silent, and what sometimes excluded the live
/// session was an ACCIDENT: a 150 MB file being written often ends
/// in the middle of a multi-byte character, `read_to_string` returned
/// `Err(InvalidData)`, and the whole file silently vanished from the corpus
/// (1,028 recoveries served with it, 831 without — 18 % of the corpus coming and going
/// depending on where the writer was).
///
/// Both defects are fixed together: reading no longer loses a file
/// for one byte (see `read_transcript`), and the exclusion rests on an
/// observable fact — the modification date. A transcript touched just now is
/// being written; it is not a measurable object.
fn is_being_written(p: &Path) -> bool {
    let Ok(md) = std::fs::metadata(p) else {
        return false;
    };
    let Ok(modified) = md.modified() else {
        return false;
    };
    modified
        .elapsed()
        .map(|d| d < std::time::Duration::from_secs(120))
        .unwrap_or(true)
}

/// **The traces of our own readings are not observations.**
///
/// A session where this instrument ran contains its output: the hashes it
/// prints read back as recoveries, its fixtures as folds,
/// its commands as calls. The modification date only catches the
/// LIVE session — and even then, only if measuring while it writes:
/// between two builds, the same reading saw the corpus go from 3,143 to
/// 3,567 folded commands depending on whether the file was fresh or not.
///
/// The criterion that does not depend on the moment: does the transcript quote the
/// instrument's output? Those sentences exist nowhere else. The French entries
/// match the instruments' earlier output, still present in older transcripts.
const OUR_OWN_ECHO: &[&str] = &[
    "INSTRUMENT NON VALIDÉ",
    "canari rejeu :",
    "canari exécution :",
    "le pli l'avait RETIRÉ",
    "TÉMOIN, après un résultat intact",
    "commandes dont le résultat portait un pli",
    "INSTRUMENT NOT VALIDATED",
    "replay canary:",
    "execution canary:",
    "the fold had REMOVED it",
    "CONTROL, after an intact result",
    "commands whose result carried a fold",
];

fn is_our_own_echo(body: &str) -> bool {
    OUR_OWN_ECHO.iter().any(|m| body.contains(m))
}

/// Backdates a canary transcript: the exclusion rule bears on
/// files WRITTEN JUST NOW, and a fixture by definition just
/// was. Backdating it brings it into the corpus exactly like a
/// real transcript — and lets the canary check the rule rather than
/// work around it (a file left FRESH must, for its part, stay out).
pub(crate) fn backdate(p: &Path) {
    if let Ok(f) = std::fs::File::options().write(true).open(p) {
        let _ = f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600));
    }
}

/// A transcript being written can end in the middle of a character: we
/// decode losing that byte, never the file.
fn read_transcript(p: &Path) -> Option<String> {
    let bytes = std::fs::read(p).ok()?;
    Some(match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    })
}

/// The hashes our OWN sources hard-code — `deadbeefdead` and its
/// neighbours, set by a test or an example. They appear in the
/// transcripts because a suite prints them, and an `expand-ref` run
/// on them fails by construction: the store never had those bytes.
/// Counting them as missed folds would blame the product for a defect that
/// the fixture stages (a corpus made of my own sessions mostly
/// measures me).
fn fixture_hashes() -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.join("crates"));
    let Some(root) = root else { return out };
    let re = regex::Regex::new(r"\b[0-9a-f]{12}\b").expect("hash pattern");
    let mut files = Vec::new();
    walk_rs(&root, &mut files);
    for f in files {
        let Ok(body) = std::fs::read_to_string(&f) else {
            continue;
        };
        for m in re.find_iter(&body) {
            out.insert(m.as_str().to_string());
        }
    }
    out
}

fn walk_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk_rs(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}
