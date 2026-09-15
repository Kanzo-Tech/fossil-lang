//! `corpus.bnf` — parse, and every projection of it.
//!
//! The sibling of [`crate::catalogue`], one data file along. That module turns
//! *which names a program may write* into six files; this one turns *which
//! columns the writer emits* into two — a Rust table for the crates and a
//! TypeScript one for the packages — so that the set stops being stated by hand
//! in three places with two different contents.
//!
//! # Why a parser here and a lexer from next door
//!
//! `catalogue`'s `lex` is shared: the comment rule, the string escapes and
//! the dotted-word rule are the parts that would drift between two scanners, and
//! both files are written in the same dialect. The statement parser is not
//! shared, because `catalogue.bnf`'s is a signature grammar and this one is four
//! clauses — and because a panic from a parser should name the file the reader
//! has open.
//!
//! # What a role is for
//!
//! A [`Role`] says what a column IS. Every reader that used to carry a
//! hand-written list now names the roles it means, which is what makes the two
//! surviving sets legibly different rather than accidentally different:
//! `fossil-graph` hides *everything the writer emits* from a field listing, and
//! `packages/corpus` excludes only *the columns it already surfaces as named
//! members*. Those are different questions and they were two literals.

use std::fmt::Write as _;
use std::path::PathBuf;

use crate::catalogue::{Tok, lex, repo_root, rustfmt};

/// Which artefact a column belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Where {
    /// A vertex payload row.
    Payload,
    /// An adjacency row, in either orientation.
    Adjacency,
}

impl Where {
    fn parse(word: &str) -> Option<Self> {
        match word {
            "payload" => Some(Self::Payload),
            "adjacency" => Some(Self::Adjacency),
            _ => None,
        }
    }

    /// The Rust identifier fragment this artefact contributes to a const name.
    const fn upper(self) -> &'static str {
        match self {
            Self::Payload => "PAYLOAD",
            Self::Adjacency => "ADJACENCY",
        }
    }
}

/// What a column is — never what a reader does with it. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// The row's rank in the ordering. Not an identity: a re-layout renumbers it.
    Address,
    /// What a bookmark keys on, and what survives a rebuild.
    Identity,
    /// One axis of the plane. Declared and drawn as a pair.
    Coordinate,
    /// An ordinal the writer computed, for a reader to colour by.
    Categorical,
    /// One end of a relation, in the aligned type's `dense_id` space.
    Endpoint,
}

impl Role {
    fn parse(word: &str) -> Option<Self> {
        match word {
            "address" => Some(Self::Address),
            "identity" => Some(Self::Identity),
            "coordinate" => Some(Self::Coordinate),
            "categorical" => Some(Self::Categorical),
            "endpoint" => Some(Self::Endpoint),
            _ => None,
        }
    }

    /// The Rust variant name, which is also the TypeScript string literal.
    const fn variant(self) -> &'static str {
        match self {
            Self::Address => "Address",
            Self::Identity => "Identity",
            Self::Coordinate => "Coordinate",
            Self::Categorical => "Categorical",
            Self::Endpoint => "Endpoint",
        }
    }

    /// The `SCREAMING` fragment a per-role const is named with.
    const fn upper(self) -> &'static str {
        match self {
            Self::Address => "ADDRESS",
            Self::Identity => "IDENTITY",
            Self::Coordinate => "COORDINATES",
            Self::Categorical => "CATEGORICAL",
            Self::Endpoint => "ENDPOINTS",
        }
    }
}

/// One `col` statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub whence: Where,
    /// The `GraphAr` spelling, which is what the manifest writes.
    pub data_type: String,
    pub role: Role,
    /// Whether the manifest's `properties:` also declares it. Exactly one does.
    pub declared: bool,
    /// `src` or `dst` on an endpoint, and `None` elsewhere.
    pub aligns: Option<String>,
}

