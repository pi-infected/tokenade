//! **Who calls what, and how far it reaches.**
//!
//! The call graph and the blast radius. Distinct from search: there we ask
//! WHERE a name IS, here we ask what BREAKS if it changes — and the second
//! question requires distrusting homonyms, hence the vouched-for qualifiers
//! and the module scope.

use super::*;

#[derive(Debug, serde::Serialize)]
pub struct ImpactReport {
    pub file: String,
    pub dependents: Vec<String>,
    pub symbols: Vec<String>,
    /// What the file declares FOR ITS OWN TESTS, kept apart from the rest.
    ///
    /// `impact` answers "what depends on this file", and a `#[cfg(test)]`
    /// declaration is not part of that surface. Counting them together
    /// overstated what a file offers by about half: 9 592 of this
    /// workspace's 17 763 indexed functions are test functions, and on
    /// `systemd.rs` the report read `fn (20)` for seven real ones. Worse,
    /// the twelve-name window each kind gets was spent on them — five of
    /// the twelve shown there were tests, and source order (the earlier
    /// mitigation) only helps while a file's API is longer than the window.
    ///
    /// Kept, not dropped: an agent looking for a test by name still finds it
    /// through `query`, and the count is still printed here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub test_symbols: Vec<String>,
    /// Files that call a name this file declares, but whose attribution was
    /// abandoned because the name has more than one home in the index.
    ///
    /// These are dependents the report CANNOT confirm and therefore does not
    /// list. Measured on `redact.rs`: 19 listed, at least 28 real — the name
    /// `redact` is declared as a module in `lib.rs` and as a `fn` in both
    /// `redact.rs` and `web_pipeline.rs`, so every `crate::redact::redact(…)`
    /// call is dropped. Reporting the count is not the fix (that needs the
    /// call qualifier); it is the difference between a
    /// number that is short and a number that looks complete.
    pub ambiguous_callers: usize,
}

impl Index {
    /// Impact radius: files that import `file` (best-effort, by path stem),
    /// plus the symbols declared in `file`. This is what the agent reads
    /// instead of grepping the whole tree before a refactor.
    /// Functions that CALL `name` — i.e., the inbound edges. Returns
    /// up to `limit` distinct callers ordered by (file, line). Empty
    /// when no call data was indexed (regex-only languages, or pre-
    /// v4 index files).
    pub fn callers(&self, name: &str, limit: usize) -> Vec<&Call> {
        self.callers_with_total(name, limit).0
    }

    /// As [`Self::callers`], plus how many distinct callers matched before
    /// the cap — a capped list that says nothing reads as the complete set,
    /// which is the rule `query`'s own hit list already follows.
    ///
    /// "Distinct" and "ordered" are what the doc above has always claimed and
    /// what the body did not do: it filtered and `take`-d in walk order, so a
    /// helper called eleven times from one function returned eleven identical
    /// rows and filled the cap with them. Same defect as the repeated
    /// `const (5): MAX_RESCUED, MAX_RESCUED` in the skeleton — and it
    /// survived because nothing in production ever called this.
    /// The callers of ONE declaration, rather than of a name.
    ///
    /// [`Self::callers_with_total`] answers "who calls this NAME", and says so
    /// when several declarations share it. When exactly ONE does, that caveat
    /// is not printed and the callers read as fact — which is where the same
    /// fabricated edge [`Self::call_reaches`] describes was still being served
    /// after `impact_of` stopped serving it. MEASURED 2026-09-06 on a Python
    /// tree holding one `execute`: `impact` answered "dependents (0)" while
    /// `query execute` answered "called by: load_rows (app/db.py:7)" — the same
    /// index, the same question, two answers, one of them invented from a
    /// `sqlite3.Cursor`.
    ///
    /// Same predicate, one spelling, so the two commands cannot disagree again.
    pub fn callers_of_declaration(
        &self,
        name: &str,
        decl_file: &str,
        limit: usize,
    ) -> (Vec<&Call>, usize) {
        let vouched = self.vouched_qualifiers();
        let mut seen: std::collections::HashSet<(&str, &str)> = std::collections::HashSet::new();
        let mut hits: Vec<&Call> = self
            .calls
            .iter()
            .filter(|c| c.callee_name == name)
            .filter(|c| self.call_reaches(c, decl_file, &vouched))
            .filter(|c| seen.insert((c.caller_name.as_str(), c.file.as_str())))
            .collect();
        hits.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
        let total = hits.len();
        hits.truncate(limit);
        (hits, total)
    }

