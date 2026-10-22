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

use std::collections::HashMap;
use std::fmt::Write as _;
use std::hash::BuildHasher;

use fossil_base::CONSTRUCTOR_PREFIX;
use fossil_graph_schema::Primitive;
use fossil_hir::def_map::reader_option_of;
use fossil_profile::{Inclusion, Profile, SourceProfile};
use fossil_syntax::name::{escape, identifier, spell};
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
    /// The value of the reader option its format's catalogue row declares (`|`, written
    /// `delimiter = "|"` on `csv`), verbatim. A format that declares none writes none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option: Option<String>,
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
        let constructor = format!("{CONSTRUCTOR_PREFIX}{}", s.format);
        let option = s
            .option
            .as_deref()
            .zip(reader_option_of(&constructor))
            .map(|(value, name)| format!(", {name} = \"{}\"", escape(value)))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "{} := {constructor}(\"{}\"{option})",
            s.name,
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

/// A question the profile cannot settle, and the answer the model stands at.
///
/// `infer` never waits on one: the model it answers takes `chosen`, the host's choice when it
/// gave one that is among `options`, else the first. A host shows the person what it asks and
/// passes the answer back by `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Ambiguity {
    /// What a choice is keyed by: `key/<source>`, `count/<source>` or `name/<type>/<name>`.
    pub id: String,
    pub about: Ambiguous,
    pub options: Vec<String>,
    pub chosen: String,
}

/// What an [`Ambiguity`] asks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Ambiguous {
    /// Which of a source's candidate keys identifies one of its rows: the options are the
    /// columns. Asked only where no inclusion lands on one, as one that does fixes it.
    Key { source: String },
    /// A link file between two types: a row of it is only that link (`link`), or a thing
    /// counted on its own (`thing`), identified by the two ends it links.
    Count {
        source: String,
        from: String,
        to: String,
    },
    /// Columns of one type whose words make one name: the options are the columns, and the
    /// one chosen keeps it. A suffix is not the answer (`/docs/design/discarded`).
    Name { r#type: String, name: String },
}

/// What [`infer`] answers: the model, and the questions it took a default for.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
pub struct Inferred {
    pub model: Model,
    pub ambiguities: Vec<Ambiguity>,
}

