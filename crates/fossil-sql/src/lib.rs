//! `fossil-sql` — a compiled program as ONE `DuckDB` query whose rows are its
//! triples: [`artefact`].
//!
//! **fossil does not run a program; the host does** (decision of 29 October:
//! "sin executor en fossil"). The compiler is sans-IO and links no engine, so
//! its last representation is SQL, the way rustc's is LLVM IR: the host runs
//! the query on the `DuckDB` it already has, and hands the rows to whatever
//! stores them (`graphar-rdf`, an N-Triples writer).
//!
//! The query is lowered straight from the MIR ([`fossil_mir::Op`]), which is
//! Min Oo and Hartig's relational algebra with a type on every schema: each
//! operator is one common table expression, and each column a relation carries
//! is named `"<qualifier>.<column>"`, so the qualification the algebra keeps
//! (`Purchase.id` beside `User.id` after a join) is the column's own name and
//! no operator has to arbitrate between two `id`s.
//!
//! # The rows
//!
//! `(s, type, p, o, datatype, lang)`, ordered by subject:
//!
//! - `s` — the subject IRI;
//! - `type` — the vertex type the subject belongs to (`Person`): what lets a
//!   reader route a subject's triples to one table without a second pass;
//! - `p` — the predicate IRI;
//! - `o` — the object: an IRI when `datatype` is NULL, else a literal's lexical
//!   form, every value cast to its fossil type first and never left as the
//!   engine inferred it;
//! - `datatype` — the literal's datatype IRI, `rdf:langString` for a tagged
//!   text;
//! - `lang` — the tag of a tagged text, lower-cased; NULL otherwise.
//!
//! A NULL generates no triple (RML-Core §12.2), and the rows are a set.
//!
//! # What the meaning is held to
//!
//! `tests/oracle.rs` runs every conformance program both ways — this query on
//! `DuckDB`, and the engine (`fossil-exec`) through its N-Triples sink — and
//! compares the triples. The engine is the definition until it is deleted.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use fossil_base::SourceFile;
use fossil_base::location::SourceAnchor;
use fossil_graph_schema::{Primitive, RDF_LANG_STRING, Term, Xsd};
use fossil_hir::stdlib::{AggFn, LoweringKind};
use fossil_hir::{BinOp, TyKind, UnOp};
use fossil_mir::{Expr, JoinKind, Op, SourceFormat, VProp};
use smol_str::SmolStr;

/// `rdf:type`.
const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";

/// What [`artefact`] gives a host: the query, and the shape documents the
/// program names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artefact {
    /// One `DuckDB` query answering `(s, type, p, o, datatype, lang)`.
    pub sql: String,
    /// The shape documents the program's output types come from, as written.
    ///
    /// A type DECLARED in the program (`type Person := { … }`) has no document
    /// yet: writing one as `ShEx` or SHACL is not done, so such a program
    /// answers an empty list here.
    pub shapes: Vec<ShapeDocument>,
}

/// One shape document a program names, as the host registered it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeDocument {
    /// The location the program's `io.shex("…")` / `io.shacl("…")` resolves to.
    pub location: String,
    /// Its text.
    pub text: String,
}

/// Why a program has no query.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// The program has errors; its diagnostics say which.
    #[error("the program does not compile")]
    DoesNotCompile,
    /// A construct this backend does not lower yet, named.
    #[error("{0}")]
    Unsupported(String),
}