    pub fn callers_with_total(&self, name: &str, limit: usize) -> (Vec<&Call>, usize) {
        let mut seen: std::collections::HashSet<(&str, &str)> = std::collections::HashSet::new();
        let mut hits: Vec<&Call> = self
            .calls
            .iter()
            .filter(|c| c.callee_name == name)
            .filter(|c| seen.insert((c.caller_name.as_str(), c.file.as_str())))
            .collect();
        hits.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
        let total = hits.len();
        hits.truncate(limit);
        (hits, total)
    }

    /// Does this import text name the module `stem` — as a whole segment,
    /// not as a fragment of a longer name?
    ///
    /// A plain `contains` reported `user.rs` as a dependent of `logs.rs`
    /// when all it imported was `console_logs`, and this repository alone
    /// carries three different modules whose name ends in `logs`. Anyone
    /// answering the question by hand makes the same mistake — the fix is
    /// to require identifier boundaries on both sides.
    fn mentions_module(dep: &str, stem: &str) -> bool {
        let is_ident = |c: char| c.is_alphanumeric() || c == '_';
        dep.match_indices(stem).any(|(i, _)| {
            dep[..i].chars().next_back().is_none_or(|c| !is_ident(c))
                && dep[i + stem.len()..]
                    .chars()
                    .next()
                    .is_none_or(|c| !is_ident(c))
        })
    }

    /// Every qualifier the index can vouch for: a name it declares, or the
    /// stem of a file it holds. `crate::logs::fold(…)` writes `logs`, which is
    /// a file stem; `Foo::bar()` writes `Foo`, which is a declared type. A
    /// qualifier in NEITHER set is a local variable holding an object the index
    /// never typed — `cursor` in `cursor.execute(…)`, `all_entries` in
    /// `all_entries.push(…)`.
    fn vouched_qualifiers(&self) -> std::collections::HashSet<&str> {
        let mut set: std::collections::HashSet<&str> =
            self.symbols.iter().map(|s| s.name.as_str()).collect();
        for f in self.mtimes.keys() {
            if let Some(stem) = Path::new(f).file_stem().and_then(|s| s.to_str()) {
                set.insert(stem);
            }
        }
        set
    }

