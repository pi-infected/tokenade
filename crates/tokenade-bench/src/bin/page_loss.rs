//! **What the DELIVERED page loses of its source, element by element.**
//!
//! Three iterations in a row rewrote this probe by
//! hand, and the first version lied: it looked for a BARE text prefix
//! in markdown, where `` `code` ``, `**bold**` and `⟨12⟩` references cut
//! the words. The three worst "losses" it named were its own
//! artifacts. So the normalisation lives HERE, written once.
//!
//! ```text
//! page_loss <page|all> [--elements p,li,h2]   # lost per element
//! page_loss <page> --render                   # the page as delivered
//! page_loss --canary                          # validates the instrument
//! ```
//!
//! **Mandatory CANARY**: `--canary` builds a page in which
//! one paragraph is dropped by a known decoration class and another is not. If
//! the instrument does not return exactly "one lost, one kept", it does NOT return
//! "nothing found" — it returns `INSTRUMENT NOT VALIDATED` and exits with failure.

const DEFAULT_ELEMENTS: &str = "p,li";
/// Below this, a fragment does not carry enough words to be searched without
/// hitting a neighbour's text.
const MIN_WORDS: usize = 4;
const PROBE_WORDS: usize = 4;

/// The two bounds above count WORDS SEPARATED BY SPACES, and a
/// script that does not use any — Chinese, Japanese, Thai — has none.
///
/// Both were wrong because of it, in opposite directions and both
/// silent. `MIN_WORDS` DROPS a whole paragraph of Chinese because it
/// counts a single "word": the probe never looks for it and counts it neither
/// kept nor lost. `PROBE_WORDS`, on the mixed fragments that get through
/// anyway (one English parenthesis is enough), takes "four words" that cover
/// the WHOLE paragraph — it must then match to the character, footnote
/// included, so it always misses.
///
/// MEASURED on 2026-09-10, before the fix: `mdnjp_leaf`, a Japanese page in the
/// corpus for months, reported `<li> kept 0 lost 11` and `<p> kept 5
/// lost 7` — a perfectly delivered rendering, described as an almost total loss,
/// without anyone re-reading it. A Chinese page added the same day:
/// `<p> kept 0 lost 25`.
///
/// The bound is therefore set per script TYPE, not by a single threshold:
/// a fragment without spaces is measured in CHARACTERS. Pages with spaces —
/// that is, all the others in the corpus — do not move by a byte, which
/// is checkable and checked.
const MIN_CHARS_SPACELESS: usize = 12;
const PROBE_CHARS_SPACELESS: usize = 12;

/// Is the fragment written without spaces?
///
/// The question is NOT "does it contain ideographs": an English paragraph
/// that quotes a Japanese word contains some and is searched very well by its words. The
/// question is whether spaces split this text, so we compare the
/// mass: more than half the non-blank characters belong to a
/// script that does not separate its words.
fn is_spaceless(t: &str) -> bool {
    let mut dense = 0usize;
    let mut total = 0usize;
    for c in t.chars().filter(|c| !c.is_whitespace()) {
        total += 1;
        let u = c as u32;
        // Han, hiragana, katakana, the compatibility forms, and Thai.
        if (0x3040..=0x30FF).contains(&u)
            || (0x3400..=0x4DBF).contains(&u)
            || (0x4E00..=0x9FFF).contains(&u)
            || (0xF900..=0xFAFF).contains(&u)
            || (0xFF66..=0xFF9D).contains(&u)
            || (0x0E00..=0x0E7F).contains(&u)
            || (0x20000..=0x2FA1F).contains(&u)
        {
            dense += 1;
        }
    }
    total > 0 && dense * 2 > total
}

