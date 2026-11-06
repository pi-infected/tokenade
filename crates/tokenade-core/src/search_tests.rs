// The tests of `search`.
//
// Spliced by `include!` from `search.rs`: the same module, so the same private
// access and the same `super::` paths, without a single visibility line
// changed. This is the shape this repository already uses for 33 other
// modules.
//
// A `//!` is impossible here: a file spliced by `include!` is not a module,
// and an INNER doc comment has nothing to qualify.

#[cfg(test)]
mod unrecognised_saved_page_tests {
    use super::*;

    /// The case that got the note written: a documentation page served as a
    /// list of results, without a word.
    #[test]
    fn a_page_no_engine_produced_says_so() {
        let n = unrecognised_saved_page_note("docsrs.html", None, 24).expect("nothing said");
        assert!(n.contains("docsrs.html"), "{n}");
        assert!(n.contains("24 links"), "{n}");
        assert!(n.contains("not results"), "{n}");
        // The singular counts too: it is the form the agent reads most often
        // on a page that is not a results page.
        let one = unrecognised_saved_page_note("x.html", None, 1).expect("nothing said");
        assert!(one.contains("1 link "), "{one}");
    }

    /// And nothing in the two cases where there is nothing to say: a
    /// recognised engine already announces itself, and an empty list is named
    /// by `render`.
    #[test]
    fn a_recognised_engine_and_an_empty_list_say_nothing_extra() {
        assert!(unrecognised_saved_page_note("m.html", Some(Engine::Mojeek), 12).is_none());
        assert!(unrecognised_saved_page_note("m.html", None, 0).is_none());
    }
}

#[cfg(test)]
mod empty_outcome_tests {
    use super::*;