/// The model a profile implies — W3C's Direct Mapping with the bridge rule D2RQ and Ontop add.
///
/// A source with a candidate key is a type, named by its binding in `PascalCase`
/// (`laureates` → `Laureates`, never singularised), identified by the key an inclusion lands on,
/// else by its first; an inclusion from a type's column to another's key is a relation, named by
/// the column without a trailing `id` (`country_id` → `country`); a source with no key whose two
/// inclusions land on types' keys is a link file, and a relation across it — `Seq<Ref<…>>` — or,
/// chosen, a type of its own; every other column is a field, named by its own words
/// ([`identifier`](fossil_syntax::name::identifier)). A source that is none of these is left out:
/// nothing identifies one of its rows.
///
/// `choices` answers [`Ambiguity`]s by id; one not given stands at its first option.
#[must_use]
pub fn infer<S: BuildHasher>(profile: &Profile, choices: &HashMap<String, String, S>) -> Inferred {
    let mut ambiguities = Vec::new();
    let mut ask = |id: String, about: Ambiguous, options: Vec<String>| -> String {
        let chosen = choices
            .get(&id)
            .filter(|c| options.contains(c))
            .or_else(|| options.first())
            .cloned()
            .unwrap_or_default();
        ambiguities.push(Ambiguity {
            id,
            about,
            options,
            chosen: chosen.clone(),
        });
        chosen
    };
    let source = |name: &str| profile.sources.iter().find(|s| s.binding == name);

    // Each type's key: the candidate an inclusion lands on, else the one chosen.
    let mut keys: Vec<(&SourceProfile, String)> = Vec::new();
    for s in profile
        .sources
        .iter()
        .filter(|s| !s.candidate_keys.is_empty())
    {
        let landed = profile
            .inclusions
            .iter()
            .find(|i| i.to.source == s.binding && s.candidate_keys.contains(&i.to.column));
        let key = match (landed, s.candidate_keys.as_slice()) {
            (Some(i), _) => i.to.column.to_string(),
            (None, [only]) => only.to_string(),
            (None, candidates) => ask(
                format!("key/{}", s.binding),
                Ambiguous::Key {
                    source: s.binding.to_string(),
                },
                candidates.iter().map(ToString::to_string).collect(),
            ),
        };
        keys.push((s, key));
    }
    let key_of = |binding: &str| {
        keys.iter()
            .find(|(s, _)| s.binding == binding)
            .map(|(_, k)| k.as_str())
    };
    let lands = |i: &Inclusion| key_of(&i.to.source) == Some(i.to.column.as_str());

    let mut names: HashMap<&str, String> = HashMap::new();
    for (s, _) in &keys {
        let name = pascal(&s.binding);
        let name = if names.values().any(|n| *n == name) {
            s.binding.to_string()
        } else {
            name
        };
        names.insert(&s.binding, name);
    }

    // The relations, each with the columns it leaves from: on the type's own source, or across
    // a link file.
    let mut edges: Vec<(ModelEdge, String)> = Vec::new();
    for i in profile
        .inclusions
        .iter()
        .filter(|i| key_of(&i.from.source).is_some() && lands(i))
    {
        let optional = source(&i.from.source)
            .and_then(|s| s.columns.iter().find(|c| c.name == i.from.column))
            .is_some_and(|c| c.null_count > 0);
        edges.push((
            ModelEdge {
                name: String::new(),
                from: names[i.from.source.as_str()].clone(),
                to: names[i.to.source.as_str()].clone(),
                on: i.from.source.to_string(),
                column: i.from.column.to_string(),
                join: None,
                many: false,
                optional,
            },
            i.from.source.to_string(),
        ));
    }
    let mut things: Vec<ModelType> = Vec::new();
    for link in profile
        .sources
        .iter()
        .filter(|s| key_of(&s.binding).is_none())
    {
        let ends: Vec<&Inclusion> = profile
            .inclusions
            .iter()
            .filter(|i| i.from.source == link.binding && lands(i))
            .collect();
        let [back, via] = ends.as_slice() else {
            continue;
        };
        let (from, to) = (
            names[back.to.source.as_str()].clone(),
            names[via.to.source.as_str()].clone(),
        );
        let counted = ask(
            format!("count/{}", link.binding),
            Ambiguous::Count {
                source: link.binding.to_string(),
                from: from.clone(),
                to: to.clone(),
            },
            vec!["link".into(), "thing".into()],
        );
        if counted == "link" {
            edges.push((
                ModelEdge {
                    name: String::new(),
                    from,
                    to,
                    on: link.binding.to_string(),
                    column: via.from.column.to_string(),
                    join: Some(back.from.column.to_string()),
                    many: true,
                    optional: false,
                },
                back.to.source.to_string(),
            ));
            continue;
        }
        let name = pascal(&link.binding);
        let name = if names.values().any(|n| *n == name) {
            link.binding.to_string()
        } else {
            name
        };
        let identity = vec![back.from.column.to_string(), via.from.column.to_string()];
        for (end, ty) in [(back, from), (via, to)] {
            edges.push((
                ModelEdge {
                    name: String::new(),
                    from: name.clone(),
                    to: ty,
                    on: link.binding.to_string(),
                    column: end.from.column.to_string(),
                    join: None,
                    many: false,
                    optional: false,
                },
                link.binding.to_string(),
            ));
        }
        things.push(ModelType {
            name,
            source: link.binding.to_string(),
            identity,
            fields: Vec::new(),
        });
    }

    let mut types: Vec<ModelType> = keys
        .iter()
        .map(|(s, key)| ModelType {
            name: names[s.binding.as_str()].clone(),
            source: s.binding.to_string(),
            identity: vec![key.clone()],
            fields: Vec::new(),
        })
        .chain(things)
        .collect();

    // The members of each type — its fields and the relations it holds — named, and every name
    // two of them would share asked.
    let mut kept: Vec<ModelEdge> = Vec::new();
    for t in &mut types {
        let Some(s) = source(&t.source) else { continue };
        let leaving: Vec<&str> = edges
            .iter()
            .filter(|(e, _)| e.from == t.name && e.on == t.source)
            .map(|(e, _)| e.column.as_str())
            .collect();
        let fields: Vec<ModelField> = s
            .descriptor
            .columns
            .iter()
            .filter(|c| {
                !t.identity.iter().any(|k| *k == c.name) && !leaving.contains(&c.name.as_str())
            })
            .map(|c| ModelField {
                name: identifier(&c.name),
                column: c.name.to_string(),
                primitive: c.primitive,
                optional: s
                    .columns
                    .iter()
                    .find(|stats| stats.name == c.name)
                    .is_some_and(|stats| stats.null_count > 0),
            })
            .collect();
        let mut own: Vec<ModelEdge> = edges
            .iter()
            .filter(|(e, _)| e.from == t.name)
            .map(|(e, _)| e.clone())
            .collect();
        for i in 0..own.len() {
            let spoken = identifier(&without_id(&own[i].column));
            let taken = fields.iter().any(|f| f.name == spoken)
                || own
                    .iter()
                    .enumerate()
                    .any(|(j, e)| j != i && identifier(&without_id(&e.column)) == spoken);
            own[i].name = if taken {
                identifier(&own[i].column)
            } else {
                spoken
            };
        }
        let mut members: Vec<(String, String)> = fields
            .iter()
            .map(|f| (f.name.clone(), f.column.clone()))
            .chain(own.iter().map(|e| (e.name.clone(), e.column.clone())))
            .collect();
        let mut shared: Vec<String> = Vec::new();
        for (i, (name, _)) in members.iter().enumerate() {
            if members[..i].iter().any(|(n, _)| n == name) && !shared.contains(name) {
                shared.push(name.clone());
            }
        }
        for name in shared {
            let columns: Vec<String> = members
                .iter()
                .filter(|(n, _)| *n == name)
                .map(|(_, c)| c.clone())
                .collect();
            let chosen = ask(
                format!("name/{}/{name}", t.name),
                Ambiguous::Name {
                    r#type: t.name.clone(),
                    name: name.clone(),
                },
                columns,
            );
            members.retain(|(n, c)| *n != name || *c == chosen);
        }
        let keeps =
            |name: &str, column: &str| members.iter().any(|(n, c)| n == name && c == column);
        t.fields = fields
            .into_iter()
            .filter(|f| keeps(&f.name, &f.column))
            .collect();
        kept.extend(own.into_iter().filter(|e| keeps(&e.name, &e.column)));
    }

    let used = |binding: &str| {
        types.iter().any(|t| t.source == binding) || kept.iter().any(|e| e.on == binding)
    };
    let sources = profile
        .sources
        .iter()
        .filter(|s| used(&s.binding))
        .filter_map(|s| {
            Some(ModelSource {
                name: s.binding.to_string(),
                format: s.format.clone()?,
                reference: s.descriptor.key.to_string(),
                option: s.option.clone(),
            })
        })
        .collect();
    Inferred {
        model: Model {
            sources,
            types,
            edges: kept,
        },
        ambiguities,
    }
}

/// A binding's words as a type's name: `nobel_prizes` → `NobelPrizes`. Not singularised: a
/// heuristic that made `Purchasis` is in `/docs/design/discarded`.
fn pascal(binding: &str) -> String {
    identifier(&binding.split('_').map(upper).collect::<String>())
}

/// A column that holds another's key, said without its trailing `id`: `country_id` → `country`,
/// `Institution.id` → `Institution`, `laureateId` → `laureate`; a column that is only `id`, or
/// has none, is itself.
fn without_id(column: &str) -> String {
    for suffix in ["_id", ".id", "-id", " id", "_ID", ".ID", "Id"] {
        if let Some(rest) = column.strip_suffix(suffix)
            && !rest.is_empty()
        {
            return rest.to_string();
        }
    }
    column.to_string()
}