/// Does the fragment carry enough to be searched without hitting a neighbour?
fn long_enough_to_search(t: &str) -> bool {
    if is_spaceless(t) {
        t.chars().count() >= MIN_CHARS_SPACELESS
    } else {
        t.split_whitespace().count() >= MIN_WORDS
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // The common seam: refuses an instrument older than the tree it
    // measures, and refuses an unknown flag.
    tokenade_bench::reject_unknown_args(&["--canary", "--elements", "--render"], &args);
    if args.iter().any(|a| a == "--canary") {
        std::process::exit(canary());
    }
    let target = args.first().cloned().unwrap_or_else(|| "all".into());
    // Reading the DELIVERED page is part of the protocol ("read every diff, never
    // the total") and used to be done with a throwaway binary rewritten at
    // every iteration. One page, as the agent receives it.
    if args.iter().any(|a| a == "--render") {
        // **`--render` takes ONE page, and used to say so through a file system
        // error.** `--render` alone, or with `all`, died on
        // `expect("the page")` with `No such file or directory`: the reader
        // looks for a broken disk where they simply passed the wrong name.
        // It is the class `reject_unknown_args` already covers for
        // flags — an instrument names the usage mistake, it does not
        // let it come out as a panic from a layer below.
        let path = page_path(&target);
        match std::fs::read_to_string(&path) {
            Ok(html) => println!("{}", distill(&html, &target)),
            Err(_) => {
                eprintln!(
                    "page_loss --render: \"{target}\" is not a page of the corpus.\n\
                     --render renders ONE page; \"all\" is not one (it is the \n\
                     default of the --elements mode). Available pages:"
                );
                for n in corpus_names() {
                    eprintln!("  {n}");
                }
                std::process::exit(2);
            }
        }
        return;
    }
    let elements: Vec<String> = args
        .windows(2)
        .find(|w| w[0] == "--elements")
        .map(|w| w[1].clone())
        .unwrap_or_else(|| DEFAULT_ELEMENTS.into())
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if canary() != 0 {
        eprintln!("INSTRUMENT NOT VALIDATED — the canary no longer reproduces the known defect");
        std::process::exit(2);
    }
    let dir = std::path::Path::new("crates/tokenade-core/tests/fixtures/pages");
    let mut pages: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .expect("the page corpus")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("html"))
        .collect();
    pages.sort();
    for p in pages {
        let name = p.file_stem().unwrap().to_string_lossy().to_string();
        if target != "all" && target != name {
            continue;
        }
        let html = std::fs::read_to_string(&p).expect("the page");
        let md = normalise(&distill(&html, &name));
        for el in &elements {
            let (kept, lost) = fragments(&html, el);
            let mut missing: Vec<String> = Vec::new();
            let mut n_kept = 0usize;
            for frag in kept.iter().chain(lost.iter()) {
                if md.contains(&probe(frag)) {
                    n_kept += 1;
                } else {
                    missing.push(frag.chars().take(110).collect());
                }
            }
            if n_kept + missing.len() == 0 {
                continue;
            }
            println!("{name}\t<{el}>\tkept {n_kept}\tlost {}", missing.len());
            for m in &missing {
                println!("\t✗ {m}");
            }
        }
    }
}

/// A corpus name, or a PATH to a page of your own. Isolating a rule requires
/// a hand-written page, and writing it into `fixtures/pages` would bring it
/// into every ratchet of the repo.
/// The corpus names, sorted — so that a usage mistake answers with the LIST
/// rather than with the name of the missing file.
fn corpus_names() -> Vec<String> {
    let dir =
        tokenade_bench::Bench::workspace_root().join("crates/tokenade-core/tests/fixtures/pages");
    let mut out: Vec<String> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("html"))
                .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

fn page_path(target: &str) -> String {
    if target.ends_with(".html") {
        return target.to_string();
    }
    // Against the repo ROOT, not against the current directory: the test
    // harness runs each binary from its crate's directory, and a
    // probe that no longer finds the corpus would report "nothing found".
    tokenade_bench::Bench::workspace_root()
        .join("crates/tokenade-core/tests/fixtures/pages")
        .join(format!("{target}.html"))
        .to_string_lossy()
        .into_owned()
}