/// Parse `corpus.bnf`.
///
/// Every statement is `col <name> = <clause> ; … .` and anything outside one is
/// commentary the lexer drops. A malformed statement is a panic rather than a
/// skip, for [`crate::catalogue::parse`]'s reason: this is a source of truth, so
/// a line that does not parse is a build failure and not a column that quietly
/// stops existing.
///
/// # Panics
///
/// On any statement that is not a well-formed `col`, on an unknown artefact or
/// role, and on a file that declares no columns at all.
#[must_use]
pub fn parse(text: &str) -> Vec<Column> {
    let toks = lex(text);
    let mut pos = 0usize;
    let mut out = Vec::new();

    let word = |pos: &mut usize| -> String {
        match toks.get(*pos) {
            Some(Tok::Word(w)) => {
                *pos += 1;
                w.clone()
            }
            other => panic!("corpus.bnf: expected a word, found {other:?}"),
        }
    };
    let punct = |pos: &mut usize, p: &str| {
        match toks.get(*pos) {
            Some(Tok::Punct(q)) if *q == p => *pos += 1,
            other => panic!("corpus.bnf: expected `{p}`, found {other:?}"),
        };
    };

    while pos < toks.len() {
        let head = word(&mut pos);
        assert_eq!(head, "col", "corpus.bnf: expected `col`, found `{head}`");
        let name = word(&mut pos);
        punct(&mut pos, "=");

        let mut whence = None;
        let mut data_type = None;
        let mut role = None;
        let mut declared = false;
        let mut aligns = None;

        loop {
            let clause = word(&mut pos);
            match clause.as_str() {
                "in" => {
                    let w = word(&mut pos);
                    whence = Some(
                        Where::parse(&w)
                            .unwrap_or_else(|| panic!("corpus.bnf: `{w}` names no artefact")),
                    );
                }
                "type" => data_type = Some(word(&mut pos)),
                "role" => {
                    let r = word(&mut pos);
                    role = Some(
                        Role::parse(&r)
                            .unwrap_or_else(|| panic!("corpus.bnf: `{r}` names no role")),
                    );
                }
                "declared" => declared = true,
                "aligns" => aligns = Some(word(&mut pos)),
                other => panic!("corpus.bnf: `{other}` is not a clause of `col {name}`"),
            }
            match toks.get(pos) {
                Some(Tok::Punct(";")) => pos += 1,
                Some(Tok::Punct(".")) => {
                    pos += 1;
                    break;
                }
                other => panic!("corpus.bnf: expected `;` or `.`, found {other:?}"),
            }
        }

        let column = Column {
            whence: whence.unwrap_or_else(|| panic!("corpus.bnf: `col {name}` declares no `in`")),
            data_type: data_type
                .unwrap_or_else(|| panic!("corpus.bnf: `col {name}` declares no `type`")),
            role: role.unwrap_or_else(|| panic!("corpus.bnf: `col {name}` declares no `role`")),
            declared,
            aligns,
            name,
        };
        assert!(
            (column.role == Role::Endpoint) == column.aligns.is_some(),
            "corpus.bnf: `col {}` — `aligns` belongs to an endpoint and to nothing else",
            column.name,
        );
        out.push(column);
    }

    assert!(!out.is_empty(), "corpus.bnf: declares no columns");
    out
}

/// Columns of one artefact, in file order.
fn of(columns: &[Column], whence: Where) -> Vec<&Column> {
    columns.iter().filter(|c| c.whence == whence).collect()
}

/// A `&[&str]` literal of the names a predicate keeps, in file order.
fn names(columns: &[&Column], keep: impl Fn(&Column) -> bool) -> String {
    let kept: Vec<String> = columns
        .iter()
        .filter(|c| keep(c))
        .map(|c| format!("{:?}", c.name))
        .collect();
    format!("&[{}]", kept.join(", "))
}

