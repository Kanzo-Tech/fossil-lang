//! Inputs + formats — the parse-only "what does this program read?" and "what
//! formats does fossil read?" surface.
//!
//! One implementation for every host — the browser `fossil-wasm` (over its
//! in-memory `WasmDb`) and any native one over a file-backed db. The host
//! injects only its own [`fossil_base::Db`] + program text; the logic — parse →
//! source headers → inputs — is identical and pure (no I/O, no `DuckDB`),
//! so it is WASM-clean.
//!
//! [`formats`] is a projection of [`fossil_base::providers`] and [`inputs`]
//! walks the def map, so its content is the language's and it is asked of the
//! compiler's own queries. The types below are the shapes a HOST reads — serde
//! JSON natively, `serde-wasm-bindgen` values in the browser — and they are
//! declared beside the two functions that fill them: a contract belongs to
//! whoever fills it.

use fossil_base::{Db, SourceFile};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The position an input plays in a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// The positional data URI of an `io.*` source constructor (`io.csv("…")`).
    Data,
    /// A shape document: `type { … } := io.shex("…")`, or a source's
    /// `schema = io.shex("…")`.
    Schema,
}

/// One input a program reads — `Input` in `@fossil-lang/types`, and the one
/// answer to «what does this program read»: the editor's, the introspecting
/// host's and the executor's.
///
/// The name is `OpenLineage`'s `RunEvent.inputs[]` and PROV's `prov:used`, and not
/// an editor's `references`, which are the usages of a symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Input {
    /// What the program reads it as.
    pub role: Role,
    /// The reference as the program wrote it — what an inferred descriptor is
    /// keyed by, and what survives a connection being repointed.
    pub key: String,
    /// `key` through [`fossil_base::location::SourceAnchor`]: what a host signs and reads.
    pub location: String,
    /// The connection the location lies under, when it was written `@name/…`
    /// and the connections have that name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    /// The binding a data input is read into (`users` in `users := io.csv(…)`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<String>,
    /// The catalogue row a data input's constructor names (`csv`), which
    /// chooses the reader.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// The reader option a data input's binding wrote (`delimiter = "|"`), verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option: Option<String>,
}

/// Every input `file` reads: one per source binding whose constructor names a
/// row the host installs — a destructured `{ A, B } := io.rdf(…)` is two — then
/// each distinct shape document it names.
///
/// The database's connections ([`fossil_base::connections()`]) expand `@conn`
/// aliases in the location only; the key is what the program wrote, so the
/// descriptor a host registers under it survives a connection being repointed.
#[must_use]
pub fn inputs(db: &dyn Db, file: SourceFile) -> Vec<Input> {
    let dir = fossil_base::location::program_dir(file.path(db));
    let anchor = fossil_base::location::SourceAnchor::new(&dir, fossil_base::connections(db));
    let read = |role, key: &str| Input {
        role,
        key: key.to_string(),
        location: anchor.location(key),
        connection: anchor.connection(key),
        binding: None,
        format: None,
        option: None,
    };
    let data = crate::def_map::def_map(db, file)
        .sources(db)
        .iter()
        .filter_map(|s| {
            let uri = s.uri.as_deref()?;
            let row = fossil_base::provider(db.system().providers(), s.constructor.as_deref()?)?;
            Some(Input {
                binding: Some(s.name.to_string()),
                format: Some(row.name.to_string()),
                option: s.delimiter.as_ref().map(ToString::to_string),
                ..read(Role::Data, uri)
            })
        })
        .collect::<Vec<_>>();
    let schemas = crate::documents::documents_named(db, file);
    data.into_iter()
        .chain(schemas.iter().map(|d| read(Role::Schema, d)))
        .collect()
}

/// A data source a host names before there is a program — the files a person
/// was asked about: what a program would write in its binding, and nothing else.
///
/// `format` is the host's: nothing selects a row by extension
/// (`fossil_base::providers`), as one extension can name two rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DataRef {
    /// The reference as a program would write it: a name beside the program,
    /// `@conn/path`, or a URL.
    pub reference: String,
    /// The catalogue row that reads it (`csv`).
    pub format: String,
    /// The value of its reader option (`|`), verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option: Option<String>,
}