/// Kept / lost for a corpus page and an element — the same decision as
/// the report, extracted so that the canary floor shares it.
#[cfg(test)]
fn counts(page: &str, element: &str) -> (usize, usize) {
    let html = std::fs::read_to_string(page_path(page)).expect("the page");
    let md = normalise(&distill(&html, page));
    let (frags, _) = fragments(&html, element);
    let lost = frags.iter().filter(|f| !md.contains(&probe(f))).count();
    (frags.len(), lost)
}

fn distill(html: &str, name: &str) -> String {
    tokenade_core::web_pipeline::distill_page(
        html,
        &format!("https://example.test/{name}"),
        Some("text/html; charset=utf-8"),
    )
    .md
}

/// The text of the source's `<el>` elements, decoded and normalised like the
/// rendering, keeping only fragments long enough to be searched.
fn fragments(html: &str, el: &str) -> (Vec<String>, Vec<String>) {
    let re = regex::Regex::new(&format!(r"(?is)<{el}\b[^>]*>(.*?)</{el}>")).unwrap();
    // **An attribute value can contain `>`**, and `<[^>]+>` breaks on it: the
    // Parsoid `data-mw` carries JSON, and Wikipedia rendered this in the
    // SOURCE text the probe compares — `2/sup + ''b''sup 2/sup {{=}} ''c''
    // sup 2/sup,"}},"i":0}}}'`. A tag closes at the `>` that is NOT inside
    // a quoted value, and its name starts with a letter, a `/`, a
    // `!` or a `?` — otherwise a prose `<` (`a < b and c > d`) passed
    // for a tag and took the sentence with it.
    let tags =
        regex::Regex::new(r#"(?is)<[a-z!/?][^>"']*(?:(?:"[^"]*"|'[^']*')[^>"']*)*>"#).unwrap();
    // The rendering drops `<svg>`, `<script>` and `<style>` entirely; the source
    // must be read the same way, otherwise the probe compares two different
    // texts. MEASURED: the Guardian puts a DRAWN quotation mark in its opinion
    // headlines, whose accessible `<title>` is "double quotation mark", and
    // the probe named **22 delivered headlines** "lost" for that reason alone.
    let subtrees =
        regex::Regex::new(r"(?is)<svg\b.*?</svg>|<script\b.*?</script>|<style\b.*?</style>")
            .unwrap();
    let mut out = Vec::new();
    for c in re.captures_iter(html) {
        let kept = subtrees.replace_all(c.get(1).unwrap().as_str(), " ");
        let kept = fold_math_to_its_tex(&kept);
        let inner = strip_tags_as_a_browser_would(&tags, &kept);
        let t = normalise(&tokenade_core::web::decode_entities(&inner));
        if long_enough_to_search(&t) {
            out.push(t);
        }
    }
    (out, Vec::new())
}

/// Remove what the pattern catches, only putting a space where the
/// removal would weld two words the page never glued together.
fn erase_without_welding(re: &regex::Regex, s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut cursor = 0usize;
    while let Some(m) = re.find_at(s, cursor) {
        out.push_str(&s[cursor..m.start()]);
        let before = out.chars().next_back();
        let after = s[m.end()..].chars().next();
        if before.is_some_and(char::is_alphanumeric) && after.is_some_and(char::is_alphanumeric) {
            out.push(' ');
        }
        cursor = m.end();
    }
    out.push_str(&s[cursor..]);
    out
}

/// **A tag is not a space.** A browser inserts none: the
/// spaces of a sentence are those of the SOURCE. Replacing each tag with
/// a space gave `Let A , B , C be the vertices` where the page says
/// `Let A, B, C be the vertices` — and the probe named "lost" a paragraph
/// delivered word for word, on the only page of the corpus that puts its variables in
/// italics.
///
/// The only space a rendering adds is the one that prevents WELDING two
/// words the page never glued: `<span>amount</span><span>integer</span>`
/// is not `amountinteger`. It is the rule of `welds_a_token`, and its
/// BOUND matters just as much: an element → TEXT boundary separates nothing, otherwise
/// `<em><strong>de</strong></em>serializing` would render `de serializing`.
/// Three questions, in this order, and each one cost a diff read:
///  1. does the run of tags carry a closing AND an opening? Otherwise
///     it is a boundary with TEXT, and it welds;
///  2. is one of the names a BLOCK element? The rendering separates two blocks:
///     the Guardian sets `<div>Brazil</div><span>‘It’s total devastation’…</span>`,
///     a section label and a headline, and welding them gave `Brazil‘It’s`. The
///     question is asked of [`tokenade_core::web_tree::is_phrasing_element_name`],
///     the list of the delivered path;
///  3. otherwise, between two PHRASING elements, only welding two words
///     justifies a space: Wikipedia writes
///     `<a>Neugebauer 1969</a><span>, p.</span>`, and separating there gave
///     `Neugebauer 1969 , p. 36.`, a space neither the page nor the rendering has.
fn strip_tags_as_a_browser_would(tags: &regex::Regex, s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut cursor = 0usize;
    while let Some(m) = tags.find_at(s, cursor) {
        // The RUN of contiguous tags, taken as one block.
        let mut end = m.end();
        let mut has_close = false;
        let mut has_open = false;
        let mut has_block = false;
        let mut note = |tag: &str| {
            if tag.starts_with("</") {
                has_close = true;
            } else {
                has_open = true;
            }
            let name: String = tag
                .trim_start_matches('<')
                .trim_start_matches('/')
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect::<String>()
                .to_ascii_lowercase();
            if !tokenade_core::web_tree::is_phrasing_element_name(&name) {
                has_block = true;
            }
        };
        note(&s[m.start()..m.end()]);
        while let Some(next) = tags.find_at(s, end) {
            if next.start() != end {
                break;
            }
            note(&s[next.start()..next.end()]);
            end = next.end();
        }
        out.push_str(&s[cursor..m.start()]);
        let welds = out.chars().next_back().is_some_and(char::is_alphanumeric)
            && s[end..].chars().next().is_some_and(char::is_alphanumeric);
        if has_close && has_open && (has_block || welds) {
            out.push(' ');
        }
        cursor = end;
    }
    out.push_str(&s[cursor..]);
    out
}

/// **The source writes a formula three times and the rendering delivers it once.** The
/// presentation MathML, the TeX `<annotation>` and the fallback image's `alt`
/// say the same equation; without this fold the probe compares the first to the
/// third and names "lost" a paragraph that starts with a formula.
/// The question is asked of the DELIVERED PATH's function, not of a
/// second spelling of the same rule.
fn fold_math_to_its_tex(html: &str) -> String {
    let open = regex::Regex::new(r"(?is)<math\b").unwrap();
    let mut out = String::with_capacity(html.len());
    let mut cursor = 0usize;
    while let Some(m) = open.find_at(html, cursor) {
        let Some(k) = html[m.end()..].to_ascii_lowercase().find("</math>") else {
            break;
        };
        let end = m.end() + k + "</math>".len();
        out.push_str(&html[cursor..m.start()]);
        match tokenade_core::web_tree::tex_of_math_fragment(&html[m.start()..end]) {
            Some(tex) => {
                out.push(' ');
                out.push_str(&tex);
                out.push(' ');
            }
            None => out.push_str(&html[m.start()..end]),
        }
        cursor = end;
    }
    out.push_str(&html[cursor..]);
    out
}

fn probe(fragment: &str) -> String {
    if is_spaceless(fragment) {
        return fragment.chars().take(PROBE_CHARS_SPACELESS).collect();
    }
    fragment
        .split_whitespace()
        .take(PROBE_WORDS)
        .collect::<Vec<_>>()
        .join(" ")
}

/// This file's only reason to exist: the rendering is MARKDOWN and the source
/// is bare text. A four-word prefix searched as is misses as soon as a word
/// carries emphasis, a `⟨12⟩` reference, a link or an entity.
fn normalise(s: &str) -> String {
    let mut t = json_unescape(s);
    // The pilcrow is a heading's permalink anchor (`go_pkg`, MDN,
    // Wikipedia put one on every section): it is in the SOURCE and
    // never in the rendering, and without this the probe named **114 of the
    // 116 level-4 headings** "lost" on a page delivered with 191 headings.
    // ORDER matters: the TARGET of a markdown link goes first, with its
    // parentheses. Removing punctuation first left `#term-thread-state`
    // in the middle of the sentence, and the probe named **23 of the 172
    // definitions of the Python glossary** "lost", all of them delivered.
    // **A removal does not put a space either.** Replacing each pattern with
    // a space produced `JSON , the ubiquitous` where the rendering says
    // `[JSON](url), the ubiquitous`: the link target went away LEAVING a
    // space neither the source nor the page has. The rule is the same as for
    // tags — only add a space where two words would be WELDED.
    // `$` is a bound the RENDERING puts around a formula and that the
    // source does not have: without this, `When ${\displaystyle …}$ is` never
    // contained the searched prefix `When {\displaystyle …}`, and three of the four
    // losses announced on the formula page were this gap.
    for pattern in [
        r"\]\([^)]*\)",
        r"⟨\d+⟩",
        r"[`*_#\[\]()<>|¶$]",
        r"https?://\S+",
        r"\\",
    ] {
        t = erase_without_welding(&regex::Regex::new(pattern).unwrap(), &t);
    }
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A CMS body travels inside a JSON string, so the source carries `–`
/// and `\n` where the rendering carries the dash and the line break. Without this decoding
/// the probe measures itself: it named "lost" a perfectly delivered `stop smoking
/// – having diabetes` (the same lesson).
fn json_unescape(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') | Some('r') | Some('t') => out.push(' '),
            Some('u') => {
                let h: String = it.by_ref().take(4).collect();
                match u32::from_str_radix(&h, 16).ok().and_then(char::from_u32) {
                    Some(ch) => out.push(ch),
                    None => out.push_str(&h),
                }
            }
            Some(x) => out.push(x),
            None => break,
        }
    }
    out
}