    /// An empty answer names the wall it ACTUALLY hit.
    ///
    /// `tokenade search <file|->` compacts a saved results page and queries
    /// nobody. Measured 2026-09-04 by piping an ordinary HTML page into it:
    /// `no usable results for "saved page" — no engine was reachable`, on a
    /// path where no engine was contacted at all. The same branch answered
    /// for a live engine that replied with zero hits, which leaves `used`
    /// full — a reachable engine reported as unreachable.
    #[test]
    fn an_empty_outcome_names_the_wall_it_actually_hit() {
        let saved = Outcome {
            results: Vec::new(),
            used: Vec::new(),
            refused: Vec::new(),
        };
        let s = render("saved page", &saved);
        assert!(
            s.contains("no engine was queried"),
            "nobody was asked, and the answer must say so: {s}"
        );
        assert!(
            !s.contains("reachable"),
            "a page we were handed blamed the network: {s}"
        );

        let answered = Outcome {
            results: Vec::new(),
            used: vec!["duckduckgo"],
            refused: Vec::new(),
        };
        let s = render("rust tokio cancellation", &answered);
        assert!(
            s.contains("duckduckgo"),
            "the engine that answered is not named: {s}"
        );
        assert!(
            !s.contains("reachable"),
            "it answered; it was reachable: {s}"
        );

        // The refusal path is what the reason string was written for, and it
        // still wins over both of the above.
        let refused = Outcome {
            results: Vec::new(),
            used: vec!["duckduckgo"],
            refused: vec![("all-engines", Refusal::RateLimited)],
        };
        let s = render("rust tokio cancellation", &refused);
        assert!(
            s.contains("all-engines") && s.contains(Refusal::RateLimited.as_str()),
            "the refusal must survive: {s}"
        );
        assert!(s.contains("use your native web search"), "{s}");

        // The FIRST search to hit the wall — every engine tried, every engine
        // refused — must send the agent to its native search as well, not
        // only the later ones that find every breaker already open.
        let walled = Outcome {
            results: Vec::new(),
            used: vec!["bing", "duckduckgo"],
            refused: vec![("bing", Refusal::Challenge), ("duckduckgo", Refusal::Unreachable)],
        };
        let s = render("rust tokio cancellation", &walled);
        assert!(s.contains("bing: bot-challenge") && s.contains("use your native web search"), "{s}");

        // Engines that answered with nothing refused nothing: no detour.
        let empty = Outcome { results: Vec::new(), used: vec!["bing"], refused: Vec::new() };
        assert!(!render("q", &empty).contains("native"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A held engine lock outlives the selector's spin, so these tests must hold
    /// theirs for longer than it: `select_and_lock` spins 20 times at 50-100 ms,
    /// about 1.5 s, before giving up. A real fetch holds for seconds. Anything
    /// shorter here and every wave "passes" because the locks free themselves
    /// mid-spin — which is exactly why the first probe of this defect came back
    /// clean and was wrong.
    const LONGER_THAN_THE_SPIN_MS: u64 = 2000;

    /// Run `n` queries through the real selector `wave` at a time, holding each
    /// query's engines for a realistic fetch. Returns how many engines each
    /// query got, in order.
    fn engines_per_query(n: usize, wave: usize, hold_ms: u64) -> Vec<usize> {
        let mut counts = Vec::new();
        let slots: Vec<usize> = (0..n).collect();
        for chunk in slots.chunks(wave) {
            let got: Vec<usize> = std::thread::scope(|sc| {
                let hs: Vec<_> = chunk
                    .iter()
                    .map(|_| {
                        sc.spawn(move || {
                            let mut h = Health::default();
                            let locks = select_and_lock(&mut h, 1_000_000);
                            let k = locks.len();
                            std::thread::sleep(std::time::Duration::from_millis(hold_ms));
                            drop(locks);
                            k
                        })
                    })
                    .collect();
                hs.into_iter().map(|h| h.join().unwrap()).collect()
            });
            counts.extend(got);
        }
        counts
    }

    /// Both halves, and the second is the reason the first exists: a batch that
    /// fires every query at once starves its own tail (those queries are told
    /// `all-engines: rate-limited`, which blames a wall nobody hit), and the
    /// same queries in waves of `concurrency_for` all get an engine.
    #[test]
    fn a_batch_wider_than_the_engine_pool_starves_its_own_tail() {
        let mut env = crate::platform::ScopedEnv::new();
        let dir = std::env::temp_dir().join(format!(
            "tokenade-batch-pool-{}-{}",
            std::process::id(),
            now_millis()
        ));
        std::fs::create_dir_all(&dir).expect("test state dir");
        env.set("TOKENADE_WEBSEARCH_STATE_DIR", &dir);

        let n = 4;
        let all_at_once = engines_per_query(n, n, LONGER_THAN_THE_SPIN_MS);
        // SHORT-CHANGED, not necessarily empty-handed — and the difference is
        // a scheduling detail this test must not depend on.
        //
        // Four queries want FANOUT engines each over a pool of seven, so the
        // pool cannot serve the wave at full strength however the threads
        // interleave. WHO ends up with nothing does depend on the interleaving:
        // spawned in a loop the last one arrives after the pool is gone and
        // gets 0 (`[3, 3, 1, 0]`, what Linux produces every time), while a
        // tighter race spreads them and everyone keeps one (`[3, 1, 2, 1]`).
        // The first spelling of this assertion pinned the zero and failed on
        // the macOS aarch64 release leg with `[3, 3, 2, 1]` — a machine whose
        // scheduling is not Linux's, measuring the scheduler rather than the
        // product. The cap exists because a query gets LESS than it asked for;
        // zero is the extreme, not the property.
        assert!(
            all_at_once.iter().any(|&c| c < FANOUT),
            "the defect this cap exists for: {n} queries at once over {} engines \
             cannot all get {FANOUT}, got {all_at_once:?}",
            ENGINES.len()
        );

        let wave = concurrency_for(ENGINES.len(), n);
        assert!(
            wave < n,
            "a cap that admits the whole batch caps nothing: {wave} of {n}"
        );
        let in_waves = engines_per_query(n, wave, LONGER_THAN_THE_SPIN_MS);
        assert!(
            in_waves.iter().all(|&c| c == FANOUT),
            "in waves of {wave} every query must get its full {FANOUT} engines, \
             not merely one — that is what the cap is sized for, got {in_waves:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_wave_never_shrinks_to_zero_and_never_exceeds_the_batch() {
        assert_eq!(
            concurrency_for(ENGINES.len(), 9),
            ENGINES.len() / FANOUT,
            "a full pool serves pool/FANOUT queries at once"
        );
        assert_eq!(
            concurrency_for(ENGINES.len(), 1),
            1,
            "a one-query batch is never widened"
        );
        for eligible in [0usize, 1, 2] {
            assert_eq!(
                concurrency_for(eligible, 5),
                1,
                "with {eligible} engine(s) usable a query must still be ATTEMPTED \
                 and get the real refusal, not be dropped by arithmetic"
            );
        }
        assert_eq!(concurrency_for(0, 0), 1, "an empty batch still divides");
    }

    /// The two readers of the eligibility rule must not drift apart: what
    /// `batch_concurrency` counts is what `select_and_lock` can take. No state
    /// directory and no environment here — only the map both readers read.
    #[test]
    fn the_cap_counts_the_same_engines_the_selector_takes() {
        let now = 1_000_000;
        assert_eq!(
            eligible_engines(&mut Health::default(), now).len(),
            ENGINES.len(),
            "an untouched health map leaves every engine usable"
        );
        assert_eq!(
            batch_concurrency_from(&mut Health::default(), now, 9),
            ENGINES.len() / FANOUT
        );

        // Open every breaker but one: one engine cannot serve FANOUT, so the
        // wave floors at 1 rather than at 2.
        let mut degraded = Health::default();
        for e in ENGINES.iter().skip(1) {
            degraded.entry(e.name().to_string()).or_default().open_until = now + 600;
        }
        assert_eq!(
            eligible_engines(&mut degraded.clone(), now).len(),
            1,
            "six breakers open leaves one engine"
        );
        assert_eq!(batch_concurrency_from(&mut degraded, now, 9), 1);
    }

    /// The minimum gap is a cadence: an engine held back by it alone is a
    /// wait, while an open breaker or a spent budget is a refusal.
    #[test]
    fn an_engine_held_back_only_by_the_gap_is_a_wait_not_a_refusal() {
        let (now, now_ms) = (1_000_000, 1_000_000_000);
        let gap = min_interval_ms();
        let mut health = Health::default();
        for e in ENGINES {
            let h = health.entry(e.name().to_string()).or_default();
            h.open_until = now + 600;
        }
        assert_eq!(
            spacing_wait_ms(&health, now, now_ms),
            None,
            "every breaker open"
        );

        let bing = health.get_mut(Engine::Bing.name()).unwrap();
        bing.open_until = 0;
        bing.window_start = now;
        bing.last_used_ms = now_ms - gap / 4;
        assert_eq!(spacing_wait_ms(&health, now, now_ms), Some(gap - gap / 4));
        health.get_mut(Engine::Bing.name()).unwrap().last_used_ms = now_ms - 2 * gap;
        assert_eq!(
            spacing_wait_ms(&health, now, now_ms),
            None,
            "already spaced: nothing to wait for"
        );

        health.get_mut(Engine::Bing.name()).unwrap().window_count = u32::MAX;
        assert_eq!(
            spacing_wait_ms(&health, now, now_ms),
            None,
            "budget spent this window"
        );
    }

    fn r(title: &str, url: &str) -> serp::Result {
        serp::Result {
            title: title.into(),
            url: url.into(),
            snippet: String::new(),
        }
    }

    /// Independent indexes converging on a URL is the strongest relevance
    /// signal available. The union used to be ordered by engine priority alone,
    /// so engine 3's best hit sorted below engine 1's tenth.
    #[test]
    fn cross_engine_agreement_outranks_engine_priority() {
        let a = vec![
            r("first engine top", "https://only-a.test/x"),
            r("agreed", "https://agreed.test/p"),
        ];
        let b = vec![r("agreed", "https://agreed.test/p")];
        let merged = merge_and_rank(&[a, b], "agreed");
        assert_eq!(
            merged[0].url, "https://agreed.test/p",
            "two engines agreeing must outrank one engine's #1"
        );
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn the_richest_description_of_a_url_survives_the_merge() {
        let mut thin = r("Short", "https://x.test/a");
        thin.snippet = "brief".into();
        let mut rich = r("A much longer and more useful title", "https://x.test/a");
        rich.snippet = "A far more informative snippet than the other one.".into();
        let merged = merge_and_rank(&[vec![thin], vec![rich]], "anything");
        assert_eq!(merged.len(), 1, "same url must dedup");
        assert!(merged[0].snippet.starts_with("A far more"));
        assert!(merged[0].title.starts_with("A much longer"));
    }

    #[test]
    fn one_site_cannot_fill_the_list() {
        let hits: Vec<_> = (0..8)
            .map(|i| r(&format!("post {i}"), &format!("https://spam.test/blog/{i}")))
            .collect();
        let merged = merge_and_rank(&[hits], "post");
        assert_eq!(merged.len(), MAX_PER_HOST, "one host is capped: {merged:?}");
    }

    #[test]
    fn www_is_not_a_different_site() {
        assert_eq!(
            canonical_url("https://www.x.test/a"),
            canonical_url("https://x.test/a")
        );
    }

    /// Two links to the same article counted as two results AND as cross-engine
    /// agreement — the merge was rewarding us for a duplicate.
    #[test]
    fn tracking_parameters_do_not_make_a_new_page() {
        let base = canonical_url("https://ex.test/article");
        for u in [
            "https://ex.test/article/",
            "https://ex.test/article#section",
            "https://ex.test/article?utm_source=x&utm_medium=y",
            "https://ex.test/article?srsltid=AfmBOo123",
            "https://ex.test/article?gclid=abc&fbclid=def",
            "https://www.ex.test/article",
            "https://ex.test/article/amp",
        ] {
            assert_eq!(canonical_url(u), base, "{u}");
        }
        // A REAL query parameter identifies a different page and must survive.
        assert_ne!(canonical_url("https://ex.test/article?page=2"), base);
        assert_eq!(
            canonical_url("https://ex.test/a?id=7&utm_source=x"),
            "https://ex.test/a?id=7"
        );
    }

    /// Agreement counts DISTINCT engines. One engine returning the same page
    /// twice (here as a plain URL and its utm-tagged twin, which canonicalise
    /// equal) must NOT read as two-engine agreement and outrank a page two
    /// separate engines actually agreed on.
    #[test]
    fn intra_engine_duplicates_do_not_inflate_agreement() {
        // Engine A lists one page twice (canonicalises to the same key).
        let a = vec![
            r("solo page", "https://solo.test/p"),
            r("solo page", "https://solo.test/p?utm_source=x"),
        ];
        // Engines B and C genuinely agree on a different page.
        let b = vec![r("agreed page", "https://agreed.test/q")];
        let c = vec![r("agreed page", "https://agreed.test/q")];
        let merged = merge_and_rank(&[a, b, c], "page");
        assert_eq!(
            merged[0].url, "https://agreed.test/q",
            "a real 2-engine agreement must outrank one engine's self-duplicate: {merged:?}"
        );
        // The self-duplicate collapses to a single row.
        assert_eq!(
            merged
                .iter()
                .filter(|r| r.url == "https://solo.test/p")
                .count(),
            1
        );
    }

    /// Agreement ties two results; coverage is what separates them.
    #[test]
    fn term_coverage_breaks_an_agreement_tie() {
        let full = r("rust async runtime comparison guide", "https://a.test/x");
        let half = r("rust programming", "https://b.test/y");
        // Both found by two engines, so agreement is equal.
        let merged = merge_and_rank(
            &[vec![half.clone(), full.clone()], vec![half, full]],
            "rust async runtime comparison",
        );
        assert_eq!(merged[0].url, "https://a.test/x", "{merged:?}");
    }

    /// One site with a good SEO team must not own the answer.
    #[test]
    fn no_host_may_dominate_the_list() {
        let mut hits: Vec<_> = (0..8)
            .map(|i| r(&format!("seo page {i}"), &format!("https://spam.test/p{i}")))
            .collect();
        hits.extend((0..4).map(|i| r(&format!("other {i}"), &format!("https://s{i}.test/a"))));
        let merged = merge_and_rank(&[hits], "page");
        let spam = merged
            .iter()
            .filter(|h| h.url.contains("spam.test"))
            .count();
        assert!(
            spam <= MAX_PER_HOST,
            "one host took {spam} slots: {merged:?}"
        );
        assert!(
            merged.len() > MAX_PER_HOST,
            "diversity must not shrink the list"
        );
    }

    #[test]
    fn an_open_breaker_takes_an_engine_out_of_rotation() {
        let now = 1_000_000;
        let mut h = Health::new();
        h.insert(
            "ecosia".into(),
            EngineHealth {
                fails: 2,
                open_until: now + 60,
                last_used: 0,
                ..Default::default()
            },
        );
        let order = rotation_order(&h, now);
        assert!(!order.contains(&Engine::Ecosia), "{order:?}");
        assert_eq!(order.len(), FANOUT);
        // Once it expires it comes back.
        assert!(rotation_order(&h, now + 61).contains(&Engine::Ecosia));
    }

    #[test]
    fn rotation_spreads_load_to_the_least_recently_used() {
        let now = 1_000_000;
        let mut h = Health::new();
        for (n, e) in ENGINES.iter().enumerate() {
            h.insert(
                e.name().into(),
                EngineHealth {
                    fails: 0,
                    open_until: 0,
                    last_used: now - n as u64,
                    ..Default::default()
                },
            );
        }
        // Highest `n` = oldest `last_used` = picked first. Asserted as a
        // PROPERTY, not against a named engine: adding an engine must not
        // break a test about load spreading.
        let order = rotation_order(&h, now);
        let oldest = *ENGINES.last().unwrap();
        assert_eq!(
            order[0], oldest,
            "least-recently-used goes first: {order:?}"
        );
        let used: Vec<u64> = order
            .iter()
            .map(|e| h.get(e.name()).unwrap().last_used)
            .collect();
        assert!(
            used.windows(2).all(|w| w[0] <= w[1]),
            "must be idle-first: {used:?}"
        );
    }

    /// "The web has nothing" and "we could not look" are different facts.
    #[test]
    fn a_refusal_is_reported_as_a_refusal_not_as_emptiness() {
        let o = Outcome {
            results: Vec::new(),
            used: Vec::new(),
            refused: vec![("bing", Refusal::Challenge), ("yahoo", Refusal::OffTopic)],
        };
        let out = render("some query", &o);
        assert!(out.contains("bing: bot-challenge"), "{out}");
        assert!(out.contains("yahoo: off-topic"), "{out}");
        assert!(!o.is_usable());
    }

    #[test]
    fn every_engine_is_reachable_from_the_rotation() {
        // A regression guard: `brave` and `marginalia` had extractors and were
        // wired to nothing but an example binary for months.
        for e in ENGINES {
            assert!(Engine::from_name(e.name()).is_some());
            assert!(e.url("test query").starts_with("https://"));
            assert!(
                e.url("a b").contains("a+b"),
                "query must be encoded: {}",
                e.url("a b")
            );
        }
        assert_eq!(ENGINES.len(), 7);
    }

    #[test]
    fn encodes_non_ascii_queries() {
        assert_eq!(urlencode_query("Klimagerät x"), "Klimager%C3%A4t+x");
    }

    /// D5: a locale prefix, an index leaf and a reordered query string all name
    /// the same page and must canonicalise identically — else the merge counts
    /// one page as several and rewards a duplicate.
    #[test]
    fn canonical_url_collapses_locale_index_and_param_order() {
        let base = canonical_url("https://ex.test/docs/guide");
        assert_eq!(canonical_url("https://ex.test/en/docs/guide"), base);
        assert_eq!(canonical_url("https://ex.test/en-US/docs/guide"), base);
        assert_eq!(canonical_url("https://ex.test/docs/guide/index.html"), base);
        // Param ORDER must not create a distinct page.
        assert_eq!(
            canonical_url("https://ex.test/s?b=2&a=1"),
            canonical_url("https://ex.test/s?a=1&b=2")
        );
        // A NON-locale two-letter segment must survive — not every short segment
        // is a language.
        assert_ne!(
            canonical_url("https://ex.test/io/guide"),
            canonical_url("https://ex.test/guide")
        );
        // A BARE locale homepage is a DISTINCT page per language: `/de` and `/fr`
        // are the German and French front pages, not the same page — they must
        // NOT collapse into each other or into the root.
        assert_ne!(
            canonical_url("https://ex.test/de"),
            canonical_url("https://ex.test/fr")
        );
        assert_ne!(
            canonical_url("https://ex.test/de/"),
            canonical_url("https://ex.test/")
        );
    }

    /// D10: a bot challenge backs off far longer than a transient blip — the
    /// flat TTL this replaced treated a CAPTCHA and a one-off timeout the same.
    #[test]
    fn breaker_duration_is_per_cause() {
        assert!(cb_duration(Refusal::Challenge) > cb_duration(Refusal::Unreachable));
        assert!(cb_duration(Refusal::OffTopic) >= cb_duration(Refusal::Thin));
        assert_eq!(cb_duration(Refusal::Challenge), 3600);
    }

    /// D11's hard constraint: two concurrent requests to the SAME engine must
    /// serialise (never overlap), while requests to DIFFERENT engines run in
    /// parallel. Tested directly on the permit, which is the mechanism.
    #[test]
    fn per_engine_permit_serialises_one_engine_not_the_fleet() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        // Same engine: the two critical sections must not overlap.
        let overlap = Arc::new(AtomicUsize::new(0));
        let inside = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let (ov, ins) = (overlap.clone(), inside.clone());
            handles.push(std::thread::spawn(move || {
                let _p = engine_permit(Engine::DuckDuckGo).lock().unwrap();
                if ins.fetch_add(1, Ordering::SeqCst) > 0 {
                    ov.fetch_add(1, Ordering::SeqCst);
                }
                std::thread::sleep(std::time::Duration::from_millis(30));
                ins.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(
            overlap.load(Ordering::SeqCst),
            0,
            "same engine must never run concurrently"
        );

        // Different engines: their permits are independent, so both can be held
        // at once (no deadlock, no false serialisation).
        let _a = engine_permit(Engine::Bing).lock().unwrap();
        let _b = engine_permit(Engine::Ecosia).lock().unwrap();
    }

    /// D8: DuckDuckGo's HTML markup parses to real external results, with the
    /// `/l/?uddg=` redirect unwrapped to the true target.
    /// D4: the alternative ranking arms run and exhibit their documented
    /// properties. RRF's pathology — a two-engine pair at ranks 8+8 (score
    /// 0.0294) outranks a single-engine #1 (0.0164), because RRF has no
    /// minimum-engine filter — is the exact reason "ours" keeps agreement as the
    /// PRIMARY key. This test records the behaviour rather than asserting one
    /// arm is best (the frozen judged set is too thin to decide precision@5).
    #[test]
    fn ranking_arms_run_and_rrf_shows_its_pathology() {
        let solo = r("solo top", "https://solo.test/x");
        let pair = r("agreed but low", "https://pair.test/y");
        // Engine A: solo at #1. Engines B and C: seven ENGINE-UNIQUE fillers
        // (so filler agreement is 1, not 2) then the pair at rank 8 (index 7).
        let mut b: Vec<_> = (0..7)
            .map(|i| r(&format!("fb{i}"), &format!("https://fb{i}.test/")))
            .collect();
        b.push(pair.clone());
        let mut c: Vec<_> = (0..7)
            .map(|i| r(&format!("fc{i}"), &format!("https://fc{i}.test/")))
            .collect();
        c.push(pair.clone());
        let per = vec![vec![solo.clone()], b, c];
        // RRF ranks the two-engine pair above the solo #1 (the pathology).
        let rrf = rank_rrf(&per);
        assert_eq!(rrf[0].url, "https://pair.test/y", "RRF pathology: {rrf:?}");
        // ours agrees here (agreement wins) but for the RIGHT reason (2 engines).
        let ours = merge_and_rank(&per, "agreed");
        assert_eq!(ours[0].url, "https://pair.test/y");
        // SearXNG and ddgs also run and dedup via canonical_url (no duplicates).
        let sx = rank_searxng(&per);
        let dd = rank_ddgs(&per, "agreed");
        for ranked in [&rrf, &sx, &dd, &ours] {
            let mut seen = std::collections::HashSet::new();
            for hit in ranked {
                assert!(
                    seen.insert(canonical_url(&hit.url)),
                    "duplicate after canonicalisation"
                );
            }
        }
    }

    /// One page at two addresses (stable and nightly docs) is one result; the
    /// place goes to the next one.
    #[test]
    fn the_same_page_at_two_addresses_takes_one_place() {
        let hit = |t: &str, u: &str, s: &str| serp::Result {
            title: t.into(),
            url: u.into(),
            snippet: s.into(),
        };
        let same = "Path::join and PathBuf::push also disregard trailing slashes. These methods do not resolve .. components";
        let longer = format!("{same} or symlinks. For full normalization use Path::canonicalize.");
        let per = vec![vec![
            hit(
                "std::path - Rust",
                "https://doc.rust-lang.org/std/path/index.html",
                same,
            ),
            hit(
                "std::path - Rust",
                "https://doc.rust-lang.org/stable/std/path/",
                &longer,
            ),
            hit(
                "Path join in Rust",
                "https://example.test/join",
                "How to join paths.",
            ),
        ]];
        let out = merge_and_rank(&per, "path join");
        let urls: Vec<&str> = out.iter().map(|r| r.url.as_str()).collect();
        assert_eq!(
            urls,
            [
                "https://doc.rust-lang.org/std/path/index.html",
                "https://example.test/join"
            ]
        );
    }

    /// The same path with an engine's query string and a cut title is the
    /// same page; a query naming another page (another title) is not.
    #[test]
    fn a_query_string_and_a_cut_title_do_not_make_another_page() {
        let hit = |t: &str, u: &str, s: &str| serp::Result {
            title: t.into(),
            url: u.into(),
            snippet: s.into(),
        };
        let base = "https://docs.npmjs.com/resolving-eacces-permissions-errors-when-installing-packages-globally/";
        let per = vec![vec![
            hit(
                "Resolving EACCES permissions errors when installing packages globally | npm Docs",
                base,
                "Configure npm to use the new directory path.",
            ),
            hit(
                "Resolving EACCES permissions errors when installing packages ...",
                &format!("{base}?fileGuid=xxQT"),
                "If you see an EACCES error when you try to install.",
            ),
            hit(
                "Search results for eacces",
                "https://docs.npmjs.com/search?q=eacces",
                "Results.",
            ),
            hit(
                "Search results for prefix",
                "https://docs.npmjs.com/search?q=prefix",
                "Other results.",
            ),
        ]];
        let out = merge_and_rank(&per, "eacces npm");
        let urls: Vec<&str> = out.iter().map(|r| r.url.as_str()).collect();
        assert!(!urls.iter().any(|u| u.contains("fileGuid")), "{urls:?}");
        assert!(urls.contains(&base), "{urls:?}");
        assert_eq!(
            urls.iter().filter(|u| u.contains("search?q=")).count(),
            2,
            "{urls:?}"
        );
    }

    #[test]
    fn duckduckgo_html_parses_and_unwraps_the_redirect() {
        let html = r#"<div class="web-result">
          <h2 class="result__title">
            <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Ftokio.rs%2F&amp;rut=abc">
              The Tokio async runtime</a></h2>
          <a class="result__snippet" href="//x">Tokio is an asynchronous runtime for Rust.</a>
        </div>
        <div class="web-result">
          <h2 class="result__title">
            <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fdocs.rs%2Ftokio&amp;rut=z">
              Tokio docs</a></h2>
          <a class="result__snippet">API documentation for the tokio crate.</a>
        </div>"#;
        let hits = Engine::DuckDuckGo.parse(html);
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert_eq!(
            hits[0].url, "https://tokio.rs/",
            "the uddg redirect must be unwrapped"
        );
        assert!(hits[0].title.contains("Tokio"));
        assert!(hits[0].snippet.contains("asynchronous"));
        assert_eq!(hits[1].url, "https://docs.rs/tokio");
        assert_eq!(engine_of_page(html), Some(Engine::DuckDuckGo));
    }
}
