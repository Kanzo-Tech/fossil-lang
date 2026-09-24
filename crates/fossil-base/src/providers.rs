//! **The provider registry — one table.** A row is the name written after
//! `io.`, the extensions it accepts, and the capabilities it declares.
//!
//! **This is the ONLY table, and dispatch is always by name.** The row checks
//! its own extension ([`Provider::accepts`]) and declares its own capabilities
//! ([`Provider::provides`]). Dispatching by extension instead makes the name
//! decorative and `io.shex("x.ttl")` and `io.shacl("x.ttl")` indistinguishable;
//! a second table alongside this one is the same bug with more steps. The row
//! does not word the refusal — that is a compiler diagnostic, and it lives with
//! the checker that raises it, in `fossil_hir::refusals`.
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
//! link one — `0e6898d` cut `ShEx` out of `fossil-mir` and the same cut applies to
//! `fossil-hir`. So the rows that read rows are `&'static` constants in this
//! crate (the compiler can name a `DuckDB` reader), the rows that read types come
//! from the crate that owns the parser, and [`crate::system::System::providers`]
//! is where a host hands over the assembled table. That split is what decides
//! which of the two generated files a row lands in — see below.
//!
//! This is a table of *behaviour*, and behaviour is what a table of `fn` is for:
//! the extension point is a table of `fn`, never a trait object, because a `fn`
//! pointer gives the `Eq`/`Hash` a memoized core needs and a `dyn` does not. As
//! against [`crate::system::System::descriptors`], which is a table of *data* and
//! therefore has no `fn` in it at all — there is nothing there to dispatch, and a
//! function pointer returning descriptors would be the indirection without the
//! reason.
//!
//! # It is data, and it is now only data
//!
//! Nothing here decides anything about a program. The catalogue is a name, the
//! extensions it accepts, the capabilities it declares, and two predicates over
//! those; composing a Fossil compiler error out of it is `fossil_hir::refusals`,
//! next to the checker that raises it.
//!
//! That is not left to a comment: `tests/substrate_has_no_prose.rs` reads this
//! file **and the module beside it** and fails on a string literal outside `mod tests`
//! with a space in it. Every literal a catalogue needs — `"csv"`, `"ttl"`,
//! `"io.{}"` — is one token; a sentence is not. It is a crude rule and it says
//! so, but it is the rule that would have caught the two functions that just
//! left.
//!
//! # The rows themselves are not written here any more
//!
//! `CSV`, `JSON`, `PARQUET`, `RDF`, `DATA` and [`NativeReader`] are **generated
//! from `catalogue.bnf`** into `providers/generated.rs`, by `cargo xtask
//! catalogue`. What is left in this file is the row TYPE, the two predicates
//! over it, the one lookup, and the Salsa input a host installs through — the
//! machinery, none of which a data file can carry.
//!
//! `catalogue.bnf` is the source of truth for the DATA half, and the statics are
//! GENERATED from it rather than compared against it — so a row's `decodes`
//! token is a Rust path the compiler resolves, which no comparison could check.

use std::hash::{Hash, Hasher};
use std::sync::OnceLock;

use fossil_graph_schema::{OutputShapes, Rejection};

mod generated;

pub use generated::{CSV, DATA, JSON, NativeReader, PARQUET, RDF};