/// Reproduces a KNOWN defect: a paragraph in a cookie banner is
/// dropped, its neighbour is not. The instrument must see exactly that.
fn canary() -> i32 {
    // Enough body to clear the extraction ladder's FLOOR: under
    // `EXTRACT_VALID_MIN_CHARS`, the second layer starts over without dropping decoration and
    // the canary would turn green on a page that does not exercise the rule.
    let filler = "La bibliotheque expose une seule fonction publique et rien d autre. \
        Elle prend un chemin, rend un document, et ne touche jamais au reseau. ";
    let html = format!(
        "<html><body><main>\
        <div class=\"cookie-notice\"><p>Nous utilisons des temoins de connexion sur ce site.</p></div>\
        <article><p>{filler}</p><p>Materials and care ¶ <a href=\"/x\">un lien</a> {filler}{filler}</p></article>\
        </main></body></html>"
    );
    let html = html.as_str();
    let md = normalise(&distill(html, "canary"));
    let (frags, _) = fragments(html, "p");
    let lost: Vec<&String> = frags.iter().filter(|f| !md.contains(&probe(f))).collect();
    let kept: Vec<&String> = frags.iter().filter(|f| md.contains(&probe(f))).collect();
    if frags.len() == 3 && lost.len() == 1 && kept.len() == 2 && lost[0].contains("temoins") {
        0
    } else {
        eprintln!(
            "canary: {} fragment(s), {} lost, {} kept — expected 3/1/2 with the banner lost",
            frags.len(),
            lost.len(),
            kept.len()
        );
        1
    }
}

