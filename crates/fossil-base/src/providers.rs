//! **The provider registry — one table.** A row is the name written after
//! `io.`, the extensions it accepts, and the capabilities it declares.
//!
//! # There were two tables for one idea, and they dispatched by different
//! criteria
//!
//! ```text
//! fossil-hir/src/stdlib.rs   SourceKind  { short_name: "csv",  extensions: ["csv"],           lowering }
//! fossil-hir/src/stdlib.rs   SourceKind  { short_name: "rdf",  extensions: ["ttl","nt","n3"], lowering }
//! fossil-base/src/…          ShapeDecoder{ name:       "shex", extensions: ["shex","shexj"],  decode   }
//! ```
//!
//! The same three fields — a name, the extensions it accepts, and what it does
//! with them — modelled twice. The data table dispatched **by name**
//! (`source_kind("io.csv")`); the schema table dispatched **by extension**
//! (`decoder_for(table, uri)`). The measured consequence was that
//! `fossil-hir/src/def_map.rs` wrote `let (_ctor, document) = parse_source_call(…)`
//! and threw the constructor away, so `io.shex("x.ttl")` and `io.shacl("x.ttl")`
//! behaved identically. Two places said the same thing and only one was read,
//! which is how they disagreed in silence.
//!
//! Ruling 13 of `SURFACE-PLAN.md` collapses them: **one row, and dispatch is
//! always by name.** The row checks its own extension and words its own
//! rejection ([`Provider::decline_extension`]); asking a row for a capability it
//! does not declare is an error naming both ([`Provider::decline_capability`]).
//!
//! # The direction is not one axis, it is two
//!
//! | | describes **data** | describes **types** |
//! |---|---|---|
//! | **reads** | `io.csv`, `io.json`, `io.parquet`, `io.rdf` | `io.shex`, `io.shacl` |
//! | **writes** | no syntax (today, the `--dest` flag) | nothing — and yet it already happens |
//!
//! `io.shex` is *input* by the bytes and *output* by the meaning: a file is
//! read, and what it describes is the output contract. That is why it never fit
//! an axis with one direction in it. The bottom-right cell already exists in the
//! artefact and not in the language — a `GraphAr` manifest is a written type
//! document and nothing names it.
//!
//! **The write half is deliberately not modelled.** `grammar.bnf` says in as
//! many words that inventing destination syntax before it is decided is how v0.1
//! filled up with ghosts: *«`io` names only the INPUT. Where a program writes to
//! has no syntax at all»*. [`Capability`] therefore has exactly the two cases
//! the language can spell today, and a third arrives as a variant plus a field
//! on [`Provider`] — a row, never a rule: a new capability enters as a catalogue
//! entry, never as a grammar production.
//!
//! # Why the row type lives here
//!
//! Because the *host* supplies the table and `System` is here. A row that reads
//! types carries a `fn` pointer into a schema language, and the compiler may not
//! link one — `0e6898d` cut ShEx out of `fossil-mir` and the same cut applies to
//! `fossil-hir`. So the rows that read rows are `&'static` constants below (the
//! compiler can name a `DuckDB` reader), the rows that read types come from the
//! crate that owns the parser, and [`crate::system::System::providers`] is where
//! a host hands over the assembled table.
//!
//! This is a table of *behaviour*, and behaviour is what a table of `fn` is for:
//! the extension point is a table of `fn`, never a trait object, because a `fn`
//! pointer gives the `Eq`/`Hash` a memoized core needs and a `dyn` does not. As
//! against [`crate::system::System::descriptors`], which is a table of *data* and
//! therefore has no `fn` in it at all — there is nothing there to dispatch, and a
//! function pointer returning descriptors would be the indirection without the
//! reason. It is not compiler logic: nothing here decides anything about a program.

use std::hash::{Hash, Hasher};

use fossil_graph_schema::{OutputShapes, Rejection};