/// The inputs `refs` are, as [`inputs`] would answer them for a program at the
/// workspace's root that bound each under its
/// [`binding_name`](fossil_syntax::name::binding_name): the same anchor, the
/// same connections, and no program text written to get there.
#[must_use]
pub fn inputs_named(db: &dyn Db, refs: &[DataRef]) -> Vec<Input> {
    let dir = fossil_base::location::program_dir("");
    let anchor = fossil_base::location::SourceAnchor::new(&dir, fossil_base::connections(db));
    refs.iter()
        .map(|r| Input {
            role: Role::Data,
            key: r.reference.clone(),
            location: anchor.location(&r.reference),
            connection: anchor.connection(&r.reference),
            binding: Some(fossil_syntax::name::binding_name(&r.reference)),
            format: Some(r.format.clone()),
            option: r.option.clone(),
        })
        .collect()
}

/// What a format can appear as in a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FormatKind {
    /// Only defines a type (e.g. a schema descriptor).
    Schema,
    /// Loads data (the `io.*` source constructors).
    Data,
    /// Usable in both positions.
    Both,
}

/// One format fossil reads — Arrow's `FileFormat`, `DuckDB`'s `FORMAT`: its short
/// name, the file extensions it reads, and how it can be used. A host lists
/// these so its UI can offer the constructors and filter files by extension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Format {
    /// Short name (e.g. `csv`, `json`, `parquet`) — what [`Input::format`] holds.
    pub name: String,
    /// File extensions it reads (no leading dot).
    pub extensions: Vec<String>,
    /// Whether it defines a type, loads data, or both.
    pub kind: FormatKind,
}