    /// Can this call site plausibly reach a declaration in `decl_file`?
    ///
    /// Asked ONLY of a callee whose name has exactly one home. The ambiguous
    /// case is already refused above; this is the case that was bound with no
    /// evidence at all, because being the only candidate was taken as proof.
    ///
    /// It is not. When a call is written on a receiver the index never typed —
    /// a `sqlite3.Cursor`, a `logging.Logger`, a `Vec` — the bare method name
    /// is matched against every indexed symbol, and a single hit was bound
    /// wherever it lived. MEASURED on this repository before the guard: 1 942
    /// such edges, and the names they attach to say what they are — `push`
    /// 557, `join` 362, `now` 196, `split` 71, `replace` 61, `sleep` 51. Every
    /// one is a std method: `all_entries.push()` was booked as a call into
    /// `model_costs.rs`, `ex.split()` into `line_gutter.rs`, `Instant.now()`
    /// into `agent/pool.rs`. `impact` on `model_costs.rs` answered **382
    /// dependents** on the strength of them.
    ///
    /// Evidence means one of: the declaration sits in the caller's own
    /// directory, which is one package in every language this path serves (and
    /// covers the caller's own file); or the caller imports the file it lives
    /// in, asked through the same `mentions_module` test `impact_of` uses.
    /// A call with NO qualifier is left alone — that is an unqualified call,
    /// usually a genuine import the best-effort resolver did not link, and
    /// 13 500 of this repository's evidence-free edges are of that kind. A
    /// qualifier the index vouches for is left alone too, since it names the
    /// home the caller meant.
    ///
    /// The trade is the one the ambiguity refusal above already settled: a
    /// missing edge degrades an answer, a fabricated one corrupts it.
    fn call_reaches(
        &self,
        call: &Call,
        decl_file: &str,
        vouched: &std::collections::HashSet<&str>,
    ) -> bool {
        if call.qualifier.is_empty() || vouched.contains(call.qualifier.as_str()) {
            return true;
        }
        // The declaration's own file, and the caller's imports.
        //
        // Both were deleted for one iteration, because from [`Index::impact_of`]
        // they are unreachable: it skips a call whose file IS the file under
        // report, and it fills `dependents` from the import graph BEFORE the
        // call loop, which then skips any file already there. Two mutations
        // survived saying exactly that, and the branches went.
        //
        // [`Index::callers_of_declaration`] then arrived and needed both. It
        // has no import loop in front of it, so without them `query` dropped
        // the genuine caller that IMPORTS the declaration and calls it on a
        // local — the very case `impact` was still reporting correctly, so the
        // two commands went on disagreeing, in the other direction. The lesson
        // is not that the clauses were wrong; it is that "unreachable" was a
        // fact about ONE consumer and was recorded as a fact about the rule.
        // No same-file clause: it is SUBSUMED by the directory test below —
        // a call in the declaring file is trivially in the declaring file's
        // directory — so it could never be the deciding one, for any consumer.
        // That is a fact about the rule, unlike the import clause beside it,
        // which was deleted once for being unreachable from a single consumer
        // and had to come back the moment a second one arrived.
        let dir = |p: &str| p.rsplit_once('/').map(|(d, _)| d.to_string());
        if dir(decl_file) == dir(&call.file) {
            return true;
        }
        // The SAME import test `impact_of` uses, not an exact path match.
        // Import resolution is best-effort: a Python `from fixtures.doubles
        // import StubCursor` is stored as written, and comparing it to the
        // path `fixtures/doubles.py` never matches. Exact equality dropped the
        // genuine caller while `impact` — which asks through `mentions_module`
        // — kept it, so the two commands disagreed a third time, on the third
        // spelling of one question.
        let Some(stem) = Path::new(decl_file).file_stem().and_then(|s| s.to_str()) else {
            return false;
        };
        self.imports
            .get(call.file.as_str())
            .is_some_and(|deps| deps.iter().any(|d| Self::mentions_module(d, stem)))
    }

