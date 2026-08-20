//! **The substrate does not talk to a program's author.**
//!
//! `fossil-base` is the Salsa `Db` trait plus the `System` abstraction, and
//! `CLAUDE.md` names the violation by name: *"Putting compiler logic in
//! `fossil-base` — it is the trait + db substrate, no business logic."* The
//! provider catalogue is admitted here because it is data a `System` serves; the
//! two functions that composed **English Fossil compiler errors** from it were
//! not, and they moved to `fossil_hir::refusals`.
//!
//! `crates/fossil-base/src/providers.rs` had asserted that in a `///` for
//! months — *«nothing here decides anything about a program»* — while its own
//! tests compared `decline_capability`'s output to a sentence, character for
//! character. This is that comment, rewritten as something that fails.
//!
//! # What it checks, and it is deliberately crude
//!
//! Every string literal outside `mod tests` must be a single token — no ASCII
//! space. A catalogue's literals are `csv`, `ttl`, `io.{}`, `shexj`. A
//! diagnostic is a sentence, and a sentence has a space in it. Nothing subtler
//! than that would have been worth having: the format string the two departed
//! functions were built around reads `reads {} documents, and {}`, and it trips
//! on the first word boundary.
//!
//! # What it cannot prove
//!
//! - **It does not see prose that reaches the author some other way.** A
//!   `String` assembled from single tokens, or a `&'static str` imported from a
//!   dependency, passes.
//! - **It is one file, not the crate.** `diagnostic.rs` is here on purpose — a
//!   `Diagnostic` is the type a message is carried IN, and the substrate owning
//!   the envelope is not the substrate writing the letter. Extending this scan
//!   crate-wide would need that distinction expressed, and it is not.
//! - **It reads text, not tokens.** A literal in a macro that this file never
//!   spells is invisible to it.

/// The source under scan, embedded at compile time so there is no path to
/// resolve and no working directory to be wrong about.
const PROVIDERS: &str = include_str!("../src/providers.rs");

/// Everything before `mod tests`. The test module is allowed its assertion
/// messages; the module proper is not allowed a sentence.
fn shipped_source(src: &str) -> &str {
    src.split_once("#[cfg(test)]")
        .map_or(src, |(before, _)| before)
}

/// The string literals on one line, ignoring `//`-comments (doc comments are
/// prose by definition and are not what this is about) and `\"` escapes.
fn literals(line: &str) -> Vec<String> {
    let code = line.trim_start();
    if code.starts_with("//") {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    let mut escaped = false;
    for ch in line.chars() {
        match current.as_mut() {
            Some(buf) => {
                if escaped {
                    escaped = false;
                    buf.push(ch);
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    out.push(std::mem::take(buf));
                    current = None;
                } else {
                    buf.push(ch);
                }
            }
            None if ch == '"' => current = Some(String::new()),
            None => {}
        }
    }
    out
}

#[test]
fn the_provider_catalogue_carries_no_sentence() {
    let offenders: Vec<String> = shipped_source(PROVIDERS)
        .lines()
        .flat_map(literals)
        .filter(|lit| lit.contains(' '))
        .collect();
    assert!(
        offenders.is_empty(),
        "`providers.rs` is the catalogue, not the diagnostic — a message belongs \
         in `fossil_hir::refusals`, beside the checker that raises it. Found: {offenders:?}"
    );
}

/// The scan is worth nothing if it cannot see a sentence, and a guard that
/// cannot fail is the failure mode this repo has already paid for. So: feed it
/// the exact line that used to be in `providers.rs`.
#[test]
fn the_scan_would_have_caught_the_two_functions_that_left() {
    let removed = r#"
        format!(
            "`{}` reads {} documents, and {}",
            self.constructor(),
            mine,
            found
        )
    "#;
    let found: Vec<String> = literals(removed.lines().nth(2).expect("the format string"))
        .into_iter()
        .filter(|lit| lit.contains(' '))
        .collect();
    assert_eq!(found.len(), 1, "the sentence was not seen: {found:?}");
}

/// A catalogue literal is one token and must keep passing — otherwise the fix
/// for a red run is to delete data.
#[test]
fn the_catalogue_literals_themselves_are_single_tokens() {
    let kept: Vec<String> = shipped_source(PROVIDERS)
        .lines()
        .flat_map(literals)
        .collect();
    assert!(
        kept.iter().any(|l| l == "csv"),
        "the scan did not reach the rows at all: {kept:?}"
    );
    assert!(
        kept.iter().any(|l| l == "io."),
        "nor the constructor prefix"
    );
}
