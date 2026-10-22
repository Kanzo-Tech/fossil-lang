//! **The program a model implies** — what a host decided a graph holds, as data, and the one
//! Fossil program that maps it, in one canonical layout.
//!
//! A host that has decided what a graph holds — the sources it reads, the types over them, each
//! identified by some of its columns, each field from one column, and the relations between them —
//! passes that [`Model`], and [`write()`] answers the program: the types declared under the
//! workspace's IRIs, the sources bound, one mapping per type and one more per relation read across a
//! link file. Everything derived from a model — an IRI, a `@subject`, a quoted column, a join — is
//! fossil's, because it is knowledge of the language; the decisions are the host's.
//!
//! What `write` is NOT is an oracle. Its output is a program like any other, and the checker reads
//! it: a field whose primitive is not its column's, or a relation to a type with no key, is said at
//! its span, as it would be in a program a person wrote.

use std::fmt::Write as _;

use fossil_graph_schema::Primitive;
use fossil_syntax::name::{escape, spell};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What a host has decided a graph holds: the sources it reads, the types over them and the
/// relations between them. `write` makes it a program.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Model {
    /// The files the program binds, in the order it binds them.
    pub sources: Vec<ModelSource>,
    pub types: Vec<ModelType>,
    pub edges: Vec<ModelEdge>,
}

/// `<name> := io.<format>("<reference>")`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelSource {
    /// The binding every reference to it is qualified by: one `IDENT`.
    pub name: String,
    /// The constructor that reads it: `csv`, `parquet`, `json`.
    pub format: String,
    pub reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delimiter: Option<String>,
}

/// A type of a [`Model`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelType {
    pub name: String,
    /// The binding of the source its rows are read from.
    pub source: String,
    /// The columns of `source` that identify one: a key, or a link file's two ends.
    pub identity: Vec<String>,
    pub fields: Vec<ModelField>,
}

/// A column of a source, kept as a field of a [`ModelType`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelField {
    /// The name the program writes: one `IDENT`.
    pub name: String,
    /// The column of the type's source it is read from, as the source names it.
    pub column: String,
    pub primitive: Primitive,
    /// Some rows have none: the field is `Option<…>`.
    #[serde(default)]
    pub optional: bool,
}

/// A relation of a [`Model`], from one type to another: a column holding the `to`'s key, on the
/// `from`'s own source, or on a link file joined to it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ModelEdge {
    /// The field of `from` that holds it.
    pub name: String,
    pub from: String,
    pub to: String,
    /// The binding of the source that holds the `to`'s key: the `from`'s own, or a link file's.
    pub on: String,
    /// The column of `on` that holds the `to`'s key.
    pub column: String,
    /// On a link file, its column that holds the `from`'s key: `on` is joined to the `from`'s
    /// source by it, in a mapping of its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub join: Option<String>,
    /// Zero or more per `from`: `Seq<Ref<…>>`.
    #[serde(default)]
    pub many: bool,
    /// Some `from` has none: `Option<Ref<…>>`.
    #[serde(default)]
    pub optional: bool,
}

/// `base` with exactly one trailing `/`: the root every IRI of a workspace is under.
fn root(base: &str) -> String {
    if base.ends_with('/') {
        base.to_string()
    } else {
        format!("{base}/")
    }
}

/// How a name in the data is said as words: `birth_date` is `birth date` and `LaureateAward` is
/// `laureate award`. A boundary is a lower-case letter or digit before a capital, and a run of
/// `_`, `-` and `.` is one space.
fn spoken(name: &str) -> String {
    let mut spaced = String::with_capacity(name.len() + 4);
    let mut previous = '\0';
    for c in name.chars() {
        if c.is_ascii_uppercase() && (previous.is_ascii_lowercase() || previous.is_ascii_digit()) {
            spaced.push(' ');
        }
        spaced.push(c);
        previous = c;
    }
    let mut words = String::with_capacity(spaced.len());
    let mut in_run = false;
    for c in spaced.chars() {
        if matches!(c, '_' | '-' | '.') {
            if !in_run {
                words.push(' ');
            }
            in_run = true;
        } else {
            in_run = false;
            words.push(c);
        }
    }
    words.trim().to_lowercase()
}

/// `Source.column`, the column spelled as the lexer has it: `Source."a.column"` when it is not
/// a name (`fossil_syntax::name::spell`).
fn column(source: &str, column: &str) -> String {
    format!("{source}.{}", spell(column))
}