    pub fn impact_of(&self, file: &str) -> ImpactReport {
        // #937 — defense-in-depth: an empty `file` made `Path::new("").file_stem()`
        // return None, falling back to the empty string, then
        // `deps.iter().any(|d| d.contains(""))` was always true → every
        // indexed file was reported as a dependent (~4200 paths). CLI +
        // MCP layers now reject this upfront, but guard here too so any
        // future caller can't trip the same wire.
        if file.trim().is_empty() {
            return ImpactReport {
                ambiguous_callers: 0,
                file: file.to_string(),
                dependents: Vec::new(),
                symbols: Vec::new(),
                test_symbols: Vec::new(),
            };
        }
        let stem = Path::new(file)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(file)
            .to_string();
        // Same defense: a path like "." has empty file_stem; falling
        // through would still match every dep.
        if stem.is_empty() {
            return ImpactReport {
                ambiguous_callers: 0,
                file: file.to_string(),
                dependents: Vec::new(),
                symbols: Vec::new(),
                test_symbols: Vec::new(),
            };
        }
        let mut dependents = Vec::new();
        // Callers whose attribution had to be abandoned — reported, not hidden.
        let mut ambiguous: std::collections::BTreeSet<String> = Default::default();
        for (f, deps) in &self.imports {
            if f == file {
                continue;
            }
            if deps.iter().any(|d| Self::mentions_module(d, &stem)) {
                dependents.push(f.clone());
            }
        }
        // Imports alone under-report badly. A language where a module can be
        // reached by a qualified path — `crate::logs::fold(…)`, `logs.Fold(…)`,
        // `Foo::bar()` — has callers that never write an import statement, and
        // this codebase writes them that way on purpose. Measured on
        // `logs.rs`: 3 dependents found, 8 real. Someone scoping a change off
        // that number misses five call sites.
        //
        // The call graph already knows. A call is attributed only when its
        // name is declared in EXACTLY ONE indexed file: `new` or `compact` is
        // declared everywhere and attributing it would trade a recall gap for
        // a precision one, which is the worse of the two here.
        // An `impl` block extends a type, it does not declare it. Counting it
        // as a declaration makes a name look like it has two homes, and the
        // attribution below deliberately gives up on any name declared in more
        // than one file — so leaving impls in here would silently drop call
        // attribution for every type that is implemented outside its own file.
        // NOT excluded here, though it looks like it should be: `kind ==
        // "module"`. A `mod redact;` line names a module, not a callable, so
        // counting it as a competing declaration is what makes `redact` look
        // like it has two homes and drops every `crate::redact::redact(…)`
        // call from attribution. Removing it MEASURED WORSE (2026-08-04):
        // `net.rs` went from 38 dependents to 105, because names like `fetch`,
        // `json`, `status` and `serve` live there too and their `mod` entry was
        // the only thing suppressing them. The module declaration is doing real
        // work as an ambiguity marker for exactly the generic names that need
        // one. This is the trade the comment above names — a recall gap is the
        // better of the two — so it stays. A qualifier-based fix would
        // resolve it properly.
        let ext = Path::new(file).extension().and_then(|s| s.to_str());
        // Does `<stem>::` name THIS file and no other? Counted over the files
        // the index actually holds, same extension only — a `logs.py` never
        // answers a Rust `logs::`.
        let stem_is_unique = {
            let mut seen = 0usize;
            for f in self.mtimes.keys() {
                if Path::new(f).extension().and_then(|s| s.to_str()) != ext {
                    continue;
                }
                if Path::new(f).file_stem().and_then(|s| s.to_str()) == Some(stem.as_str()) {
                    seen += 1;
                    if seen > 1 {
                        break;
                    }
                }
            }
            seen == 1
        };
        // Test declarations are not part of what this file offers, so they
        // must not decide who depends on it either. Two ways they did:
        //
        //  * a test helper named `out`, `sym` or `scratch` put that name into
        //    `declared`, and every file calling anything so named became a
        //    caller this report had to give up on;
        //  * a test declaration in ANOTHER file made a real name look like it
        //    had several homes, and attribution was abandoned for all of them.
        //
        // Measured on this repository: `disclose.rs` went from 13 unattributed
        // callers to 4, `redact.rs` from 6 to 3, and `code_intel.rs` gained a
        // dependent it had been giving up on.
        //
        // Sound in Rust rather than merely convenient: a `#[cfg(test)]` item
        // is compiled only for its own crate's test build, and each file under
        // `tests/` is its own crate that nothing links back to. The exception
        // is a `pub` helper inside a `mod tests` reached from a sibling module
        // under `cfg(test)`; that call is dropped rather than mis-attributed,
        // which is the direction this whole branch already errs in.
        // Spans once for the whole index, not once per candidate: the loop
        // below asks about every symbol that shares a name with this file's,
        // and rebuilding a file's spans inside it is quadratic.
        let mut spans_by_file: std::collections::HashMap<&str, Vec<(usize, usize)>> =
            std::collections::HashMap::new();
        for s in &self.symbols {
            if s.kind == "module" && (s.name == "tests" || s.name == "test") && s.end_line > s.line
            {
                spans_by_file
                    .entry(s.file.as_str())
                    .or_default()
                    .push((s.line, s.end_line));
            }
        }
        const NO_SPANS: &[(usize, usize)] = &[];
        let spans_of = |f: &str| -> &[(usize, usize)] {
            spans_by_file
                .get(f)
                .map(|v| v.as_slice())
                .unwrap_or(NO_SPANS)
        };
        let file_spans = spans_of(file).to_vec();
        let declared: std::collections::HashSet<&str> = self
            .symbols
            .iter()
            .filter(|s| s.file == file && s.kind != "impl" && !symbol_is_test(s, &file_spans))
            .map(|s| s.name.as_str())
            .collect();
        if !declared.is_empty() {
            let mut homes: std::collections::HashMap<&str, Option<&str>> =
                std::collections::HashMap::new();
            for s in &self.symbols {
                if !declared.contains(s.name.as_str()) {
                    continue;
                }
                // Same-language only. The caller loop below already refuses to
                // let a `.py` file call a Rust function; a declaration in
                // another language must likewise not make a Rust name look
                // ambiguous. Measured: a `fn redact` in `npm/install.js` was
                // one of the four "homes" that killed attribution for
                // `tokenade-core/src/redact.rs`.
                if Path::new(&s.file).extension().and_then(|x| x.to_str()) != ext {
                    continue;
                }
                // A test declaration is not a home a call can land in — see
                // the note on `declared` above. No `s.file != file` clause:
                // `declared` already holds no test name of this file's, so the
                // only symbols reaching here from `file` itself are its API,
                // and a mutation that dropped the clause changed nothing.
                if symbol_is_test(s, spans_of(&s.file)) {
                    continue;
                }
                match homes.get(s.name.as_str()) {
                    Some(Some(f)) if *f != s.file.as_str() => {
                        homes.insert(s.name.as_str(), None);
                    }
                    Some(_) => {}
                    None => {
                        homes.insert(s.name.as_str(), Some(s.file.as_str()));
                    }
                }
            }
            // Spans once for the whole loop, like `spans_by_file` above.
            let vouched = self.vouched_qualifiers();
            // A `.py` file does not call a Rust function. Without this, a
            // name this file happens to share with a builtin (`run`, `main`,
            // `compact`) dragged in every script in the repository — measured:
            // 9 of 14 reported dependents were Python, for a Rust module.
            for c in &self.calls {
                if c.file == file || dependents.iter().any(|d| d == &c.file) {
                    continue;
                }
                if Path::new(&c.file).extension().and_then(|s| s.to_str()) != ext {
                    continue;
                }
                match homes.get(c.callee_name.as_str()) {
                    // One home is not on its own a reason to bind — see
                    // [`Index::call_reaches`].
                    Some(Some(home)) => {
                        if self.call_reaches(c, home, &vouched) {
                            dependents.push(c.file.clone());
                        }
                    }
                    // The name is declared here, but in more than one place.
                    // The bare name cannot say which home this caller meant —
                    // but the caller may have SAID: `crate::logs::fold(…)`
                    // names the module, and that is precisely how a generic
                    // name gets written when it is generic. Measured on this
                    // repository: 2 036 call sites carry a qualifier naming a
                    // real module while their callee has several homes.
                    //
                    // Only when the stem is unambiguous itself. Two `logs.rs`
                    // in one repository would make `logs::` name neither, and
                    // guessing between them is the precision loss this whole
                    // branch exists to avoid.
                    Some(None) if !c.qualifier.is_empty() && c.qualifier == stem => {
                        if stem_is_unique {
                            dependents.push(c.file.clone());
                        } else {
                            ambiguous.insert(c.file.clone());
                        }
                    }
                    // Silence here reads as "no such caller exists", so a
                    // caller we had to give up on is counted, not hidden.
                    Some(None) => {
                        ambiguous.insert(c.file.clone());
                    }
                    None => {}
                }
            }
        }
        dependents.sort();
        dependents.dedup();
        // Source order, not the index's global by-name order. A report about
        // ONE file is the only place declaration order is even defined, and
        // its consumers show a bounded window of the names — so the order
        // decides which names an agent ever sees. By name, that window is
        // whatever sorts first; by line, it is the head of the file, where a
        // module's exported surface sits and an inline test module never does.
        // (`line` ties break on name so the output stays byte-stable.)
        let mut own: Vec<&Symbol> = self.symbols.iter().filter(|s| s.file == file).collect();
        own.sort_by(|a, b| a.line.cmp(&b.line).then_with(|| a.name.cmp(&b.name)));
        let test_spans = test_module_spans(own.iter().copied());
        let is_test = |s: &Symbol| -> bool { symbol_is_test(s, &test_spans) };
        // `label()`, not `kind` + `name`: `impact` splits this back at the
        // first space and groups by the head, so the tail is whatever the
        // one declarer says. Without it the report printed
        // `impl (6): TokenadeError ×6` and `fn (12): …, from ×4` — the
        // `×N` collapse hiding a distinction the index now holds.
        let symbols: Vec<String> = own
            .iter()
            .filter(|s| !is_test(s))
            .map(|s| s.label())
            .collect();
        let test_symbols: Vec<String> = own
            .iter()
            .filter(|s| is_test(s))
            .map(|s| s.label())
            .collect();
        // A caller that made it into `dependents` by another route (an import,
        // or a second call whose name is unambiguous) is not missing from the
        // report, so it must not be counted as lost.
        let ambiguous_callers = ambiguous.iter().filter(|f| !dependents.contains(f)).count();
        ImpactReport {
            file: file.to_string(),
            dependents,
            symbols,
            test_symbols,
            ambiguous_callers,
        }
    }
}
