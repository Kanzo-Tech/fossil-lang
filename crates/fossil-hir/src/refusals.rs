//! **What the compiler says when a program names a provider it cannot use.**
//!
//! Three sentences, and they are the whole of the provider's user-facing prose:
//! a constructor no host installs, a capability the row does not declare, an
//! extension the row does not accept.
//!
//! # Why they are not on `Provider`
//!
//! They were — `fossil_base::Provider::decline_capability` and
//! `decline_extension`, with `Capability::describe`/`noun` supplying the verb
//! phrase and the bare noun. The argument for putting them there was that a row
//! should word its own refusal. The argument against is that
//! `crates/fossil-base/src/providers.rs` opened by saying *«nothing here decides
//! anything about a program»* while its own tests asserted English Fossil
//! compiler errors, and `fossil-base` is the trait-and-db substrate: the
//! catalogue is data a `System` serves, the wording of a refusal is a
//! diagnostic, and a diagnostic belongs to the checker that raises it.
//!
//! The row still owns every *decision* — [`fossil_base::Provider::provides`] and
//! [`fossil_base::Provider::accepts`] are the predicates, and nothing here calls
//! anything else. What moved is only the sentence.
//!
//! # One sentence per refusal, not three
//!
//! [`unknown_constructor`] replaces three spellings of one message that had
//! drifted apart in three crates:
//!
//! ```text
//! fossil-hir/src/lower.rs   `io.linkml` is not a provider this host installs — it has `io.csv`, …
//! fossil-hir/src/shapes.rs  `io.linkml` is not a provider — this host installs `io.csv`, …
//! fossil-engine/src/lib.rs  `io.linkml` is not a provider this host installs
//! ```
//!
//! The third one dropped the list, so the `fossil run` path told an author their
//! constructor was wrong and not what the alternatives were, while `fossil check`
//! on the same file told them both.

use fossil_base::{Capability, Provider};

/// **A constructor no installed row answers to.** Names what was written and
/// then the whole table, because a reader who guessed `io.linkml` needs the list
/// more than the refusal.
#[must_use]
pub fn unknown_constructor(constructor: &str, installed: &[&'static Provider]) -> String {
    let rows: Vec<String> = installed
        .iter()
        .map(|p| format!("`{}`", p.constructor()))
        .collect();
    format!(
        "`{constructor}` is not a provider this host installs — it has {}",
        rows.join(", ")
    )
}

/// **A capability the row does not declare**, naming both — the row and what was
/// asked of it.
///
/// `type { P } := io.csv("users.csv")` → «`io.csv` reads rows, not types».
/// `installed` is the whole table, so the message can end by naming the rows
/// that DO have the capability rather than leaving the reader to guess.
#[must_use]
pub fn decline_capability(
    row: &Provider,
    wanted: Capability,
    installed: &[&'static Provider],
) -> String {
    let mine: Vec<&str> = row.capabilities().map(describe).collect();
    let has = if mine.is_empty() {
        "does nothing".to_string()
    } else {
        mine.join(" and ")
    };
    let able: Vec<String> = installed
        .iter()
        .filter(|p| p.provides(wanted))
        .map(|p| format!("`{}`", p.constructor()))
        .collect();
    let tail = match able.as_slice() {
        [] => String::new(),
        [one] => format!(" — {one} {}", describe(wanted)),
        many => format!(" — {} read {}", many.join(", "), noun(wanted)),
    };
    format!(
        "`{}` {}, not {}{}",
        row.constructor(),
        has,
        noun(wanted),
        tail
    )
}

/// **An extension the row does not accept**, naming both — the constructor and
/// the extension.
///
/// `io.shex("catalogue.ttl")` → «`io.shex` reads `.shex`, `.shexj` or `.shexc`
/// documents, and `catalogue.ttl` is `.ttl`».
#[must_use]
pub fn decline_extension(row: &Provider, uri: &str) -> String {
    let mine = join_or(
        &row.extensions
            .iter()
            .map(|e| format!("`.{e}`"))
            .collect::<Vec<_>>(),
    );
    let found = fossil_base::providers::extension_of(uri).map_or_else(
        || format!("`{uri}` has no extension"),
        |ext| format!("`{uri}` is `.{ext}`"),
    );
    format!(
        "`{}` reads {} documents, and {}",
        row.constructor(),
        mine,
        found
    )
}

/// The verb phrase a diagnostic uses: "reads rows" / "reads types".
const fn describe(capability: Capability) -> &'static str {
    match capability {
        Capability::ReadRows => "reads rows",
        Capability::ReadTypes => "reads types",
    }
}

/// The bare noun, for the second half of "…, not types".
const fn noun(capability: Capability) -> &'static str {
    match capability {
        Capability::ReadRows => "rows",
        Capability::ReadTypes => "types",
    }
}