const RUST_HEADER: &str = "\
//! @generated by `cargo xtask corpus` from `corpus.bnf`. DO NOT EDIT.
//!
//! The columns the writer emits, and what each one IS. `corpus.bnf` carries the
//! rows and the argument; this is the Rust projection of them.
//!
//! **Read a set by the ROLES it means, not by a name you remember.** The two
//! consumers that used to carry hand-written lists meant different things by
//! \"the writer's columns\" and nothing said so — see `corpus.bnf`.
";

/// The Rust table: `crates/fossil-sinks/src/generated.rs`.
///
/// It lands in `fossil-sinks` because that crate is the junta — the manifest
/// model, in the closure of both the writer and the reader — so every Rust
/// consumer already depends on it and none grows an edge to reach this.
#[must_use]
pub fn emit_rust(columns: &[Column]) -> String {
    let mut out = String::from(RUST_HEADER);

    out.push_str(
        "\n/// What a column IS — never what a reader does with it.\n\
         ///\n\
         /// A reader that used to carry a list now names the roles it means.\n\
         #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]\n\
         pub enum ColumnRole {\n",
    );
    let mut seen: Vec<Role> = columns.iter().map(|c| c.role).collect();
    seen.sort_unstable();
    seen.dedup();
    for role in &seen {
        // No backticks around the sentence: `role_doc` is prose, and two of the
        // five already contain an inline span of their own.
        let _ = writeln!(out, "    /// {}.", capitalise(role_doc(*role)));
        let _ = writeln!(out, "    {},", role.variant());
    }
    out.push_str("}\n");

    out.push_str(
        "\n/// One column the writer emits.\n\
         #[derive(Debug, Clone, Copy, PartialEq, Eq)]\n\
         pub struct WriterColumn {\n\
         \x20   /// The column name, as it appears in the Parquet schema.\n\
         \x20   pub name: &'static str,\n\
         \x20   /// The `GraphAr` spelling of its type, which is what the manifest writes.\n\
         \x20   pub data_type: &'static str,\n\
         \x20   pub role: ColumnRole,\n\
         \x20   /// Whether the manifest's `properties:` also declares it.\n\
         \x20   pub declared: bool,\n\
         }\n",
    );

    for whence in [Where::Payload, Where::Adjacency] {
        let cols = of(columns, whence);
        let what = match whence {
            Where::Payload => "a vertex payload row",
            Where::Adjacency => "an adjacency row, in either orientation",
        };
        let _ = writeln!(
            out,
            "\n/// Every column of {what}, in writer order.\n\
             pub const {}_COLUMNS: &[WriterColumn] = &[",
            whence.upper()
        );
        for c in &cols {
            let _ = writeln!(
                out,
                "    WriterColumn {{ name: {:?}, data_type: {:?}, role: ColumnRole::{}, declared: {} }},",
                c.name,
                c.data_type,
                c.role.variant(),
                c.declared
            );
        }
        out.push_str("];\n");

        let _ = writeln!(
            out,
            "\n/// The names of {}, in writer order.\npub const {}_NAMES: &[&str] = {};",
            what,
            whence.upper(),
            names(&cols, |_| true)
        );

        let mut roles: Vec<Role> = cols.iter().map(|c| c.role).collect();
        roles.sort_unstable();
        roles.dedup();
        for role in roles {
            let _ = writeln!(
                out,
                "\n/// The {} column(s) of {what} — {}.\npub const {}_{}: &[&str] = {};",
                role.variant().to_lowercase(),
                role_doc(role),
                whence.upper(),
                role.upper(),
                names(&cols, |c| c.role == role)
            );
        }
    }

    let _ = writeln!(
        out,
        "\n/// The payload columns the manifest's `properties:` also declares.\n\
         ///\n\
         /// One of five, and that asymmetry is the whole of the manifest-versus-bytes\n\
         /// difference a reader has to know about.\n\
         pub const PAYLOAD_DECLARED: &[&str] = {};",
        names(&of(columns, Where::Payload), |c| c.declared)
    );

    for c in columns.iter().filter(|c| c.aligns.is_some()) {
        let _ = writeln!(
            out,
            "\n/// The endpoint column a `{}`-aligned orientation is tiled by.\n\
             pub const ALIGNED_{}: &str = {:?};",
            c.aligns.as_deref().unwrap_or_default(),
            c.aligns.as_deref().unwrap_or_default().to_uppercase(),
            c.name
        );
    }

    rustfmt(&out)
}