/// The provider rows a host installs, projected onto the wire as formats. Sorted for
/// a deterministic order (the registry's own order is priority, not display).
///
/// # `FormatKind` is read off the row
///
/// The row declares its capabilities, so the three cases are read off it:
/// `io.shex` and `io.shacl` come out `Schema`, and the day one row does both,
/// `Both`.
///
/// `table` is the host's, not a constant, because the rows that read types are
/// the host's to install — `crate::providers::PROVIDERS` for a host
/// that compiles, `fossil_base::providers::DATA` for one that does not.
#[must_use]
pub fn formats(table: &[&'static fossil_base::Provider]) -> Vec<Format> {
    use fossil_base::Capability;

    let mut formats: Vec<Format> = table
        .iter()
        .map(|p| Format {
            name: p.name.to_string(),
            extensions: p.extensions.iter().map(|e| (*e).to_string()).collect(),
            kind: match (
                p.provides(Capability::ReadRows),
                p.provides(Capability::ReadTypes),
            ) {
                (true, true) => FormatKind::Both,
                (false, true) => FormatKind::Schema,
                _ => FormatKind::Data,
            },
        })
        .collect();
    formats.sort_by(|a, b| a.name.cmp(&b.name));
    formats
}

#[cfg(test)]
mod tests {
    use super::{DataRef, FormatKind, Input, Role, formats, inputs, inputs_named};

    #[test]
    fn an_input_keeps_what_the_program_wrote_and_resolves_where_it_is_read() {
        let mut db = fossil_base::test_support::new_db();
        let file = fossil_base::SourceFile::new(
            &db,
            "users := io.csv(\"@lake/users.csv\", delimiter = \"|\")\n\
             orders := io.parquet(\"orders.parquet\")\n"
                .to_string(),
            "a/prog.fossil".to_string(),
        );
        fossil_base::set_connections(
            &mut db,
            [("lake".to_string(), "s3://bucket/".to_string())].into(),
        );
        assert_eq!(
            inputs(&db, file),
            [
                Input {
                    role: Role::Data,
                    key: "@lake/users.csv".to_string(),
                    location: "s3://bucket/users.csv".to_string(),
                    connection: Some("lake".to_string()),
                    binding: Some("users".to_string()),
                    format: Some("csv".to_string()),
                    option: Some("|".to_string()),
                },
                Input {
                    role: Role::Data,
                    key: "orders.parquet".to_string(),
                    location: "a/orders.parquet".to_string(),
                    connection: None,
                    binding: Some("orders".to_string()),
                    format: Some("parquet".to_string()),
                    option: None,
                },
            ]
        );
    }

    #[test]
    fn a_named_source_is_the_input_a_program_at_the_root_binding_it_would_read() {
        let mut db = fossil_base::test_support::new_db();
        fossil_base::set_connections(
            &mut db,
            [("lake".to_string(), "s3://bucket/".to_string())].into(),
        );
        let file = fossil_base::SourceFile::new(
            &db,
            "_lake_users := io.csv(\"@lake/users.csv\", delimiter = \"|\")\n\
             orders := io.parquet(\"orders.parquet\")\n"
                .to_string(),
            String::new(),
        );
        let named = [
            DataRef {
                reference: "@lake/users.csv".into(),
                format: "csv".into(),
                option: Some("|".into()),
            },
            DataRef {
                reference: "orders.parquet".into(),
                format: "parquet".into(),
                option: None,
            },
        ];
        assert_eq!(inputs_named(&db, &named), inputs(&db, file));
    }

    #[test]
    fn a_shape_document_is_an_input_once() {
        let mut db = fossil_base::test_support::new_db();
        let file = fossil_base::SourceFile::new(
            &db,
            "type { P } := io.shex(\"@lake/x.shex\")\n\
             { A, B } := io.rdf(\"g.ttl\", schema = io.shex(\"@lake/x.shex\"))\n"
                .to_string(),
            "prog.fossil".to_string(),
        );
        fossil_base::set_connections(
            &mut db,
            [("lake".to_string(), "s3://bucket/".to_string())].into(),
        );
        let schemas: Vec<Input> = inputs(&db, file)
            .into_iter()
            .filter(|i| i.role == Role::Schema)
            .collect();
        assert_eq!(
            schemas,
            [Input {
                role: Role::Schema,
                key: "@lake/x.shex".to_string(),
                location: "s3://bucket/x.shex".to_string(),
                connection: Some("lake".to_string()),
                binding: None,
                format: None,
                option: None,
            }]
        );
    }

    #[test]
    fn providers_are_sorted_nonempty_and_include_csv() {
        let p = formats(fossil_base::providers::DATA);
        assert!(!p.is_empty(), "the source registry must expose formats");
        let mut sorted = p.clone();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(
            p, sorted,
            "formats must be deterministically sorted by name"
        );
        let names: Vec<&str> = p.iter().map(|x| x.name.as_str()).collect();
        assert!(names.contains(&"csv"), "csv must be present: {names:?}");
        assert!(
            p.iter().all(|x| x.kind == FormatKind::Data),
            "the default table reads data and nothing else"
        );
    }

    /// `FormatKind::Schema` had no producer at all until the registry
    /// collapsed: the wire contract described three cases and the data behind it
    /// was half a table. A row that reads types comes out `Schema`.
    #[test]
    fn a_type_reading_row_projects_as_a_schema_provider() {
        use fossil_base::Provider;
        use fossil_base::test_support::decode_lines;

        static ROW: Provider = Provider {
            name: "shex",
            extensions: &["shex"],
            reads_rows: None,
            // Any `DecodeTypes` will do — what is under test is the projection,
            // and `fossil-base`'s reference decoder is the one row this crate can
            // name without learning a schema language.
            reads_types: Some(decode_lines),
        };

        let p = formats(&[&fossil_base::providers::CSV, &ROW]);
        assert_eq!(p[0].name, "csv");
        assert_eq!(p[0].kind, FormatKind::Data);
        assert_eq!(p[1].name, "shex");
        assert_eq!(p[1].kind, FormatKind::Schema);
    }
}