/// **The artifacts this probe produced, each one once.**
///
/// Three times in a row it named "lost" perfectly delivered text, and
/// three times the cause was the same: it compared the SOURCE to the RENDERED
/// markdown without applying the rendering rules. One test per artifact; the canary
/// carries them too, so that the executable refuses to return a verdict when
/// one of them comes back.
#[cfg(test)]
mod what_the_probe_invented {
    use super::{fragments, normalise, probe};

    #[test]
    fn a_tag_is_not_a_space() {
        // `archwiki_page`, `wikipedia_math`: a page that puts its variables in
        // italics writes `Let <i>A</i>, <i>B</i>, <i>C</i> be the vertices`, and
        // the probe read `Let A , B , C` — five paragraphs delivered word for
        // word, named lost.
        let (frags, _) = fragments(
            "<p>Let <i>A</i>, <i>B</i>, <i>C</i> be the vertices.</p>",
            "p",
        );
        assert_eq!(
            frags,
            vec!["Let A, B, C be the vertices.".to_string()],
            "{frags:?}"
        );
    }

    #[test]
    fn two_boxes_the_page_never_glued_keep_their_space() {
        // The BOUND, and it has two halves. Two BLOCK elements are separated
        // by the rendering (the Guardian sets a section label and a headline side by side),
        // and two PHRASING elements are only separated if welding them would make a word
        // the page does not contain.
        let (block, _) = fragments(
            "<h3><div>Brazil</div><span>‘It’s total devastation’ says the report</span></h3>",
            "h3",
        );
        assert!(block[0].starts_with("Brazil ‘It’s"), "{block:?}");
        let (weld, _) = fragments(
            "<h4><span>amount</span><span>integer</span> is the field name here</h4>",
            "h4",
        );
        assert!(weld[0].starts_with("amount integer"), "{weld:?}");
        let (glue, _) = fragments(
            "<p><a>Neugebauer 1969</a><span>, p. 36</span> is the reference here.</p>",
            "p",
        );
        assert!(glue[0].starts_with("Neugebauer 1969, p. 36"), "{glue:?}");
    }