/// Sentence case, for a doc comment that starts with this line.
fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
        .unwrap_or_default()
}

/// One line of prose per role, used in both generated files so the two agree.
const fn role_doc(role: Role) -> &'static str {
    match role {
        Role::Address => "the row's rank in the ordering, which a re-layout renumbers",
        Role::Identity => "what a bookmark keys on, and what survives a rebuild",
        Role::Coordinate => "one axis of the plane",
        Role::Categorical => "an ordinal the writer computed, for a reader to colour by",
        Role::Endpoint => "one end of a relation, in the aligned type's `dense_id` space",
    }
}

/// The TypeScript table: `packages/corpus/src/vocabulary.generated.ts`.
#[must_use]
pub fn emit_ts(columns: &[Column]) -> String {
    let mut out = String::from(
        "// @generated by `cargo xtask corpus` from `corpus.bnf`. DO NOT EDIT.\n\
         //\n\
         // The columns the writer emits, and what each one IS. `corpus.bnf` carries the\n\
         // rows and the argument; this is the TypeScript projection of them, and it is\n\
         // the same table `crates/fossil-sinks/src/generated.rs` carries on the Rust side.\n\
         //\n\
         // Read a set by the ROLES it means. The two consumers that used to carry\n\
         // hand-written lists meant different things by \"the writer's columns\".\n\n",
    );

    let mut seen: Vec<Role> = columns.iter().map(|c| c.role).collect();
    seen.sort_unstable();
    seen.dedup();
    out.push_str(
        "/** What a column IS — never what a reader does with it. */\nexport type ColumnRole =\n",
    );
    for role in &seen {
        let _ = writeln!(out, "  /** {}. */", capitalise(role_doc(*role)));
        let _ = writeln!(out, "  | '{}'", role.variant().to_lowercase());
    }
    out.push_str(";\n\n");

    out.push_str(
        "/** One column the writer emits. */\n\
         export interface WriterColumn {\n\
         \x20 readonly name: string;\n\
         \x20 /** The GraphAr spelling of its type, which is what the manifest writes. */\n\
         \x20 readonly dataType: string;\n\
         \x20 readonly role: ColumnRole;\n\
         \x20 /** Whether the manifest's `properties:` also declares it. */\n\
         \x20 readonly declared: boolean;\n\
         }\n",
    );

    for whence in [Where::Payload, Where::Adjacency] {
        let cols = of(columns, whence);
        let what = match whence {
            Where::Payload => "a vertex payload row",
            Where::Adjacency => "an adjacency row, in either orientation",
        };
        let ident = match whence {
            Where::Payload => "PAYLOAD",
            Where::Adjacency => "ADJACENCY",
        };
        let _ = writeln!(
            out,
            "\n/** Every column of {what}, in writer order. */\nexport const {ident}_COLUMNS: readonly WriterColumn[] = ["
        );
        for c in &cols {
            let _ = writeln!(
                out,
                "  {{ name: '{}', dataType: '{}', role: '{}', declared: {} }},",
                c.name,
                c.data_type,
                c.role.variant().to_lowercase(),
                c.declared
            );
        }
        out.push_str("];\n");

        let _ = writeln!(
            out,
            "\n/** The names of {what}, in writer order. */\nexport const {ident}_NAMES: readonly string[] = {};",
            ts_names(&cols, |_| true)
        );

        let mut roles: Vec<Role> = cols.iter().map(|c| c.role).collect();
        roles.sort_unstable();
        roles.dedup();
        for role in roles {
            let _ = writeln!(
                out,
                "\n/** The {} column(s) of {what} — {}. */\nexport const {ident}_{}: readonly string[] = {};",
                role.variant().to_lowercase(),
                role_doc(role),
                role.upper(),
                ts_names(&cols, |c| c.role == role)
            );
        }
    }

    let _ = writeln!(
        out,
        "\n/**\n\
         \x20* The payload columns the manifest's `properties:` also declares.\n\
         \x20*\n\
         \x20* One of five, and that asymmetry is the whole of the manifest-versus-bytes\n\
         \x20* difference a reader has to know about.\n\
         \x20*/\nexport const PAYLOAD_DECLARED: readonly string[] = {};",
        ts_names(&of(columns, Where::Payload), |c| c.declared)
    );

    out
}