/// `(uri_as_written, text) -> decoded`.
///
/// **Pure**: no IO, no network, no clock. It is called from inside a tracked
/// query, where an untracked read of the outside world is exactly the bug
/// [`crate::shape_documents::shape_document`] exists to remove. The `uri` is
/// passed because a document can carry relative references and a diagnostic
/// wants to name the file — not so the decoder can go and read it.
pub type DecodeTypes = fn(&str, &str) -> Result<OutputShapes, Rejection>;

/// The native `DuckDB` readers a [`RowReader::Native`] maps to.
///
/// `fossil-mir` exhaustively maps each to a `SourceFormat`, so a new reader is a
/// compile error until handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NativeReader {
    /// `read_csv_auto`.
    CsvAuto,
    /// `read_json_auto`.
    JsonAuto,
    /// `read_parquet`.
    Parquet,
}

/// How a row-reading provider's bytes become rows a `DuckDB` plan can scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RowReader {
    /// A native `DuckDB` table function (`read_csv_auto`, …) — portable
    /// native↔WASM, no custom decode.
    Native(NativeReader),
    /// Something outside the reader materialises the relation (RDF, …) and the
    /// core only scans the result. Lowers to `fossil_mir::SourceFormat::Provider`
    /// and is executed by `fossil-df`. There is no `SourceProvider` trait and no
    /// `fossil-provider-rdf` crate, which is what the comment this replaces
    /// claimed.
    Materialised,
}

/// What a row can be asked to do. The **position** in the program picks which
/// one is asked for; the row declares which ones it has.
///
/// ```text
/// User := io.csv("users.csv")          binding → read rows
/// type { P } := io.shex("shop.shex")   type    → read types
/// type { P } := io.csv("users.csv")    ERROR: `io.csv` reads rows, not types
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Turn the bytes into rows a plan can scan.
    ReadRows,
    /// Turn the bytes into [`OutputShapes`] — the neutral vocabulary the
    /// compiler reads instead of a schema language.
    ReadTypes,
}

impl Capability {
    /// The verb phrase a diagnostic uses: "reads rows" / "reads types".
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::ReadRows => "reads rows",
            Self::ReadTypes => "reads types",
        }
    }

    /// The bare noun, for the second half of "…, not types".
    #[must_use]
    pub const fn noun(self) -> &'static str {
        match self {
            Self::ReadRows => "rows",
            Self::ReadTypes => "types",
        }
    }
}

/// One row of the registry — one thing that can be written after `io.`.
///
/// # Identity is the row's own address
///
/// The escape hatch for a `&'static` that has to be named INSIDE a query: it is
/// named by a `&'static` whose `Eq`/`Hash` are `ptr::eq`/`ptr::hash`, so the
/// entry's identity is its own address and nothing about its contents.
///
/// `ShapeDecoder`, which this replaces, keyed identity on the address of its
/// `decode` function. That cannot survive the merge and should not: a row that
/// reads rows has no `decode`, so **every data row would have compared equal to
/// every other**. The address of the ROW is the same escape hatch, defined for
/// every row rather than for two thirds of them, and it is strictly finer —
/// two rows that share one `decode` (a `shex` row and a hypothetical `shexc`
/// row) are now two rows, which is what their two names already claimed.
///
/// The consequence, and it is enforced by the type: `Provider` is **not**
/// `Copy` and **not** `Clone`. A copy has a different address and would be a
/// different row. Rows are held as `&'static Provider` and nothing else.
#[derive(Debug)]
pub struct Provider {
    /// What is written after `io.` — `"csv"`, `"shex"`, `"shacl"`. Stable,
    /// lowercase, namespace-free. **This is what dispatch goes by.**
    pub name: &'static str,
    /// The file extensions this row accepts, lowercase and without the dot. The
    /// row checks its own ([`Self::accepts`]) and words its own rejection
    /// ([`Self::decline_extension`]); nothing dispatches on these.
    pub extensions: &'static [&'static str],
    /// The `read rows` capability, or `None` when the row does not have it.
    pub reads_rows: Option<RowReader>,
    /// The `read types` capability, or `None` when the row does not have it.
    pub reads_types: Option<DecodeTypes>,
    // The two `write` cells of ruling 13's table are deliberately absent, not
    // stubbed. `grammar.bnf`'s NOT-IN-MVP list: «`io` names only the INPUT.
    // Where a program writes to has no syntax at all, and inventing one here
    // before it is decided is how v0.1 got its ghosts.» The shape of this
    // struct is what leaves the hole ready — a third capability is a field and
    // a `Capability` variant, and nothing else moves.
}