/// A type's `@subject`: its things' IRI prefix under `base`, and the columns of its identity as
/// holes — `https://acme.example/id/laureate/{laureates.id}`. The prefix is the workspace's and
/// the type's, never the mapping's author's.
fn subject(ty: &ModelType, base: &str) -> String {
    let holes: Vec<String> = ty
        .identity
        .iter()
        .map(|c| format!("{{{}}}", column(&ty.source, c)))
        .collect();
    format!(
        "{}id/{}/{}",
        escape(&root(base)),
        escape(&spoken(&ty.name).replace(' ', "-")),
        holes.join("-")
    )
}

/// A relation's type as the checker prints it: `many` wins over `optional`, as a sequence is
/// already empty.
fn relation(edge: &ModelEdge) -> String {
    let to = format!("Ref<{}>", edge.to);
    match (edge.many, edge.optional) {
        (true, _) => format!("Seq<{to}>"),
        (false, true) => format!("Option<{to}>"),
        (false, false) => to,
    }
}

/// `Award` → `Award`, `affiliation` → `Affiliation`: a mapping's name from a type's and an edge's.
fn upper(name: &str) -> String {
    let mut chars = name.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The program `model` implies, every IRI under `base`: each type declared — its IRI and each
/// field's `<base>/vocab/<name>`, a field's type the checker's own spelling of its primitive — then
/// each source bound, then one mapping per type, with its `@subject`, each field from its column
/// and each relation read on its own source; then one more per relation read across a link file,
/// joined to it on the type's first identity column, writing again what the type requires: its
/// fields that are not optional and the relations it holds exactly one of.
///
/// Deterministic: the same model writes the same text.
#[must_use]
pub fn write(model: &Model, base: &str) -> String {
    let vocab = format!("{}vocab/", root(base));
    let mut out = String::new();

    for t in &model.types {
        let _ = writeln!(
            out,
            "@iri(\"{}{}\")\ntype {}",
            escape(&vocab),
            t.name,
            t.name
        );
        for f in &t.fields {
            let ty = f.primitive.to_string();
            let ty = if f.optional {
                format!("Option<{ty}>")
            } else {
                ty
            };
            let _ = writeln!(
                out,
                "    @iri(\"{}{}\") {}: {ty}",
                escape(&vocab),
                f.name,
                f.name
            );
        }
        for e in model.edges.iter().filter(|e| e.from == t.name) {
            let _ = writeln!(
                out,
                "    @iri(\"{}{}\") {}: {}",
                escape(&vocab),
                e.name,
                e.name,
                relation(e)
            );
        }
        out.push('\n');
    }

    for s in &model.sources {
        let delimiter = s
            .delimiter
            .as_deref()
            .map(|d| format!(", delimiter = \"{}\"", escape(d)))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "{} := io.{}(\"{}\"{delimiter})",
            s.name,
            s.format,
            escape(&s.reference)
        );
    }

    for t in &model.types {
        let subject = format!("    @subject = \"{}\"", subject(t, base));
        let field = |f: &ModelField| format!("    {} = {}", f.name, column(&t.source, &f.column));
        let edge =
            |e: &ModelEdge| format!("    {} = {}({})", e.name, e.to, column(&e.on, &e.column));
        let edges: Vec<&ModelEdge> = model.edges.iter().filter(|e| e.from == t.name).collect();

        let _ = write!(
            out,
            "\n{}Rows : {} from {}\n{subject}\n",
            t.name, t.name, t.source
        );
        for f in &t.fields {
            let _ = writeln!(out, "{}", field(f));
        }
        for e in edges.iter().filter(|e| e.join.is_none()) {
            let _ = writeln!(out, "{}", edge(e));
        }

        for e in edges.iter().filter(|e| e.join.is_some()) {
            let (Some(key), Some(join)) = (t.identity.first(), e.join.as_deref()) else {
                continue;
            };
            let _ = write!(
                out,
                "\n{}{} : {} from {}.join({}, on = {} == {})\n{subject}\n",
                t.name,
                upper(&e.name),
                t.name,
                t.source,
                e.on,
                column(&t.source, key),
                column(&e.on, join)
            );
            for f in t.fields.iter().filter(|f| !f.optional) {
                let _ = writeln!(out, "{}", field(f));
            }
            for required in edges
                .iter()
                .filter(|r| r.join.is_none() && !r.many && !r.optional)
            {
                let _ = writeln!(out, "{}", edge(required));
            }
            let _ = writeln!(out, "{}", edge(e));
        }
    }
    out
}