    #[test]
    fn a_formula_is_read_as_the_page_declares_it() {
        // The source writes the formula three times and the rendering delivers it once.
        let (frags, _) = fragments(
            "<p><math alttext=\"a^{2}+b^{2}\"><mi>a</mi><mn>2</mn></math> is the equation here.</p>",
            "p",
        );
        assert!(
            frags[0].starts_with("a^{2}+b^{2} is the equation"),
            "{frags:?}"
        );
    }

    #[test]
    fn a_permalink_pilcrow_is_not_part_of_the_heading() {
        // `go_pkg`: 114 of the 116 level-4 headings named lost, on a page
        // that delivers 191 of them.
        let source = normalise("Clients and Transports ¶");
        let rendered = normalise("#### Clients and Transports");
        assert!(
            rendered.contains(&probe(&source)),
            "{source:?} / {rendered:?}"
        );
    }

    #[test]
    fn the_target_of_a_link_is_not_part_of_the_sentence() {
        // `py_glossary`: 23 of the 172 definitions named lost, all delivered.
        let source = normalise("A thread state that is active for the current OS thread.");
        let rendered = normalise(
            "A [thread state](#term-thread-state) that is active for the current OS thread.",
        );
        assert!(
            rendered.contains(&probe(&source)),
            "{source:?} / {rendered:?}"
        );
    }

    #[test]
    fn a_drawn_glyph_is_not_a_word_of_the_title() {
        // The Guardian puts a DRAWN quotation mark in its opinion headlines, whose
        // accessible `<title>` is "double quotation mark": 22 delivered headlines
        // named lost.
        let html = "<h3><svg><title>double quotation mark</title></svg>The trouble with the \
                    latest nominees? Integrity</h3>";
        let (frags, _) = fragments(html, "h3");
        assert_eq!(frags.len(), 1, "{frags:?}");
        assert!(
            frags[0].starts_with("The trouble with"),
            "the glyph stayed in the headline: {:?}",
            frags[0]
        );
    }