impl Provider {
    /// `io.<name>` — how a program spells this row.
    #[must_use]
    pub fn constructor(&self) -> String {
        format!("io.{}", self.name)
    }

    /// Does this row declare `wanted`?
    #[must_use]
    pub const fn provides(&self, wanted: Capability) -> bool {
        match wanted {
            Capability::ReadRows => self.reads_rows.is_some(),
            Capability::ReadTypes => self.reads_types.is_some(),
        }
    }

    /// Every capability this row declares, in table order.
    pub fn capabilities(&self) -> impl Iterator<Item = Capability> + '_ {
        [Capability::ReadRows, Capability::ReadTypes]
            .into_iter()
            .filter(|c| self.provides(*c))
    }

    /// Does this row accept `uri`'s extension?
    ///
    /// A URI with no extension is accepted by nobody. The caller decides
    /// whether that is worth a diagnostic — [`Self::decline_extension`] words
    /// one either way.
    #[must_use]
    pub fn accepts(&self, uri: &str) -> bool {
        extension_of(uri).is_some_and(|ext| self.extensions.iter().any(|c| *c == ext))
    }

    /// **The row's own rejection of a capability it does not declare**, naming
    /// both — the row and what was asked of it.
    ///
    /// `type { P } := io.csv("users.csv")` → «`io.csv` reads rows, not types».
    /// `installed` is the whole table, so the message can end by naming the rows
    /// that DO have the capability rather than leaving the reader to guess.
    #[must_use]
    pub fn decline_capability(&self, wanted: Capability, installed: &[&'static Self]) -> String {
        let mine: Vec<&str> = self.capabilities().map(Capability::describe).collect();
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
            [one] => format!(" — {one} {}", wanted.describe()),
            many => format!(" — {} read {}", many.join(", "), wanted.noun()),
        };
        format!(
            "`{}` {}, not {}{}",
            self.constructor(),
            has,
            wanted.noun(),
            tail
        )
    }

    /// **The row's own rejection of an extension it does not accept**, naming
    /// both — the constructor and the extension.
    ///
    /// `io.shex("catalogue.ttl")` → «`io.shex` reads `.shex`, `.shexj` or
    /// `.shexc` documents, and `catalogue.ttl` is `.ttl`».
    #[must_use]
    pub fn decline_extension(&self, uri: &str) -> String {
        let mine = join_or(
            &self
                .extensions
                .iter()
                .map(|e| format!("`.{e}`"))
                .collect::<Vec<_>>(),
        );
        let found = extension_of(uri).map_or_else(
            || format!("`{uri}` has no extension"),
            |ext| format!("`{uri}` is `.{ext}`"),
        );
        format!(
            "`{}` reads {} documents, and {}",
            self.constructor(),
            mine,
            found
        )
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

/// Identity is `ptr::eq` on the row's own address — see [`Provider`].
impl PartialEq for Provider {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

impl Eq for Provider {}

/// `ptr::hash` on the same address, so `Hash` and `Eq` agree.
impl Hash for Provider {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::ptr::hash(self, state);
    }
}

/// The extension of `uri`: the text after the last `.` **of the last path
/// segment**, lowercased and without the dot.
///
/// `"shapes/shop.shex"` is `shex`, `"SHOP.ShEx"` is `shex`, and `"v1.2/shop"`
/// has none rather than `2/shop`.
#[must_use]
pub fn extension_of(uri: &str) -> Option<String> {
    let segment = uri.rsplit(['/', '\\']).next().unwrap_or(uri);
    let (_, ext) = segment.rsplit_once('.')?;
    if ext.is_empty() {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

/// **The one lookup.** The row a program names, by the name it writes after
/// `io.`.
///
/// `constructor` may be written either way — `"io.csv"` or `"csv"` — because
/// the two callers have it in the two forms (`def_map` keeps the dotted callee,
/// a `providers` listing keeps the short name) and a second function would be a
/// second way to say one thing.
#[must_use]
pub fn provider(table: &[&'static Provider], constructor: &str) -> Option<&'static Provider> {
    let name = constructor.strip_prefix("io.").unwrap_or(constructor);
    table.iter().copied().find(|p| p.name == name)
}

// `type_reader_claiming(table, uri)` lived here — the one position with no name
// to dispatch on, `{ A, B } := io.rdf("g.ttl", schema = "x.shex")`, where a
// named argument carried a path and no constructor. It was the last
// extension-based selection in the tree and its own doc said it would go the day
// `schema = …` named a provider.
//
// It has. `schema = io.shex("x.shex")` is a provider call like every other
// position that names a document, so there is ONE rule and no corner:
// **where there is a document, there is a row that names it.** A bare string is
// a diagnostic (`fossil_hir::lower::check_provider`), not a fallback.
//
// The alternative considered was killing `schema =` outright and taking every
// document from a `type { … }` binding. It is not the same document: `schema =`
// shapes the INPUT rows and `type { … }` is the OUTPUT contract, and nothing
// says a program's two ends read one file.

/// `io.csv` — `read_csv_auto`.
pub static CSV: Provider = Provider {
    name: "csv",
    extensions: &["csv"],
    reads_rows: Some(RowReader::Native(NativeReader::CsvAuto)),
    reads_types: None,
};

/// `io.json` — `read_json_auto`.
pub static JSON: Provider = Provider {
    name: "json",
    extensions: &["json"],
    reads_rows: Some(RowReader::Native(NativeReader::JsonAuto)),
    reads_types: None,
};

/// `io.parquet` — `read_parquet`.
pub static PARQUET: Provider = Provider {
    name: "parquet",
    extensions: &["parquet"],
    reads_rows: Some(RowReader::Native(NativeReader::Parquet)),
    reads_types: None,
};

/// `io.rdf` — materialised outside the reader.
///
/// The extension list is kept in lockstep with the RDF provider's own.
pub static RDF: Provider = Provider {
    name: "rdf",
    extensions: &["ttl", "nt", "n3", "rdf"],
    reads_rows: Some(RowReader::Materialised),
    reads_types: None,
};

/// The rows the compiler can name on its own: everything that reads DATA.
///
/// This is [`crate::system::System::providers`]'s default, and it is a real
/// answer rather than a stub — a host that decodes no shape document still has
/// to recognise `io.csv`. A host that compiles programs installs a superset
/// (`fossil_descriptors_output::PROVIDERS`), and the rows it adds are the ones
/// carrying a `fn` into a schema language the compiler may not link.
pub static DATA: &[&Provider] = &[&CSV, &JSON, &PARQUET, &RDF];

#[cfg(test)]
mod tests {
    use super::*;

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

    static TABLE: &[&Provider] = &[&CSV, &JSON, &PARQUET, &RDF, &SHEX, &SHACL];

    /// Identity is the row's address, not its name and not its `decode`.
    /// `SHEX` and `SHACL` share one `decode` here **on purpose**: under the
    /// rule this replaces they would have been one row.
    #[test]
    fn a_row_is_identified_by_its_address_not_by_its_decode() {
        assert_eq!(SHEX.reads_types, SHACL.reads_types, "one `decode`");
        assert_ne!(SHEX, SHACL, "and still two rows");
        assert_eq!(&SHEX, &SHEX);

        let mut set = std::collections::HashSet::new();
        set.insert(&SHEX);
        assert!(set.contains(&&SHEX), "Hash agrees with Eq");
        assert!(!set.contains(&&SHACL));
    }

    /// Dispatch is by NAME, and it takes either spelling.
    #[test]
    fn dispatch_is_by_name() {
        assert_eq!(provider(TABLE, "io.shex").map(|p| p.name), Some("shex"));
        assert_eq!(provider(TABLE, "shex").map(|p| p.name), Some("shex"));
        assert_eq!(provider(TABLE, "io.shacl").map(|p| p.name), Some("shacl"));
        assert!(provider(TABLE, "io.linkml").is_none());
        // The pair that behaved identically until now: same extension, two
        // different rows, and the name is what tells them apart.
        assert!(SHEX.accepts("x.shexc"));
        assert!(!SHEX.accepts("x.ttl"));
        assert!(SHACL.accepts("x.ttl"));
    }

    #[test]
    fn a_row_declares_its_capabilities() {
        assert!(CSV.provides(Capability::ReadRows));
        assert!(!CSV.provides(Capability::ReadTypes));
        assert!(SHEX.provides(Capability::ReadTypes));
        assert!(!SHEX.provides(Capability::ReadRows));
        assert_eq!(
            CSV.capabilities().collect::<Vec<_>>(),
            [Capability::ReadRows]
        );
    }

    /// The message names BOTH — the row and what was asked of it — and then
    /// says who could.
    #[test]
    fn asking_for_a_capability_a_row_lacks_names_both() {
        assert_eq!(
            CSV.decline_capability(Capability::ReadTypes, TABLE),
            "`io.csv` reads rows, not types — `io.shex`, `io.shacl` read types"
        );
        assert_eq!(
            SHEX.decline_capability(Capability::ReadRows, TABLE),
            "`io.shex` reads types, not rows — `io.csv`, `io.json`, `io.parquet`, \
             `io.rdf` read rows"
        );
        // One candidate takes the singular, because a message that reads like a
        // typo is a message a reader distrusts.
        assert_eq!(
            CSV.decline_capability(Capability::ReadTypes, &[&CSV, &SHEX]),
            "`io.csv` reads rows, not types — `io.shex` reads types"
        );
    }

    /// The row words its own extension rejection, naming the constructor and
    /// the extension — `io.shex("catalogue.ttl")` is the case ruling 13 leads
    /// with.
    #[test]
    fn a_row_declines_an_extension_in_its_own_words() {
        assert_eq!(
            SHEX.decline_extension("catalogue.ttl"),
            "`io.shex` reads `.shex`, `.shexj` or `.shexc` documents, and \
             `catalogue.ttl` is `.ttl`"
        );
        assert_eq!(
            CSV.decline_extension("users"),
            "`io.csv` reads `.csv` documents, and `users` has no extension"
        );
    }

    #[test]
    fn the_extension_is_the_last_segments_last_dot() {
        assert_eq!(extension_of("shapes/shop.shex").as_deref(), Some("shex"));
        assert_eq!(extension_of("SHOP.ShEx").as_deref(), Some("shex"));
        assert_eq!(extension_of("v1.2/shop"), None);
        assert_eq!(extension_of("trailing."), None);
        assert_eq!(extension_of(""), None);
    }

    /// `.ttl` is claimed by `io.rdf` (rows) and `io.shacl` (types). Under the
    /// extension-based selection this replaces, the answer depended on which
    /// question was being asked and on table order; under dispatch by name there
    /// is no question to ask — the program said which.
    #[test]
    fn one_extension_two_rows_and_the_name_is_the_only_thing_that_separates_them() {
        assert!(RDF.accepts("g.ttl") && SHACL.accepts("g.ttl"));
        assert!(RDF.provides(Capability::ReadRows));
        assert!(SHACL.provides(Capability::ReadTypes));
        assert_eq!(provider(TABLE, "io.rdf"), Some(&RDF));
        assert_eq!(provider(TABLE, "io.shacl"), Some(&SHACL));
    }

    /// The default table recognises every data constructor and nothing else.
    #[test]
    fn the_default_table_is_the_data_rows() {
        assert_eq!(DATA.len(), 4);
        assert!(DATA.iter().all(|p| p.provides(Capability::ReadRows)));
        assert!(DATA.iter().all(|p| !p.provides(Capability::ReadTypes)));
        assert!(provider(DATA, "io.shex").is_none());
    }
}