/// `(uri_as_written, text) -> decoded`.
///
/// **Pure**: no IO, no network, no clock. It is called from inside a tracked
/// query, where an untracked read of the outside world is exactly the bug
/// `fossil_hir::shape_documents::shape_document` exists to remove. The `uri` is
/// passed because a document can carry relative references and a diagnostic
/// wants to name the file — not so the decoder can go and read it.
pub type DecodeTypes = fn(&str, &str) -> Result<OutputShapes, Rejection>;

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
    /// row checks its own ([`Self::accepts`]); nothing dispatches on these, and
    /// nothing here words the refusal — `fossil_hir::refusals` does.
    pub extensions: &'static [&'static str],
    /// The `read rows` capability, or `None` when the row does not have it.
    pub reads_rows: Option<RowReader>,
    /// The `read types` capability, or `None` when the row does not have it.
    pub reads_types: Option<DecodeTypes>,
    // The two `write` cells of the direction table above are absent, not
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
    /// A URI with no extension is accepted by nobody. The caller decides whether
    /// that is worth a diagnostic, and words one — `fossil_hir::refusals` has
    /// the three sentences.
    #[must_use]
    pub fn accepts(&self, uri: &str) -> bool {
        extension_of(uri).is_some_and(|ext| self.extensions.iter().any(|c| *c == ext))
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

/// Is `uri` claimed by any row in `table` — is it a file something READS?
///
/// **This is not a selection**, which is the whole reason it can exist beside
/// the tombstone below. It answers `bool` and names no row, so `.ttl` being
/// claimed by both `io.rdf` and `io.shacl` is not a question with a wrong
/// answer: every row is asked [`Provider::accepts`], and one `true` is enough.
/// A URI with no extension is claimed by nobody.
///
/// The conclusion drawn from `true` belongs to the caller and is worded there.
/// A HOST is what has a use for this: it holds buffers somebody opened and
/// nothing in them says what they are, and a file the installed table claims is
/// an INPUT — bytes a program reads through `io.…` — rather than a program.
#[must_use]
pub fn claimed(table: &[&'static Provider], uri: &str) -> bool {
    table.iter().any(|p| p.accepts(uri))
}

// Nothing selects a row by extension. `schema = io.shex("x.shex")` is a
// provider call like every other position that names a document, so there is ONE
// rule and no corner: **where there is a document, there is a row that names
// it.** A bare string is a diagnostic (`fossil_hir::lower::check_provider`), not
// a fallback.
//
// The alternative considered was killing `schema =` outright and taking every
// document from a `type { … }` binding. It is not the same document: `schema =`
// shapes the INPUT rows and `type { … }` is the OUTPUT contract, and nothing
// says a program's two ends read one file.

// The four data rows and `DATA` stood here as hand-written statics. They are
// generated from `catalogue.bnf` now — `mod generated`, above — and the argument
// each one's doc comment carried moved into that file's `(* … *)` commentary,
// which is where `catalogue.bnf` says the argument lives.

// ===================================================================== the input

/// The rows installed in this database.
///
/// **An input, and every query reads it through here.** Reading the rows
/// straight off `System::providers()` instead makes them a `&'static` that
/// cannot come from a file read at run time and cannot be a Salsa input, so the
/// catalogue could not be declarative and nothing would invalidate when it
/// changed.
#[salsa::input(debug)]
pub struct Registry {
    #[returns(ref)]
    pub rows: Vec<&'static Provider>,
}

/// The handle the database holds — the same shape as [`crate::files::Files`],
/// for the same reason: a Salsa input can only be created with a database in
/// hand, and `Default` has none.
#[derive(Debug, Default, Clone)]
pub struct Catalogue {
    registry: OnceLock<Registry>,
}

impl Catalogue {
    /// The registry input, allocating it on first use **from the database's own
    /// host** — `db.system().providers()`.
    ///
    /// Seeding from the host here rather than at each constructor is not
    /// convenience. A `Db` implementation that forgot to seed would get [`DATA`]
    /// and silently lose the rows that read TYPES, so every shape document in
    /// that session would decode to nothing — a wrong answer, not an error.
    /// Taking `&dyn Db` instead of `&dyn salsa::Database` is what makes
    /// forgetting unspellable.
    ///
    /// Force it from the database constructor anyway. Salsa 0.26 lets an input
    /// be created while a query runs and nothing checks, but the created input
    /// is invisible to that query's dependency list; [`install`] needs `&mut`,
    /// which no query body can have, so "install before you query" is the only
    /// expressible order.
    pub fn registry(&self, db: &dyn crate::db::Db) -> Registry {
        *self
            .registry
            .get_or_init(|| Registry::new(db, db.system().providers().to_vec()))
    }
}

/// Every row a program may name after `io.`.
///
/// **The read registers a Salsa dependency, and that is the point.** A miss
/// here is not a cached dead end: it is a dependency on the catalogue's
/// contents, so [`install`] invalidates the queries that previously refused a
/// name.
#[must_use]
pub fn installed(db: &dyn crate::db::Db) -> &[&'static Provider] {
    db.catalogue().registry(db).rows(db)
}

/// Install the rows this host recognises, replacing whatever was there.
///
/// Takes `&mut dyn Db` because writing a Salsa input takes the database
/// exclusively.
pub fn install(db: &mut dyn crate::db::Db, rows: Vec<&'static Provider>) {
    use salsa::Setter as _;

    let registry = db.catalogue().registry(&*db);
    registry.set_rows(db).to(rows);
}

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
        // Never `assert_eq!` on the two `Option<fn>`: two `fn` pointers can
        // differ across codegen units and two distinct functions can be merged
        // to one address. `std::ptr::fn_addr_eq` is the spelling that admits it.
        assert!(
            std::ptr::fn_addr_eq(
                SHEX.reads_types.expect("shex reads types"),
                SHACL.reads_types.expect("shacl reads types"),
            ),
            "one `decode`"
        );
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

    /// The question a host has about a buffer nobody described: is this a file
    /// something READS, or is it a program?
    ///
    /// The table already answers it, and the answer is one `bool` over rows
    /// that disagree about which row it is — `.ttl` is claimed twice here and
    /// [`claimed`] does not care, because it selects nothing.
    #[test]
    fn the_table_says_which_uris_it_claims() {
        assert!(claimed(TABLE, "person.shex"), "a shape document");
        assert!(claimed(TABLE, "shapes/SHOP.ShExJ"), "case and path folded");
        assert!(claimed(TABLE, "users.csv"), "a data file is read too");
        assert!(claimed(TABLE, "g.ttl"), "claimed by io.rdf AND io.shacl");

        assert!(!claimed(TABLE, "hello.fossil"), "no row reads a program");
        assert!(!claimed(TABLE, "README"), "no extension, no claim");
        assert!(
            !claimed(DATA, "person.shex"),
            "the DEFAULT table has no shape row, so a host that installed it \
             claims no `.shex` — which is the honest answer for a host that \
             cannot decode one"
        );
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