    /// A script without spaces does not have "four words", and both bounds
    /// were wrong because of it in opposite directions — one drops the fragment, the other
    /// makes it a prefix as long as the paragraph.
    #[test]
    fn a_script_without_spaces_is_measured_in_characters() {
        let zh = "人工智能的常见应用包括高级网络搜索引擎、聊天机器人、虚拟助手";
        assert!(super::is_spaceless(zh));
        // A single "word": the old bound dropped it without saying anything.
        assert_eq!(zh.split_whitespace().count(), 1);
        assert!(super::long_enough_to_search(zh));
        // And the searched prefix is BOUNDED, instead of being the whole paragraph.
        let pr = probe(zh);
        assert_eq!(pr.chars().count(), super::PROBE_CHARS_SPACELESS);
        assert!(zh.starts_with(&pr));
        // Too short to be searched without hitting a neighbour: the bound
        // still exists, it is only counted in the right unit.
        assert!(!super::long_enough_to_search("人工智能"));
    }

    /// The question is "do spaces split this text", not "is there an
    /// ideograph somewhere": an English sentence quoting one is searched
    /// very well by its words, and measuring it in characters would shorten it.
    #[test]
    fn a_latin_sentence_quoting_one_ideograph_keeps_its_word_probe() {
        let en = "The character 猫 means cat in Chinese and Japanese alike.";
        assert!(!super::is_spaceless(en));
        assert_eq!(probe(en), "The character 猫 means");
    }

    /// The FLOOR: these two pages are delivered whole on these elements, and
    /// a probe that finds a loss there is wrong. It is what makes
    /// `--canary` a verdict and not a formality.
    #[test]
    fn two_pages_delivered_whole_report_nothing_lost() {
        for (page, element, floor) in [
            ("py_glossary", "dd", 150),
            ("go_pkg", "h4", 70),
            // This page puts its variables in italics, and the probe
            // named five paragraphs delivered word for word lost.
            ("archwiki_page", "p", 110),
        ] {
            let (kept, lost) = super::counts(page, element);
            assert!(kept >= floor, "{page} <{element}>: only {kept} fragments");
            assert_eq!(lost, 0, "{page} <{element}>: {lost} invented loss(es)");
        }
    }

    /// The CEILING, the other half of the same verdict: on these pages the probe
    /// invented losses in bulk, and the number it reports must no longer
    /// climb back there. Measured 2026-09-11, whole corpus: **1,504 losses
    /// announced, 1,418 after** — 86 delivered fragments it named lost.
    #[test]
    fn the_probe_no_longer_invents_losses_in_bulk() {
        for (page, element, ceiling) in [
            // 36 prices — `<span>$</span><span>71</span><span>.99</span>` read
            // `$ 71 .99`, never found in a rendering that says `$71.99`.
            ("newegg_cat", "li", 20),
            // 7 definitions of the Python glossary, all delivered.
            ("py_glossary", "p", 2),
            // The formula page: 11 paragraphs announced lost, 4 real.
            ("wikipedia_math", "p", 5),
        ] {
            let (_, lost) = super::counts(page, element);
            assert!(
                lost <= ceiling,
                "{page} <{element}>: {lost} lost, ceiling {ceiling}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{corpus_names, page_path};

    /// **The list the usage mistake displays must exist.**
    ///
    /// `--render all` died on `expect("the page")` with `No such file or
    /// directory`: the reader looks for a broken disk where they passed the
    /// wrong name. The message now names the mistake AND the available
    /// pages — but a message that listed nothing would be worse than the
    /// panic, so the reach floor is here, not in a comment.
    #[test]
    fn the_corpus_names_itself() {
        let names = corpus_names();
        assert!(names.len() >= 30, "corpus not found: {names:?}");
        assert!(names.contains(&"go_pkg".to_string()), "{names:?}");
        // Sorted: the list is read by a human looking for a name.
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
        // `all` is NOT a page — that is what the message must say.
        assert!(!names.contains(&"all".to_string()));
    }

    /// The bound of the path: a corpus name resolves, `all` does not. Without it,
    /// "the file does not exist" and "the name is wrong" would be the same case.
    #[test]
    fn a_corpus_name_resolves_and_all_does_not() {
        assert!(std::path::Path::new(&page_path("go_pkg")).exists());
        assert!(!std::path::Path::new(&page_path("all")).exists());
        // An explicit path stays accepted as is.
        assert_eq!(page_path("x/y.html"), "x/y.html");
    }
}