/// A TypeScript array literal of the names a predicate keeps, in file order.
fn ts_names(columns: &[&Column], keep: impl Fn(&Column) -> bool) -> String {
    let kept: Vec<String> = columns
        .iter()
        .filter(|c| keep(c))
        .map(|c| format!("'{}'", c.name))
        .collect();
    format!("[{}]", kept.join(", "))
}

/// Read `corpus.bnf` from the repository root.
///
/// # Panics
///
/// If the file cannot be read, which means the repository is not where
/// [`repo_root`] says it is.
#[must_use]
pub fn read() -> Vec<Column> {
    let path = repo_root().join("corpus.bnf");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    parse(&text)
}

/// Every file generated from `corpus.bnf`, as `(absolute path, contents)`.
///
/// The list `--check` and `crates/xtask/tests/corpus_generated.rs` both walk. A
/// target added to an emitter and forgotten here is invisible to both, exactly
/// as it is on the catalogue side.
#[must_use]
pub fn generated() -> Vec<(PathBuf, String)> {
    let columns = read();
    let root = repo_root();
    vec![
        (
            root.join("crates/fossil-sinks/src/generated.rs"),
            emit_rust(&columns),
        ),
        (
            root.join("packages/corpus/src/vocabulary.generated.ts"),
            emit_ts(&columns),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_column_parses_every_clause() {
        let cols = parse("col x = in payload ; type float32 ; role coordinate .");
        assert_eq!(cols.len(), 1);
        assert_eq!(cols[0].name, "x");
        assert_eq!(cols[0].whence, Where::Payload);
        assert_eq!(cols[0].data_type, "float32");
        assert_eq!(cols[0].role, Role::Coordinate);
        assert!(!cols[0].declared);
        assert!(cols[0].aligns.is_none());
    }

    #[test]
    fn declared_and_aligns_are_flags_not_values() {
        let cols = parse(
            "col subject = in payload ; type string ; role identity ; declared .\n\
             col src_dense = in adjacency ; type uint32 ; role endpoint ; aligns src .",
        );
        assert!(cols[0].declared);
        assert_eq!(cols[1].aligns.as_deref(), Some("src"));
    }

    /// The one cross-clause rule, and it is checked rather than documented:
    /// `aligns` says which orientation tiles by a column, so it belongs to an
    /// endpoint and to nothing else.
    #[test]
    #[should_panic(expected = "belongs to an endpoint")]
    fn aligns_on_a_non_endpoint_is_refused() {
        let _ = parse("col x = in payload ; type float32 ; role coordinate ; aligns src .");
    }

    #[test]
    #[should_panic(expected = "names no role")]
    fn an_unknown_role_is_refused() {
        let _ = parse("col x = in payload ; type float32 ; role decoration .");
    }

    /// The real file, so a clause added to it without an emitter is caught here
    /// rather than in a generated file nobody reads.
    #[test]
    fn the_real_file_declares_exactly_one_address_and_one_identity() {
        let cols = read();
        let count = |r: Role| cols.iter().filter(|c| c.role == r).count();
        assert_eq!(count(Role::Address), 1);
        assert_eq!(count(Role::Identity), 1);
        assert_eq!(count(Role::Coordinate), 2);
        assert_eq!(cols.iter().filter(|c| c.declared).count(), 1);
    }
}
