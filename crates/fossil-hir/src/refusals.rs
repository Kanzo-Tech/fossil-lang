//! **What the compiler adds when a program names a provider it cannot use.**
//!
//! Three refusals — a constructor no host installs, a capability the row does
//! not declare, an extension the row does not accept. The refusal itself is a
//! [`fossil_graph_schema::Problem`] (`provider/unknown`, `provider/wrong-capability`,
//! `provider/wrong-extension`); what lives here is the `help:` line under it,
//! which names the alternatives, because a reader who guessed `io.linkml` needs
//! the list more than the refusal.
//!
//! # Why they are not on `Provider`
//!
//! `crates/fossil-base/src/providers.rs` says *«nothing here decides anything
//! about a program»*, and `fossil-base` is the trait-and-db substrate: the
//! catalogue is data a `System` serves, the wording of a refusal is a
//! diagnostic, and a diagnostic belongs to the checker that raises it. The row
//! still owns every *decision* — [`fossil_base::Provider::provides`] and
//! [`fossil_base::Provider::accepts`] are the predicates, and nothing here calls
//! anything else.
//!
//! # One help per refusal, whichever path raised it
//!
//! `fossil check` (this crate) and `fossil run` (`fossil-df`'s descriptor) both
//! call these, so the two paths cannot drift apart again — they once did, and
//! the `run` one dropped the list.

use fossil_base::{Capability, Provider};

/// The capability as the `provider/wrong-capability` problem words it: `"read
/// rows"` / `"read types"`.
#[must_use]
pub const fn capability(wanted: Capability) -> &'static str {
    match wanted {
        Capability::ReadRows => "read rows",
        Capability::ReadTypes => "read types",
    }
}

/// **A constructor no installed row answers to**: the whole table.
#[must_use]
pub fn unknown_constructor(installed: &[&'static Provider]) -> String {
    let rows: Vec<String> = installed
        .iter()
        .map(|p| format!("`{}`", p.constructor()))
        .collect();
    format!("this host installs {}", rows.join(", "))
}

/// **A capability the row does not declare**: what the row does, and then the
/// rows that DO have the capability rather than leaving the reader to guess.
///
/// `type { P } := io.csv("users.csv")` → «`io.csv` reads rows; `io.shex`,
/// `io.shacl` read types».
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
        [one] => format!("; {one} {}", describe(wanted)),
        many => format!("; {} read {}", many.join(", "), noun(wanted)),
    };
    format!("`{}` {has}{tail}", row.constructor())
}

/// **An extension the row does not accept**: the extensions it does, and the
/// one it was given.
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

/// The verb phrase: "reads rows" / "reads types".
const fn describe(capability: Capability) -> &'static str {
    match capability {
        Capability::ReadRows => "reads rows",
        Capability::ReadTypes => "reads types",
    }
}

/// The bare noun, for "… read types".
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

    /// The help says what the row does, and then who could.
    #[test]
    fn asking_for_a_capability_a_row_lacks_names_both() {
        assert_eq!(
            decline_capability(&CSV, Capability::ReadTypes, TABLE),
            "`io.csv` reads rows; `io.shex`, `io.shacl` read types"
        );
        assert_eq!(
            decline_capability(&SHEX, Capability::ReadRows, TABLE),
            "`io.shex` reads types; `io.csv`, `io.json`, `io.parquet`, `io.rdf` read rows"
        );
        // One candidate takes the singular, because a message that reads like a
        // typo is a message a reader distrusts.
        assert_eq!(
            decline_capability(&CSV, Capability::ReadTypes, &[&CSV, &SHEX]),
            "`io.csv` reads rows; `io.shex` reads types"
        );
    }

    /// A host that installs no row with the capability says nothing about who
    /// could.
    #[test]
    fn a_host_with_no_candidate_names_none() {
        assert_eq!(
            decline_capability(&CSV, Capability::ReadTypes, DATA),
            "`io.csv` reads rows"
        );
    }

    /// The extension refusal names both the constructor and the extension.
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

    /// **The installed list, whichever path raised it.** The call sites used to
    /// word this differently, and the `fossil run` one dropped the list.
    #[test]
    fn an_unknown_constructor_always_carries_the_installed_list() {
        assert_eq!(
            unknown_constructor(DATA),
            "this host installs `io.csv`, `io.json`, `io.parquet`, `io.rdf`"
        );
        assert_eq!(
            unknown_constructor(TABLE),
            "this host installs `io.csv`, `io.json`, `io.parquet`, `io.rdf`, `io.shex`, \
             `io.shacl`"
        );
    }
}