/// `"a"` / `"a or b"` / `"a, b or c"`.
fn join_or(items: &[String]) -> String {
    match items {
        [] => "no".to_string(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fossil_base::providers::{CSV, DATA, JSON, PARQUET, RDF};
    use fossil_graph_schema::{OutputShapes, Rejection};

    #[allow(clippy::unnecessary_wraps)]
    fn decode_nothing(_uri: &str, _text: &str) -> Result<OutputShapes, Rejection> {
        Ok(OutputShapes::new(Vec::new(), Vec::new()))
    }

    static SHEX: Provider = Provider {
        name: "shex",
        extensions: &["shex", "shexj", "shexc"],
        reads_rows: None,
        reads_types: Some(decode_nothing),
    };

    static SHACL: Provider = Provider {
        name: "shacl",
        extensions: &["ttl", "shacl"],
        reads_rows: None,
        reads_types: Some(decode_nothing),
    };

    /// The table a COMPILING host installs — the shape of
    /// `fossil_descriptors_output::PROVIDERS`, which this crate may not name.
    static TABLE: &[&Provider] = &[&CSV, &JSON, &PARQUET, &RDF, &SHEX, &SHACL];

    /// The message names BOTH — the row and what was asked of it — and then
    /// says who could.
    #[test]
    fn asking_for_a_capability_a_row_lacks_names_both() {
        assert_eq!(
            decline_capability(&CSV, Capability::ReadTypes, TABLE),
            "`io.csv` reads rows, not types — `io.shex`, `io.shacl` read types"
        );
        assert_eq!(
            decline_capability(&SHEX, Capability::ReadRows, TABLE),
            "`io.shex` reads types, not rows — `io.csv`, `io.json`, `io.parquet`, \
             `io.rdf` read rows"
        );
        // One candidate takes the singular, because a message that reads like a
        // typo is a message a reader distrusts.
        assert_eq!(
            decline_capability(&CSV, Capability::ReadTypes, &[&CSV, &SHEX]),
            "`io.csv` reads rows, not types — `io.shex` reads types"
        );
    }

    /// A host that installs no row with the capability says nothing about who
    /// could, rather than trailing off after an em dash.
    #[test]
    fn a_host_with_no_candidate_names_none() {
        assert_eq!(
            decline_capability(&CSV, Capability::ReadTypes, DATA),
            "`io.csv` reads rows, not types"
        );
    }

    /// The extension refusal names the constructor and the extension — the
    /// `io.shex("catalogue.ttl")` case ruling 13 leads with.
    #[test]
    fn an_extension_is_declined_in_the_rows_own_words() {
        assert_eq!(
            decline_extension(&SHEX, "catalogue.ttl"),
            "`io.shex` reads `.shex`, `.shexj` or `.shexc` documents, and \
             `catalogue.ttl` is `.ttl`"
        );
        assert_eq!(
            decline_extension(&CSV, "users"),
            "`io.csv` reads `.csv` documents, and `users` has no extension"
        );
        assert_eq!(
            decline_extension(&SHACL, "g.json"),
            "`io.shacl` reads `.ttl` or `.shacl` documents, and `g.json` is `.json`"
        );
    }

    /// **One sentence, whichever path raised it.** The three call sites used to
    /// word this differently, and the `fossil run` one dropped the list.
    #[test]
    fn an_unknown_constructor_always_carries_the_installed_list() {
        assert_eq!(
            unknown_constructor("io.linkml", DATA),
            "`io.linkml` is not a provider this host installs — it has `io.csv`, \
             `io.json`, `io.parquet`, `io.rdf`"
        );
        assert_eq!(
            unknown_constructor("io.linkml", TABLE),
            "`io.linkml` is not a provider this host installs — it has `io.csv`, \
             `io.json`, `io.parquet`, `io.rdf`, `io.shex`, `io.shacl`"
        );
    }
}