/// The query that gives `file`'s triples, and the shapes they conform to.
///
/// A source is read with its columns typed as the PROGRAM types them (the row
/// type the checker holds), never as the engine would infer them: a column of
/// zip codes that a profile called text stays text.
///
/// # Errors
///
/// [`Refusal::DoesNotCompile`] when lowering any mapping is poisoned, and
/// [`Refusal::Unsupported`] for a construct named in it.
pub fn artefact(db: &dyn fossil_base::Db, file: SourceFile) -> Result<Artefact, Refusal> {
    let program_dir = fossil_base::location::program_dir(file.path(db));
    let anchor = SourceAnchor::new(&program_dir, fossil_base::connections(db));
    let mut q = Query::default();
    let mut types: Vec<VertexType> = Vec::new();
    let mut edges: Vec<Edge> = Vec::new();

    for (m, &mapping) in fossil_hir::def_map::def_map(db, file)
        .mappings(db)
        .iter()
        .enumerate()
    {
        let mir = fossil_mir::lower_to_mir_pg(db, mapping);
        if mir.error(db).is_some() {
            return Err(Refusal::DoesNotCompile);
        }
        let ops = mir.ops(db);
        let mut rel: BTreeMap<usize, Relation> = BTreeMap::new();
        for (i, op) in ops.iter().enumerate() {
            match op {
                Op::EmitVertex {
                    input,
                    type_name,
                    rdf_type,
                    id,
                    dedup,
                    props,
                } => {
                    let from = relation(&rel, *input)?;
                    let name = format!("m{m}_vertex");
                    let mut cols = vec![format!("{} AS {}", expr(id, from)?, ident("subject"))];
                    let mut emitted = Vec::new();
                    for p in props {
                        let prim = fossil_hir::shapes::inner_primitive(db, p.ty);
                        cols.push(format!(
                            "{} AS {}",
                            typed(&expr(&p.value, from)?, prim),
                            ident(&p.name)
                        ));
                        emitted.push(Prop::of(p, prim));
                    }
                    q.cte(
                        &name,
                        format!("SELECT {} FROM {}", cols.join(", "), ident(&from.name)),
                    );
                    let part = Part {
                        cte: name,
                        props: emitted,
                    };
                    match types.iter_mut().find(|t| t.label == type_name.as_str()) {
                        Some(t) => {
                            t.parts.push(part);
                            t.dedup |= *dedup;
                        }
                        None => types.push(VertexType {
                            label: type_name.to_string(),
                            class: rdf_type.as_ref().map(ToString::to_string),
                            dedup: *dedup,
                            parts: vec![part],
                        }),
                    }
                }
                Op::EmitEdge {
                    input,
                    rdf_uri,
                    src_type,
                    dst_type,
                    src_id,
                    dst_id,
                    ..
                } => {
                    let from = relation(&rel, *input)?;
                    let name = format!("m{m}_edge{i}");
                    q.cte(
                        &name,
                        format!(
                            "SELECT {} AS s, {} AS o FROM {}",
                            expr(src_id, from)?,
                            expr(dst_id, from)?,
                            ident(&from.name)
                        ),
                    );
                    edges.push(Edge {
                        cte: name,
                        predicate: rdf_uri.as_ref().map(ToString::to_string),
                        src_type: src_type.to_string(),
                        dst_type: dst_type.to_string(),
                    });
                }
                Op::Sink { .. } => {}
                _ => {
                    let r = lower(db, anchor, op, &rel, &format!("m{m}_o{i}"), &mut q)?;
                    rel.insert(i, r);
                }
            }
        }
    }

    let mut triples = Vec::new();
    for t in &types {
        vertex(t, &mut q, &mut triples);
    }
    for e in &edges {
        let Some(p) = &e.predicate else { continue };
        if !types.iter().any(|t| t.label == e.src_type)
            || !types.iter().any(|t| t.label == e.dst_type)
        {
            continue;
        }
        triples.push(format!(
            "SELECT s, {} AS type, {} AS p, o, NULL AS datatype, NULL AS lang FROM {} \
             WHERE s IS NOT NULL AND o IS NOT NULL \
             AND s IN (SELECT subject FROM {}) AND o IN (SELECT subject FROM {})",
            string(&e.src_type),
            string(p),
            ident(&e.cte),
            ident(&format!("subjects:{}", e.src_type)),
            ident(&format!("subjects:{}", e.dst_type)),
        ));
    }
    let body = if triples.is_empty() {
        "SELECT NULL::VARCHAR AS s, NULL::VARCHAR AS type, NULL::VARCHAR AS p, \
         NULL::VARCHAR AS o, NULL::VARCHAR AS datatype, NULL::VARCHAR AS lang WHERE false"
            .to_string()
    } else {
        triples.join("\nUNION\n")
    };
    q.cte("triples", body);
    let mut sql = q.render();
    sql.push_str("SELECT s, type, p, o, datatype, lang FROM triples ORDER BY s, type, p, o, datatype, lang\n");

    Ok(Artefact {
        sql,
        shapes: shapes(db, file),
    })
}

// ─────────────────────────────────────────────────────────────── the query

/// The common table expressions, in the order they are defined: an operator's
/// inputs are always defined before it.
#[derive(Default)]
struct Query {
    ctes: Vec<(String, String)>,
}

impl Query {
    fn cte(&mut self, name: &str, body: String) {
        self.ctes.push((name.to_string(), body));
    }

    fn render(&self) -> String {
        let mut out = String::from("WITH\n");
        for (i, (name, body)) in self.ctes.iter().enumerate() {
            let sep = if i + 1 == self.ctes.len() { "" } else { "," };
            let _ = writeln!(out, "{} AS (\n{body}\n){sep}", ident(name));
        }
        out
    }
}

/// A relation the query defines: its name, and its columns as
/// `(qualifier, column)` — each one named `"qualifier.column"` in SQL.
#[derive(Clone)]
struct Relation {
    name: String,
    cols: Vec<(SmolStr, SmolStr)>,
}

impl Relation {
    /// The SQL column a `ColRef` names. An unqualified reference (a field of
    /// the mapping's own row) is the one column of that name.
    fn column(&self, source: &str, column: &str) -> Result<String, Refusal> {
        let hit = self
            .cols
            .iter()
            .find(|(q, c)| c == column && (source.is_empty() || q == source));
        hit.map(|(q, c)| ident(&qualified(q, c))).ok_or_else(|| {
            Refusal::Unsupported(format!(
                "`{source}.{column}` is not a column of `{}`",
                self.name
            ))
        })
    }
}

fn relation(rel: &BTreeMap<usize, Relation>, i: usize) -> Result<&Relation, Refusal> {
    rel.get(&i)
        .ok_or_else(|| Refusal::Unsupported(format!("op {i} is not a relation")))
}

/// Every column of `from` under `name`: the select list, and the columns.
fn requalify(from: &Relation, name: &SmolStr) -> (String, Vec<(SmolStr, SmolStr)>) {
    let cols: Vec<(SmolStr, SmolStr)> = from
        .cols
        .iter()
        .map(|(_, c)| (name.clone(), c.clone()))
        .collect();
    let select = from
        .cols
        .iter()
        .zip(&cols)
        .map(|((q, c), (nq, nc))| {
            format!(
                "{} AS {}",
                ident(&qualified(q, c)),
                ident(&qualified(nq, nc))
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    (select, cols)
}

fn qualified(q: &str, c: &str) -> String {
    format!("{q}.{c}")
}

/// One relational operator as one CTE named `name`.
fn lower(
    db: &dyn fossil_base::Db,
    anchor: SourceAnchor<'_>,
    op: &Op<'_>,
    rel: &BTreeMap<usize, Relation>,
    name: &str,
    q: &mut Query,
) -> Result<Relation, Refusal> {
    let (cols, body) = match op {
        Op::Source {
            uri,
            format,
            row_type,
            binding,
        } => {
            let TyKind::Record(record) = row_type.kind(db) else {
                return Err(Refusal::Unsupported(format!(
                    "the source `{binding}` has no row type: its columns are not known"
                )));
            };
            let fields: Vec<(SmolStr, Option<Primitive>)> = record
                .fields(db)
                .iter()
                .map(|f| {
                    (
                        f.name.clone(),
                        fossil_hir::shapes::inner_primitive(db, f.ty),
                    )
                })
                .collect();
            let location = anchor.location(uri);
            let reader = reader(&location, format, &fields)?;
            let cols: Vec<(SmolStr, SmolStr)> = fields
                .iter()
                .map(|(f, _)| (binding.clone(), f.clone()))
                .collect();
            let select = fields
                .iter()
                .map(|(f, p)| {
                    format!(
                        "{} AS {}",
                        typed(&ident(f), *p),
                        ident(&qualified(binding, f))
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            (cols, format!("SELECT {select} FROM {reader}"))
        }
        Op::Filter { input, pred } => {
            let from = relation(rel, *input)?;
            (
                from.cols.clone(),
                format!(
                    "SELECT * FROM {} WHERE {}",
                    ident(&from.name),
                    expr(pred, from)?
                ),
            )
        }
        Op::Project { input, cols } => {
            let from = relation(rel, *input)?;
            let mut out = Vec::new();
            let mut select = Vec::new();
            for c in cols {
                select.push(from.column(&c.source, &c.column)?);
                let q = from
                    .cols
                    .iter()
                    .find(|(q, n)| *n == c.column && (c.source.is_empty() || *q == c.source))
                    .map_or_else(|| c.source.clone(), |(q, _)| q.clone());
                out.push((q, c.column.clone()));
            }
            (
                out,
                format!("SELECT {} FROM {}", select.join(", "), ident(&from.name)),
            )
        }
        Op::Extend {
            input,
            field,
            expr: e,
        } => {
            let from = relation(rel, *input)?;
            let mut cols = from.cols.clone();
            let q = cols.first().map(|(q, _)| q.clone()).unwrap_or_default();
            cols.push((q.clone(), field.clone()));
            (
                cols,
                format!(
                    "SELECT *, {} AS {} FROM {}",
                    expr(e, from)?,
                    ident(&qualified(&q, field)),
                    ident(&from.name)
                ),
            )
        }
        Op::Rename { input, old, new } => {
            let from = relation(rel, *input)?;
            let mut select = Vec::new();
            let mut cols = Vec::new();
            for (q, c) in &from.cols {
                let n = if c == old { new.clone() } else { c.clone() };
                select.push(format!(
                    "{} AS {}",
                    ident(&qualified(q, c)),
                    ident(&qualified(q, &n))
                ));
                cols.push((q.clone(), n));
            }
            (
                cols,
                format!("SELECT {} FROM {}", select.join(", "), ident(&from.name)),
            )
        }
        Op::Distinct { input } => {
            let from = relation(rel, *input)?;
            (
                from.cols.clone(),
                format!("SELECT DISTINCT * FROM {}", ident(&from.name)),
            )
        }
        // `UNION ALL` by position, then every column re-qualified under the
        // relation the op names: neither side's qualifier is the union's.
        Op::Union {
            left,
            right,
            relation: name_,
        } => {
            let (l, r) = (relation(rel, *left)?, relation(rel, *right)?);
            // `UNION ALL` pairs by position and names the result after the
            // left side, so each side is re-qualified under its own names.
            let (left_select, cols) = requalify(l, name_);
            let (right_select, _) = requalify(r, name_);
            (
                cols,
                format!(
                    "SELECT {left_select} FROM {} UNION ALL SELECT {right_select} FROM {}",
                    ident(&l.name),
                    ident(&r.name)
                ),
            )
        }
        Op::GroupBy {
            input,
            keys,
            aggs,
            relation: name_,
        } => {
            let from = relation(rel, *input)?;
            let mut cols = Vec::new();
            let mut select = Vec::new();
            let mut group = Vec::new();
            for k in keys {
                let c = from.column(&k.source, &k.column)?;
                group.push(c.clone());
                select.push(c);
                cols.push((k.source.clone(), k.column.clone()));
            }
            for a in aggs {
                let arg = from.column(&a.column.source, &a.column.column)?;
                let f = match a.agg_fn {
                    AggFn::Sum => "sum",
                    AggFn::Avg => "avg",
                    AggFn::Min => "min",
                    AggFn::Max => "max",
                };
                let prim = fossil_hir::shapes::inner_primitive(db, a.ty);
                select.push(format!(
                    "{} AS {}",
                    typed(&format!("{f}({arg})"), prim),
                    ident(&qualified(name_, &a.out_field))
                ));
                cols.push((name_.clone(), a.out_field.clone()));
            }
            let by = if group.is_empty() {
                String::new()
            } else {
                format!(" GROUP BY {}", group.join(", "))
            };
            (
                cols,
                format!(
                    "SELECT {} FROM {}{by}",
                    select.join(", "),
                    ident(&from.name)
                ),
            )
        }
        // `fila(izq) ++ fila(der)`: every column of both sides, under the names
        // each side is addressed by; an alias re-qualifies its side.
        Op::Join {
            left,
            right,
            on,
            kind,
        } => {
            let sql_kind = match kind {
                JoinKind::Inner => "INNER",
                JoinKind::LeftOuter => "LEFT",
                JoinKind::RightOuter => "RIGHT",
                JoinKind::Full => "FULL",
            };
            let side =
                |s: &fossil_mir::JoinSide| -> Result<(String, Vec<(SmolStr, SmolStr)>), Refusal> {
                    let from = relation(rel, s.input)?;
                    let Some(alias) = &s.alias else {
                        return Ok((ident(&from.name), from.cols.clone()));
                    };
                    let (select, cols) = requalify(from, alias);
                    Ok((
                        format!("(SELECT {select} FROM {})", ident(&from.name)),
                        cols,
                    ))
                };
            let (lsql, lcols) = side(left)?;
            let (rsql, rcols) = side(right)?;
            let mut cols = lcols;
            cols.extend(rcols);
            let joined = Relation {
                name: name.to_string(),
                cols: cols.clone(),
            };
            (
                cols,
                format!(
                    "SELECT * FROM {lsql} AS __l {sql_kind} JOIN {rsql} AS __r ON {}",
                    expr(on, &joined)?
                ),
            )
        }
        Op::Empty { schema } => (
            schema
                .iter()
                .map(|c| (SmolStr::default(), c.clone()))
                .collect(),
            format!(
                "SELECT {} WHERE false",
                if schema.is_empty() {
                    "NULL AS __empty".to_string()
                } else {
                    schema
                        .iter()
                        .map(|c| format!("NULL AS {}", ident(&qualified("", c))))
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            ),
        ),
        Op::EmitVertex { .. } | Op::EmitEdge { .. } | Op::Sink { .. } => {
            return Err(Refusal::Unsupported("an emit op is not a relation".into()));
        }
    };
    q.cte(name, body);
    Ok(Relation {
        name: name.to_string(),
        cols,
    })
}

/// The table function that reads a source, its columns typed as the program
/// types them.
fn reader(
    location: &str,
    format: &SourceFormat,
    fields: &[(SmolStr, Option<Primitive>)],
) -> Result<String, Refusal> {
    let columns = fields
        .iter()
        .map(|(f, p)| format!("{}: {}", string(f), string(duck_type(*p))))
        .collect::<Vec<_>>()
        .join(", ");
    Ok(match format {
        SourceFormat::Csv { delimiter } => {
            let delim = delimiter
                .as_ref()
                .map(|d| format!(", delim = {}", string(d)))
                .unwrap_or_default();
            format!(
                "read_csv({}, header = true, columns = {{{columns}}}{delim})",
                string(location)
            )
        }
        SourceFormat::Json => format!("read_json({}, columns = {{{columns}}})", string(location)),
        SourceFormat::Parquet => format!("read_parquet({})", string(location)),
        SourceFormat::Provider { name } => {
            return Err(Refusal::Unsupported(format!(
                "`io.{name}` is read by a provider, and this backend reads no provider"
            )));
        }
    })
}

/// The `DuckDB` type a value of a fossil primitive is held in.
const fn duck_type(p: Option<Primitive>) -> &'static str {
    use fossil_graph_schema::Precision;
    match p {
        Some(Primitive::Integer | Primitive::Time(Precision::Year)) => "BIGINT",
        Some(Primitive::Float) => "DOUBLE",
        Some(Primitive::Bool) => "BOOLEAN",
        Some(Primitive::Time(Precision::Date)) => "DATE",
        Some(Primitive::Time(Precision::DateTime)) => "TIMESTAMP",
        Some(Primitive::Time(Precision::Hour)) => "TIME",
        Some(Primitive::String(_) | Primitive::Uri) | None => "VARCHAR",
    }
}

/// `e` cast to the type a primitive is held in — the CAST that makes a column
/// the program's type and not the engine's guess. A tagged text is a struct,
/// and is left as `str.lang` built it.
fn typed(e: &str, p: Option<Primitive>) -> String {
    match p {
        Some(Primitive::String(fossil_graph_schema::Lang::Tagged)) | None => e.to_string(),
        p => format!("CAST({e} AS {})", duck_type(p)),
    }
}

// ─────────────────────────────────────────────────────────────── vertices

struct VertexType {
    label: String,
    class: Option<String>,
    dedup: bool,
    parts: Vec<Part>,
}

/// One mapping's contribution to a vertex type: its CTE and the properties it
/// writes.
struct Part {
    cte: String,
    props: Vec<Prop>,
}

#[derive(Clone)]
struct Prop {
    name: String,
    predicate: Option<String>,
    primitive: Option<Primitive>,
    /// The datatype IRI, or `None` for an IRI object.
    datatype: Option<String>,
    single: bool,
}

impl Prop {
    fn of(p: &VProp<'_>, primitive: Option<Primitive>) -> Self {
        let term = p
            .term
            .clone()
            .unwrap_or_else(|| Term::of(primitive.unwrap_or(Primitive::STRING)));
        let datatype = match term {
            Term::Iri => None,
            Term::Literal(Some(x)) => Some(x.as_str().to_string()),
            Term::Literal(None) => Some(
                Xsd::try_from(primitive.unwrap_or(Primitive::STRING)).map_or_else(
                    |_| "http://www.w3.org/2001/XMLSchema#string".to_string(),
                    |x| x.as_str().to_string(),
                ),
            ),
        };
        Self {
            name: p.name.to_string(),
            predicate: p.rdf_uri.as_ref().map(ToString::to_string),
            primitive,
            datatype,
            single: p.single_valued,
        }
    }
}

/// A vertex type's subjects and its triples: the union of its mappings'
/// rows, merged per subject for a single-valued property, refused when a
/// subject is given two values of one.
fn vertex(t: &VertexType, q: &mut Query, triples: &mut Vec<String>) {
    let mut props: Vec<Prop> = Vec::new();
    for part in &t.parts {
        for p in &part.props {
            if !props.iter().any(|x| x.name == p.name) {
                props.push(p.clone());
            }
        }
    }
    // Every part aligned by name, a NULL where a mapping does not write one.
    let rows = format!("rows:{}", t.label);
    let union = t
        .parts
        .iter()
        .map(|part| {
            let cols = std::iter::once(ident("subject"))
                .chain(props.iter().map(|p| {
                    if part.props.iter().any(|x| x.name == p.name) {
                        ident(&p.name)
                    } else {
                        format!("NULL AS {}", ident(&p.name))
                    }
                }))
                .collect::<Vec<_>>()
                .join(", ");
            format!("SELECT {cols} FROM {}", ident(&part.cte))
        })
        .collect::<Vec<_>>()
        .join(" UNION ALL ");
    q.cte(&rows, union);
    q.cte(
        &format!("subjects:{}", t.label),
        format!(
            "SELECT DISTINCT subject FROM {} WHERE subject IS NOT NULL",
            ident(&rows)
        ),
    );

    // A single-valued property of a deduplicated type: the one value its rows
    // offer, or the run fails naming the subject and both values.
    let merged = format!("merged:{}", t.label);
    let single: Vec<&Prop> = props.iter().filter(|p| t.dedup && p.single).collect();
    if !single.is_empty() {
        let cols = single
            .iter()
            .map(|p| {
                let c = ident(&p.name);
                format!(
                    "CASE WHEN min({c}) IS DISTINCT FROM max({c}) THEN error('`{}.{}`: ' || subject || ' has two values, ' || CAST(min({c}) AS VARCHAR) || ' and ' || CAST(max({c}) AS VARCHAR)) ELSE min({c}) END AS {c}",
                    t.label.replace('\'', "''"),
                    p.name.replace('\'', "''"),
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        q.cte(
            &merged,
            format!(
                "SELECT subject, {cols} FROM {} WHERE subject IS NOT NULL GROUP BY subject",
                ident(&rows)
            ),
        );
    }

    let label = string(&t.label);
    if let Some(class) = &t.class {
        triples.push(format!(
            "SELECT subject AS s, {label} AS type, {} AS p, {} AS o, NULL AS datatype, NULL AS lang FROM {}",
            string(RDF_TYPE),
            string(class),
            ident(&format!("subjects:{}", t.label)),
        ));
    }
    for p in &props {
        let Some(predicate) = &p.predicate else {
            continue;
        };
        let from = if t.dedup && p.single { &merged } else { &rows };
        let v = ident(&p.name);
        let (o, datatype, lang) = if p.primitive == Some(Primitive::LANG_STRING) {
            (
                format!("struct_extract_at({v}, 1)"),
                string(RDF_LANG_STRING),
                format!("lower(struct_extract_at({v}, 2))"),
            )
        } else {
            (
                lexical(&v, p.primitive),
                p.datatype
                    .as_deref()
                    .map_or_else(|| "NULL".to_string(), string),
                "NULL".to_string(),
            )
        };
        triples.push(format!(
            "SELECT subject AS s, {label} AS type, {} AS p, {o} AS o, {datatype} AS datatype, {lang} AS lang FROM {} WHERE subject IS NOT NULL AND {o} IS NOT NULL",
            string(predicate),
            ident(from),
        ));
    }
}

/// A value's lexical form: its text, a timestamp with the `T` of
/// `xsd:dateTime`.
fn lexical(v: &str, p: Option<Primitive>) -> String {
    match p {
        Some(Primitive::Time(fossil_graph_schema::Precision::DateTime)) => {
            format!("replace(CAST({v} AS VARCHAR), ' ', 'T')")
        }
        Some(Primitive::String(_) | Primitive::Uri) => v.to_string(),
        _ => format!("CAST({v} AS VARCHAR)"),
    }
}

struct Edge {
    cte: String,
    predicate: Option<String>,
    src_type: String,
    dst_type: String,
}

// ─────────────────────────────────────────────────────────────── expressions

/// A MIR expression over the columns of `from`, as SQL.
fn expr(e: &Expr<'_>, from: &Relation) -> Result<String, Refusal> {
    Ok(match e {
        Expr::LitString(s) => string(s),
        Expr::LitBool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
        Expr::LitInt(v) => format!("CAST({v} AS BIGINT)"),
        Expr::LitFloat(v) => format!("CAST({:?} AS DOUBLE)", v.get()),
        Expr::ColRef { source, column } => from.column(source, column)?,
        Expr::Concat(a, b) => format!("({} || {})", expr(a, from)?, expr(b, from)?),
        Expr::IsNull { operand, negated } => format!(
            "({} IS {}NULL)",
            expr(operand, from)?,
            if *negated { "NOT " } else { "" }
        ),
        Expr::Call { func, args, .. } => {
            let entry = fossil_hir::stdlib::stdlib()
                .lookup(func)
                .ok_or_else(|| Refusal::Unsupported(format!("`{func}` is not catalogued")))?;
            let LoweringKind::Expr(template) = &entry.lowering else {
                return Err(Refusal::Unsupported(format!(
                    "`{func}` is an operator, not a value"
                )));
            };
            let args = args
                .iter()
                .map(|a| expr(a, from))
                .collect::<Result<Vec<_>, _>>()?;
            format!("({})", fossil_hir::stdlib::render_template(template, &args))
        }
        // `/` is Float in fossil whatever its operands.
        Expr::BinOp {
            op: BinOp::Div,
            lhs,
            rhs,
            ..
        } => format!(
            "(CAST({} AS DOUBLE) / {})",
            expr(lhs, from)?,
            expr(rhs, from)?
        ),
        Expr::BinOp { op, lhs, rhs, .. } => {
            let o = match op {
                BinOp::Eq => "=",
                BinOp::Ne => "<>",
                BinOp::Lt => "<",
                BinOp::Le => "<=",
                BinOp::Gt => ">",
                BinOp::Ge => ">=",
                BinOp::And => "AND",
                BinOp::Or => "OR",
                BinOp::Add => "+",
                BinOp::Sub => "-",
                BinOp::Mul => "*",
                BinOp::Div => "/",
                BinOp::Rem => "%",
            };
            format!("({} {o} {})", expr(lhs, from)?, expr(rhs, from)?)
        }
        Expr::UnaryOp { op, operand, .. } => match op {
            UnOp::Neg => format!("(-{})", expr(operand, from)?),
            UnOp::Not => format!("(NOT {})", expr(operand, from)?),
        },
        Expr::Ternary {
            cond,
            then,
            otherwise,
            ..
        } => format!(
            "(CASE WHEN {} THEN {} ELSE {} END)",
            expr(cond, from)?,
            expr(then, from)?,
            expr(otherwise, from)?
        ),
        Expr::Assert { inner, .. } => expr(inner, from)?,
    })
}

/// A quoted identifier.
fn ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// A string literal.
fn string(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

// ─────────────────────────────────────────────────────────────── shapes

fn shapes(db: &dyn fossil_base::Db, file: SourceFile) -> Vec<ShapeDocument> {
    fossil_hir::documents::documents_named(db, file)
        .into_iter()
        .filter_map(|document| {
            let location = fossil_hir::documents::registry_key(db, file, &document);
            let text = fossil_base::file_at(db, &location)?.text(db).clone();
            Some(ShapeDocument { location, text })
        })
        .collect()
}
