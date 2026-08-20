//! `fossil-engine` — the native orchestration surface behind the `fossil`
//! binaries. It owns the compile→run pipeline (parse → `def_map` → typecheck →
//! `lower_to_mir` → `decompose_for_writer` → materialise `GraphAr`) plus `check` /
//! `refs` / `providers`, returning STRUCTURED data. The binary
//! (`fossil-cli`) is a thin shell: it parses args, reads files/stdin, calls these
//! functions, and renders the result (rustc-style miette for `check`, JSON/human
//! for the rest). Mirrors the rust-analyzer `ide`-façade / biome `service`
//! pattern — the orchestration is a named, testable crate, not a binary monolith.

#[cfg(target_arch = "wasm32")]
compile_error!(
    "fossil-engine is native-only (depends on fossil-runtime which uses bundled DuckDB); \
     do not add it to the WASM CI gate"
);

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use fossil_base::{Db, Diagnostic, Severity, SourceAnchor, Span, System};
use fossil_descriptors_input::{InferredColumn, InferredDescriptor};
use fossil_descriptors_output::OutputDescriptorKind;
use fossil_graph_schema::Primitive;
use fossil_run_status::{ProviderInfo, RunStatus, SourceRefInfo};
use smol_str::SmolStr;

pub mod census;
pub mod creds;
mod documents;
mod system;

pub use creds::RunCreds;
use system::open_db;

// ===================================================================== providers

/// List the data-source providers fossil supports. Thin native wrapper over
/// [`fossil_lineage::providers`] (the shared, WASM-clean implementation — one
/// source of truth for both the CLI and the browser).
#[must_use]
pub fn providers() -> Vec<ProviderInfo> {
    // The engine's own table, which is the whole registry — so `fossil
    // providers` now lists `shex` and `shacl` as `Schema` rows beside the four
    // `Data` ones. The wire contract has carried that distinction since it was
    // written and nothing ever produced anything but `Data`.
    fossil_lineage::providers(fossil_descriptors_output::PROVIDERS)
}

// ========================================================================= refs

/// Parse a program and return its external references (data URI + `schema =` for
/// every source), each tagged with the `@conn` alias it targets (or `None` for a
/// direct URL/path). Parse-only — the typed lineage keasy reads to derive a job's
/// connection set without scanning script text.
///
/// # Errors
/// Returns a read error if `path` is unreadable.
pub fn refs(path: &Path) -> miette::Result<Vec<SourceRefInfo>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e}", path.display()))?;
    let (db, file) = open_db(text, path);
    // The native host reads the file; the lineage logic (parse → typed refs,
    // dedup) is the shared WASM-clean `fossil_lineage::source_refs` — same code
    // the browser runs over its in-memory db.
    Ok(fossil_lineage::source_refs(&db, file))
}

// ======================================================================== check

/// The result of [`check`]: the source text (for rendering spans) + every
/// accumulated diagnostic. The caller decides how to render and whether to fail
/// (exit non-zero iff any [`fossil_base::Severity::Error`] is present).
#[derive(Debug)]
pub struct CheckOutcome {
    pub source: String,
    pub diagnostics: Vec<Diagnostic>,
    /// How many mappings the file defines. Zero with no diagnostics is a
    /// program that parses and builds nothing — the caller renders that
    /// differently from a clean program, because it is not the same answer.
    pub mappings: usize,
}

/// Parse + type-check + lower `path`, draining the Salsa `Diagnostic`
/// accumulator across every mapping. Same pre-introspection as [`run`] so
/// `check` sees the same forward-propagated types the compiler will (no `@conn`
/// creds on `check`).
///
/// Drains from `lower_to_mir_pg` rather than `typecheck_mapping`: Salsa
/// accumulators are transitive, and lowering calls the typechecker, so this
/// yields the typecheck diagnostics PLUS the lowering ones without duplicating
/// either. Draining only the typechecker used to make `check` report `ok` for a
/// program `run` then refused — e.g. a mapping reading `from` a derived binding,
/// whose source cannot be resolved. `check` must not pass what `run` rejects.
///
/// A file with NO mappings is drained at the file level instead — see the body.
/// Zero mappings and zero diagnostics is reported as success, not as an error:
/// `check` answers "is this text a well-formed program", and a program that
/// declares nothing is vacuously well-formed. `run` refuses it (`no mapping
/// found in …`) because `run` was asked to produce a graph and there is nothing
/// to produce it from — a different question, asked of a different command. So
/// the count rides out on [`CheckOutcome::mappings`] and the caller says so
/// plainly rather than claiming a clean bill of health it did not earn.
///
/// # Errors
/// Returns a read error if `path` is unreadable.
pub fn check(path: &Path) -> miette::Result<CheckOutcome> {
    tracing::debug!(?path, "fossil check");
    let text = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e}", path.display()))?;

    let (db, file) = open_db(text.clone(), path);
    // `check` has no `--creds-stdin`, so it resolves with no connection map —
    // and against the SAME directory `run` will. The two used to differ here:
    // `check` pre-introspected against `path.parent()` while the executor read
    // against the process's cwd, so the two commands disagreed about where one
    // file was.
    let program_dir = fossil_base::program_dir(&path.to_string_lossy());
    let anchor = SourceAnchor::beside(&program_dir);
    pre_introspect_and_register(db.system(), &text, anchor, &HashMap::new());

    let def_map = fossil_hir::def_map::def_map(&db, file);
    let mappings = def_map.mappings(&db);

    // ── The FILE-level queries, drained once and unconditionally ──────────
    //
    // A file that yields no mapping — empty, or broken badly enough that the
    // parser recovered nothing — used to report `ok`, because the per-mapping
    // loop was the ONLY thing that read an accumulator and it never ran. The
    // parse errors were not missing; they were unreachable, because salsa
    // collects an accumulator over a query's whole dependency subtree and
    // `parse` is only in the subtree of a mapping.
    //
    // These two drains were guarded on `mappings.is_empty()`, which is where
    // the second half of the bug lived: `lower_to_hir` is where a top-level
    // BINDING is checked (`check_provider`, `check_schema_arg`), and it is not
    // in `def_map`'s subtree — so a file with no mapping lost every provider
    // diagnostic it produced, silently, which is the failure this whole change
    // is against. Both are file-keyed, so draining them always is correct and
    // the duplicates it creates are removed below.
    //
    // Their spans are file-absolute — the parser and `lower_to_hir` both
    // measure against the file — which is why nothing is rebased here.
    let mut diagnostics: Vec<Diagnostic> =
        fossil_hir::def_map::def_map::accumulated::<Diagnostic>(&db, file)
            .into_iter()
            .cloned()
            .collect();
    diagnostics.extend(
        fossil_hir::lower::lower_to_hir::accumulated::<Diagnostic>(&db, file)
            .into_iter()
            .cloned(),
    );
    // One identity per type: every mapping that produces `T` declares the same
    // `@subject`, and two that disagree are an ERROR — never a warning —
    // naming both mappings and both templates, because a warning about
    // identity gets ignored and the result is two entities where there was one.
    //
    // It is a third file-level drain and not a fourth per-mapping one because
    // uniqueness is a fact about the FILE:
    // `body::check_identity` is keyed by `MappingLoc` and by construction cannot
    // see a second mapping. Its own spans are file-absolute — it rebased both of
    // them itself, being the only party that holds two mappings at once.
    //
    // FILTERED to the file-absolute ones, and that is not a nicety. Unlike the
    // two drains above, this query sits BELOW `body`: salsa accumulates over the
    // whole dependency subtree, so draining it unfiltered also yields every
    // mapping-relative diagnostic every body produced — raw, while the loop
    // below yields the same ones REBASED. Two spans, so `dedup_file_level`
    // cannot see them as one, and the raw copy points at whatever sits at that
    // offset from the start of the file. A mapping-relative diagnostic has an
    // owner and this drain is not it.
    let _ = fossil_hir::identity::check_identities(&db, file);
    diagnostics.extend(
        fossil_hir::identity::check_identities::accumulated::<Diagnostic>(&db, file)
            .into_iter()
            .filter(|d| d.frame == fossil_base::SpanFrame::FileAbsolute)
            .cloned(),
    );

    for mapping in mappings {
        let _ = fossil_mir::lower_to_mir_pg(&db, *mapping);
        let diags = fossil_mir::lower_to_mir_pg::accumulated::<Diagnostic>(&db, *mapping);
        // Spans come out mapping-relative; the caller renders against the file.
        diagnostics.extend(fossil_hir::spans::rebase_to_file(
            &db,
            *mapping,
            diags.into_iter().cloned(),
        ));
    }
    dedup_file_level(&mut diagnostics);
    Ok(CheckOutcome {
        source: text,
        diagnostics,
        mappings: mappings.len(),
    })
}

/// Drop repeats, keeping the first of each.
///
/// **Salsa accumulates over a query's whole dependency subtree**, and every
/// mapping's subtree contains the two FILE-keyed queries above. So a diagnostic
/// about a top-level binding — `type { P } := io.csv("users.csv")` — comes out
/// once per mapping, and a program with ten mappings reported one mistake ten
/// times. It is not a per-mapping fact and there is no mapping to attribute it
/// to.
///
/// The key is `(severity, message, span)`, and each part is load-bearing. Two
/// diagnostics with one message at two spans are two mistakes and both survive
/// — which is why this is not a `message`-only dedup. Two with one message at
/// ONE span are one statement about one range of bytes, and printing it twice
/// is noise by construction, whichever query emitted it.
///
/// It runs over the whole list rather than only the file-level drains because
/// the per-mapping path is where the duplicates actually arrive: they are the
/// file-level ones, carried along by `lower_to_mir_pg::accumulated`, and there
/// is nothing at that point marking which is which.
fn dedup_file_level(diagnostics: &mut Vec<Diagnostic>) {
    let mut seen: std::collections::HashSet<(Severity, String, Span)> =
        std::collections::HashSet::new();
    diagnostics.retain(|d| seen.insert((d.severity, d.message.clone(), d.span)));
}

// =============================================================== pre-introspection

/// Map a `DuckDB` column-type string onto the lattice — the native sibling of
/// `@fossil-lang/introspect`'s `duckdbTypeToFossilPrimitive`. A vocabulary the
/// engine reads and nobody else does, which is why it lives here and not on
/// [`Primitive`]; the xsd direction is the one the lattice owns.
fn duckdb_type_to_fossil_primitive(t: &str) -> Primitive {
    let upper = t.trim().to_ascii_uppercase();
    match upper.as_str() {
        "INTEGER" | "BIGINT" | "INT" | "SMALLINT" | "TINYINT" | "HUGEINT" => Primitive::Integer,
        "DOUBLE" | "FLOAT" | "REAL" => Primitive::Float,
        t if t.starts_with("DECIMAL") => Primitive::Float,
        "BOOLEAN" | "BOOL" => Primitive::Bool,
        "DATE" => Primitive::Date,
        "TIMESTAMP" | "DATETIME" => Primitive::DateTime,
        "TIME" => Primitive::Time,
        _ => Primitive::String,
    }
}

/// Scrape source-binding RHS source URLs from a `.fossil` file's text. It is a
/// regex placeholder for an AST walk, and it is wrong on any binding the regex
/// cannot see.
///
/// `@fossil-lang/introspect` scrapes the same bindings for the browser, and
/// the regex below plus the reader each constructor picks are read out of THIS
/// FILE by `packages/introspect/tests/rust-parity.test.ts`. Editing either
/// here turns that test red until the TypeScript follows; it is a `pnpm` test,
/// so `cargo test` will not tell you.
fn extract_source_refs(text: &str) -> Vec<(SmolStr, SmolStr, String)> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r#"(\w[\w\d_]*)\s*:=\s*io\.(csv|json|parquet)\(\s*['"]([^'"]+)['"]"#)
            .expect("static regex")
    });
    re.captures_iter(text)
        .map(|c| {
            (
                SmolStr::from(c.get(1).unwrap().as_str()),
                SmolStr::from(c.get(2).unwrap().as_str()),
                c.get(3).unwrap().as_str().to_string(),
            )
        })
        .collect()
}

/// The token that decides whether a cached descriptor still describes its
/// source: the file's modification time in nanoseconds since the epoch, paired
/// with its byte length. Two `stat` fields, no read of the source itself: the
/// native host does NOT hash the bytes because hashing means reading the whole
/// source to decide whether the source needs reading — hundreds of megabytes
/// to save a `DESCRIBE` that reads the first rows, which would make the cache
/// cost more than the thing it caches. `mtime` can say "changed" when nothing
/// did (a `touch`), which costs one extra `DESCRIBE` and no wrong answer;
/// pairing it with the size is what narrows the one case that *is* wrong, a
/// file restored with both its old `mtime` and its exact old length.
///
/// Returns `""` for anything this host cannot `stat` — an `http(s)://` or
/// `s3://` locator, or a path that does not exist. An empty token is never
/// fresh ([`fossil_descriptors_input::DescriptorCache::is_fresh`]), so those are
/// re-introspected on every compile. That is the honest answer for an object we
/// would have to make a network round trip to interrogate.
fn freshness_token(resolved: &str) -> String {
    let Ok(meta) = std::fs::metadata(resolved) else {
        return String::new();
    };
    let Ok(modified) = meta.modified() else {
        return String::new();
    };
    let Ok(since_epoch) = modified.duration_since(std::time::SystemTime::UNIX_EPOCH) else {
        return String::new();
    };
    format!(
        "mtime:{}.{:09}:size:{}",
        since_epoch.as_secs(),
        since_epoch.subsec_nanos(),
        meta.len()
    )
}

/// Pre-introspect every source the program names and register an
/// [`InferredDescriptor`] on the host's descriptor cache BEFORE typecheck,
/// keyed by the URI the program writes rather than by the resolved locator —
/// the written URI is the only string the host and the checker both see.
///
/// A source whose cached descriptor still carries the current
/// [`freshness_token`] is skipped — no `DESCRIBE`, no read. That is where the
/// cost is: programs are small and sources are not, so the introspection is
/// the expensive half of a compile and it is the half that rarely needs doing
/// twice.
///
/// Per-source failures are non-fatal — they log + skip; the compile may still
/// succeed with no forward propagation for that source.
fn pre_introspect_and_register(
    system: &dyn System,
    source_text: &str,
    anchor: SourceAnchor<'_>,
    connections: &HashMap<String, creds::ConnectionCreds>,
) {
    let Some(cache) = system.descriptors() else {
        tracing::debug!("host keeps no descriptor cache; skipping pre-introspection");
        return;
    };

    // Opened on the first miss, not on entry. A compile whose sources are all
    // fresh must do no DuckDB work at all, and opening a connection is work.
    let mut conn: Option<duckdb::Connection> = None;

    for (source_name, constructor, raw_uri) in extract_source_refs(source_text) {
        let token = freshness_token(&anchor.locator(&raw_uri));
        if cache.is_fresh(&raw_uri, &token) {
            tracing::debug!("`{raw_uri}` is unchanged since it was introspected; reusing");
            continue;
        }

        let conn = if let Some(c) = &conn {
            c
        } else {
            let opened = match duckdb::Connection::open_in_memory() {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("DuckDB in-memory open failed; skipping pre-introspection: {e}");
                    return;
                }
            };
            if let Err(e) = apply_source_creds(&opened, connections) {
                tracing::warn!("applying source creds for pre-introspection failed: {e}");
            }
            conn.insert(opened)
        };

        // Resolved a second time deliberately: the token above is about the
        // bytes on disk, this is the string DuckDB reads, and conflating them
        // would make a `@conn` alias silently change meaning between the two.
        let resolved_path = anchor.locator(&raw_uri);
        let escaped_path = resolved_path.replace('\'', "''");
        // The CONSTRUCTOR chooses the reader, and it used to not: every source
        // was `read_csv_auto` whatever `io.` said. A JSON array read as CSV
        // introspects to one column named after the first line, so
        // `data/sightings.json` — which opens with a bare `[` — produced a
        // schema whose only column was literally `[`, and every real column
        // came back as `unknown column \`id\` — did you mean \`[\`?`. The
        // did-you-mean is what made it legible: it printed the wrong schema.
        let reader = match constructor.as_str() {
            "json" => "read_json_auto",
            "parquet" => "read_parquet",
            _ => "read_csv_auto",
        };
        let sql = format!("DESCRIBE SELECT * FROM {reader}('{escaped_path}')");
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    "DESCRIBE prepare failed for source `{source_name}` (uri=`{raw_uri}`): {e}"
                );
                continue;
            }
        };
        let cols: Vec<InferredColumn> = match stmt.query_map([], |row| {
            let name: String = row.get(0)?;
            let typ: String = row.get(1)?;
            Ok(InferredColumn {
                name: SmolStr::from(name),
                primitive: duckdb_type_to_fossil_primitive(&typ),
            })
        }) {
            Ok(iter) => iter.filter_map(Result::ok).collect(),
            Err(e) => {
                tracing::warn!("DESCRIBE query_map failed for `{source_name}`: {e}");
                continue;
            }
        };
        cache.insert(InferredDescriptor {
            uri: SmolStr::from(raw_uri.as_str()),
            columns: cols,
            freshness_token: token,
        });
        tracing::debug!("introspected `{raw_uri}` for source `{source_name}`");
    }
}

// ================================================================== run pipeline

/// Resolve the program-resident OUTPUT descriptor. The shape is sourced from the
/// PROGRAM, never a host flag (invariant #1).
///
/// # Two places a program can name its output shape, and the order between them
///
/// 1. **`type { … } := io.shex("shop.shex")`** — the binding ruling 3 of
///    2026-08-11 makes MANDATORY. It is what the CHECKER reads
///    ([`fossil_hir::def_map::DefMap::output_shape_document`]), and it is
///    consulted FIRST.
/// 2. `io.rdf(schema = …)` — an RDF *input* whose `ShEx` doubles as the output
///    contract. It stays as the fallback for a program that reads a graph and
///    writes one back.
///
/// **Only (2) existed here, and that was a hole between step 4 and step 7 of
/// `SURFACE-PLAN.md`.** A pure-`io.csv` program — every program in
/// `apps/docs/programs/` bar one — got `ACCEPT_ALL_DEFAULT`, and
/// `fossil_mir::apply_output_shape` against an empty schema returns the ops
/// unchanged: **zero `Op::EmitEdge`, for every CSV program there is**. The
/// tombstone `fossil-mir/src/lower.rs` left when `subject_skeletons` was deleted
/// says the shape classifies edges «and since ruling 3 that path is always
/// available»; it was available to the checker and not to the executor, so
/// deleting the skeletons did not migrate the edge capability, it dropped it.
/// This function is the other half of that ruling: the document a program is
/// REQUIRED to name is the document the run classifies edges with.
///
/// v1: one shape per program (a second, different `io.rdf` schema is rejected,
/// not merged).
fn resolve_output_descriptor(
    db: &fossil_base::FossilDb,
    def_map: fossil_hir::def_map::DefMap<'_>,
    anchor: SourceAnchor<'_>,
) -> miette::Result<OutputDescriptorKind> {
    // The `type { … } := io.shex(…)` binding, and it wins: it is the one the
    // checker resolved the mapping's target shape against, so preferring it is
    // what keeps «what compiled» and «what ran» the same document. The
    // CONSTRUCTOR travels with it — ruling 13 — because it is what selects the
    // row that reads it, and reading a document with a row the program did not
    // name is how the run came to use a different parser from the check.
    // The program's `@rename`s travel with the document, because they decide
    // the emitted column's name. Read off the same `def_map` — and read HERE
    // rather than inside the decode, so the one place that has the program is
    // the one place that supplies them.
    let renames = def_map.renames(db);

    if let Some((constructor, document)) = def_map.output_shape_binding(db) {
        return read_output_shape(constructor.as_deref(), document.as_str(), anchor, &renames);
    }

    // The `schema =` argument carries its OWN provider now
    // (`schema = io.shex("x.shex")`), so the pair travels together here exactly
    // as the `type { … }` pair does above — there is no position left where a
    // document arrives without the row that reads it.
    let mut schema: Option<(Option<SmolStr>, SmolStr)> = None;
    for s in def_map.sources(db) {
        let is_provider = s
            .constructor
            .as_deref()
            .and_then(|c| fossil_base::provider(fossil_descriptors_output::PROVIDERS, c))
            .is_some_and(|p| p.reads_rows == Some(fossil_base::RowReader::Materialised));
        if !is_provider {
            continue;
        }
        let Some(arg) = s.schema_arg.as_ref() else {
            continue;
        };
        match &schema {
            Some((_, existing)) if existing != arg => {
                return Err(miette::miette!(
                    "a program may declare only one io.rdf output shape (v1); found `{existing}` and `{arg}`"
                ));
            }
            _ => schema = Some((s.schema_provider.clone(), arg.clone())),
        }
    }

    let Some((provider, schema)) = schema else {
        return Ok(OutputDescriptorKind::ACCEPT_ALL_DEFAULT);
    };

    read_output_shape(provider.as_deref(), schema.as_str(), anchor, &renames)
}

/// Read and decode one shape document into the run's output descriptor,
/// **through the registry row the program named** — the same seam the checker
/// goes through.
///
/// # What this was, and why it was a bug nobody could see from one side
///
/// ```ignore
/// let desc = fossil_shex::ShExDescriptor::from_reader(text.as_bytes())?;   // was
/// ```
///
/// `from_reader` is **`ShExJ` (JSON) and only `ShExJ`**. Every `.shex` in
/// `apps/docs/programs/` is `ShExC`. The checker's path goes
/// `decoded_document` → `shape_document` → the `shex` row → `from_shex_source`,
/// which auto-detects both. So a document that **type-checked** made the `run`
/// fail on the same bytes, and neither side was wrong on its own — which is
/// exactly how it survived (`SURFACE-PLAN.md` §B′).
///
/// It also meant the run had a hard-coded language: a program naming
/// `io.shacl("catalogue.ttl")` was handed to a `ShEx` parser.
///
/// Both are one fix. The row is selected by the constructor the program wrote,
/// its `reads_types` is the same `fn` the checker calls, and what comes back is
/// [`fossil_graph_schema::OutputShapes`] — so «what compiled» and «what ran»
/// are now the same decode of the same bytes by construction, not by two
/// implementations agreeing.
///
/// The descriptor is [`OutputDescriptorKind::Lowered`] whatever the language —
/// the variant was called `Shacl`, and the rename is part of this: it never
/// meant SHACL, it meant "the decode already happened". The rich `ShEx`
/// resolved table it replaces was only ever read by the checker, which does not
/// come through here; the executor reads `to_graph_schema` and nothing else.
fn read_output_shape(
    constructor: Option<&str>,
    document: &str,
    anchor: SourceAnchor<'_>,
    renames: &fossil_graph_schema::Renames,
) -> miette::Result<OutputDescriptorKind> {
    use fossil_base::providers::{Capability, provider};

    let table = fossil_descriptors_output::PROVIDERS;
    let ctor = constructor.ok_or_else(|| {
        miette::miette!(
            "the shape document `{document}` is named by no provider — write \
             `io.shex(\"…\")` or `io.shacl(\"…\")`"
        )
    })?;
    let row = provider(table, ctor)
        .ok_or_else(|| miette::miette!("`{ctor}` is not a provider this host installs"))?;
    if !row.provides(Capability::ReadTypes) {
        return Err(miette::miette!(
            "{}",
            row.decline_capability(Capability::ReadTypes, table)
        ));
    }
    if !row.accepts(document) {
        return Err(miette::miette!("{}", row.decline_extension(document)));
    }
    let decode = row
        .reads_types
        .ok_or_else(|| miette::miette!("`{}` reads no types", row.constructor()))?;

    // The one resolution rule, and the same one the CHECKER went through to
    // read this document (`fossil_hir::def_map::resolve_relative`). A run that
    // anchored differently would decode a different file from the one that
    // type-checked, which is the same class of bug as decoding it with a
    // different parser.
    let locator = anchor.locator(document);
    let text = std::fs::read_to_string(&locator)
        .map_err(|e| miette::miette!("read output shape document `{locator}`: {e}"))?;
    let shapes = decode(&locator, &text)
        .map_err(|e| miette::miette!("parse output shape document `{locator}`: {e:?}"))?;
    Ok(OutputDescriptorKind::Lowered(
        shapes.to_graph_schema(renames),
    ))
}

/// The name→base-URL view of the run's connections — what
/// [`fossil_base::SourceAnchor`] expands a `@conn` alias through, and what the
/// executor is handed for the same purpose.
///
/// Projected ONCE per command and then borrowed, rather than rebuilt inside a
/// per-URI resolver: the map was cloned for every source of every program, and
/// a second copy of it was built again at the executor seam. One projection is
/// also what lets the anchor be a borrow — the pair (directory, connections)
/// has to outlive every resolution done against it, which is exactly the
/// lifetime of the command.
fn connection_urls(
    connections: &HashMap<String, creds::ConnectionCreds>,
) -> HashMap<String, String> {
    connections
        .iter()
        .map(|(name, c)| (name.clone(), c.url.clone()))
        .collect()
}

/// Install each source connection's scoped read secret on `conn`, so a
/// `read_csv_auto` over a cloud `@conn` source authenticates. No-op for
/// connections without a secret (local / public-URL sources).
fn apply_source_creds(
    conn: &duckdb::Connection,
    connections: &HashMap<String, creds::ConnectionCreds>,
) -> miette::Result<()> {
    for (i, c) in connections.values().enumerate() {
        if let Some(spec) = &c.secret {
            let resolved =
                fossil_resolver::ResolvedPath::with_secret(&c.url, spec.to_cloud_secret());
            fossil_runtime::install_secret(conn, &resolved, &format!("__fossil_src_{i}"))
                .map_err(|e| miette::miette!("install source secret: {e}"))?;
        }
    }
    Ok(())
}

/// The local filesystem directory a dest URL writes under, or `None` for a cloud
/// object store (which needs no directory pre-creation).
fn local_dest_dir(url: &str) -> Option<PathBuf> {
    if let Some(rest) = url.strip_prefix("file://") {
        return Some(PathBuf::from(rest));
    }
    if url.contains("://") {
        return None; // cloud scheme — flat namespace, no mkdir
    }
    Some(PathBuf::from(url))
}

/// Compile + execute a `.fossil` file, materialising `GraphAr` `Parquet` to
/// `dest_url`. Returns the [`RunStatus`] describing the output graph's structure.
/// The output descriptor is program-resident (invariant #1). `creds` carries the
/// cloud config (empty ⇒ local / public-URL behaviour).
///
/// `memory_bytes` is the run's declared memory budget, and it is one number for
/// the whole run: the `DataFusion` pool the write path executes under and the
/// `DuckDB` `memory_limit` of the layout pass that follows it. Two engines spend
/// memory here; a budget that governed only one of them would be a budget for
/// half the run. `None` runs both unbounded.
///
/// # Errors
/// Returns a compile, read, or materialisation error.
pub fn run(
    path: &Path,
    dest_url: &str,
    creds: &RunCreds,
    memory_bytes: Option<u64>,
) -> miette::Result<RunStatus> {
    tracing::debug!(?path, dest_url, "fossil run");
    let text = std::fs::read_to_string(path)
        .map_err(|e| miette::miette!("read {}: {e}", path.display()))?;

    let (db, file) = open_db(text.clone(), path);
    // The run's one anchor: the directory of the program being run, and the
    // `@conn` map it expands aliases through. Everything below resolves through
    // this and nothing below consults the process's working directory — which
    // is what makes `fossil run apps/docs/programs/hello/hello.fossil` mean the
    // same thing from the repository root as from beside the program.
    let connections = connection_urls(&creds.connections);
    let program_dir = fossil_base::program_dir(&path.to_string_lossy());
    let anchor = SourceAnchor::new(&program_dir, &connections);
    // CSV type pre-introspection (DuckDB DESCRIBE) feeds the type-checker's
    // `source_row`, which the property-graph lowering reads for prop datatypes.
    pre_introspect_and_register(db.system(), &text, anchor, &creds.connections);

    let def_map = fossil_hir::def_map::def_map(&db, file);
    if def_map.mappings(&db).is_empty() {
        return Err(miette::miette!("no mapping found in {}", path.display()));
    }

    // The program-resident output descriptor (ShEx) drives the PG edge/cardinality
    // classification inside `execute_graph` (via `apply_output_shape`).
    let descriptor = resolve_output_descriptor(&db, def_map, anchor)?;

    // The single execution path: lower to the property-graph MIR + execute on
    // DataFusion + write the GraphAr tree. The host's only job is the byte seam
    // for provider (RDF) sources — resolve the URI (`@conn` + relative) and read
    // it; object-store formats stream through the executor's filesystem store.
    let dest_dir = local_dest_dir(dest_url).ok_or_else(|| {
        miette::miette!("the DataFusion run path writes a local directory; cloud dest `{dest_url}` is not yet wired")
    })?;
    // The host's byte seam for provider (RDF) sources. `provider_bindings` has
    // already put every URI through the anchor, so this call is idempotent on
    // what it is handed — it is here because a host that read a raw URI would
    // be the fourth resolution rule.
    let read_uri = |uri: &str| -> Result<String, String> {
        let locator = anchor.locator(uri);
        std::fs::read_to_string(&locator).map_err(|e| format!("read source `{locator}`: {e}"))
    };
    let graph = fossil_df::run_to_dir(
        &db,
        file,
        &descriptor,
        &dest_dir,
        &connections,
        read_uri,
        memory_bytes,
    )
    .map_err(|e| miette::miette!("execute: {e}"))?;

    // W3.1b layout post-pass: replace the placeholder x/y/cluster_id with a real
    // WCC partition + deterministic placement, rewriting each vertex Parquet in
    // place (DuckDB — the one remaining native-runtime use on this path).
    //
    // The status is built BEFORE the pass and repointed BY it, and that order is
    // the fix: `run_status` names `vertex/<Type>.parquet`, which is the truth
    // for `run_to_dir`'s own output and for the wasm host (which runs no layout
    // pass) — and a file this pass deletes. `fossil run --output-json` was
    // handing keasy a path to a file that no longer existed, and nothing here
    // failed, because the deletion is the LAST thing the pass does.
    let mut status = graph.run_status(dest_url);
    enrich_written_layout(&graph, &dest_dir, memory_bytes, &mut status)?;

    Ok(status)
}

/// Run the W3 layout enrichment over the just-written `GraphAr` tree: for each
/// vertex type, point `DuckDB` at its `vertex/<Type>.parquet` plus the CSR Parquet
/// of any self-edge, and rewrite the placeholder `x/y/cluster_id` with a real
/// layout. Local-filesystem paths (the `run_to_dir` dest is a local dir).
///
/// `memory_bytes` is the run's budget again, applied to the second engine — the
/// pass is measured at 0.13 GiB net today, but it is the half of the write path
/// that reads back everything the first half wrote, and an unbounded `DuckDB`
/// targets ~80% of the machine.
fn enrich_written_layout(
    graph: &fossil_df::GraphArData,
    dest_dir: &Path,
    memory_bytes: Option<u64>,
    status: &mut RunStatus,
) -> miette::Result<()> {
    let conn =
        duckdb::Connection::open_in_memory().map_err(|e| miette::miette!("open duckdb: {e}"))?;
    if let Some(bytes) = memory_bytes {
        fossil_runtime::apply_memory_budget(&conn, bytes)
            .map_err(|e| miette::miette!("apply duckdb memory budget: {e}"))?;
    }

    let path_str = |rel: String| dest_dir.join(rel).to_string_lossy().into_owned();
    let adjacency = |e: &fossil_df::EdgeTable, file: &str| {
        path_str(format!(
            "edge/{}_{}_{}/{file}.parquet",
            e.src_type, e.label, e.dst_type
        ))
    };
    let targets: Vec<fossil_runtime::layout::VertexLayoutTarget> = graph
        .schema
        .nodes
        .iter()
        .map(|node| {
            let self_edge_csr = graph
                .edges
                .iter()
                .filter(|e| e.src_type == node.label && e.dst_type == node.label)
                .map(|e| adjacency(e, "by_source"))
                .collect();
            fossil_runtime::layout::VertexLayoutTarget {
                type_name: node.label.clone(),
                vertex_parquet: path_str(format!("vertex/{}.parquet", node.label)),
                // Trailing separator: the layout appends `chunk{k}.parquet`.
                chunk_prefix: path_str(format!("vertex/{}/", node.label)),
                // The same constant the manifest is written with, so the files
                // and the promise cannot drift apart.
                chunk_size: fossil_sinks::manifest::DEFAULT_CHUNK_SIZE,
                self_edge_csr,
            }
        })
        .collect();

    // The tile directories are not created here. `DuckDB`'s COPY writes a file
    // and not the directory above it, so somebody has to — and once the layout
    // emits edge tiles under a prefix it derives for itself, that somebody can
    // only be the layout. Two callers creating the same directory is one of them
    // being wrong about which directories exist.

    // Every adjacency file, both orientations, cross-type included — the layout
    // renumbers `dense_id`, and a file left out keeps ids that now belong to
    // somebody else. Enumerated here rather than derived there because this is
    // the side that has the schema: a missed file is a silent corruption, so
    // naming the set is the caller's job and not a guess.
    let adjacencies: Vec<fossil_runtime::layout::AdjacencyTarget> = graph
        .edges
        .iter()
        .flat_map(|e| {
            use fossil_runtime::layout::Endpoint;
            [
                (adjacency(e, "by_source"), Endpoint::Src),
                (adjacency(e, "by_target"), Endpoint::Dst),
            ]
            .map(
                |(parquet, ordered_by)| fossil_runtime::layout::AdjacencyTarget {
                    parquet,
                    src_type: e.src_type.clone(),
                    dst_type: e.dst_type.clone(),
                    ordered_by,
                },
            )
        })
        .collect();

    fossil_runtime::layout::enrich_layout(&conn, &targets, &adjacencies)
        .map_err(|e| miette::miette!("layout: {e}"))?;

    // The single-file vertex Parquet was this pass's input and nothing reads it
    // afterwards: the manifest points at the chunk prefix, and leaving it would
    // be a second copy of every vertex, stale the moment anything is re-run.
    //
    // Which is why the status is repointed in the same loop. The wire contract
    // says where a host fetches a vertex type's rows, and after this pass that
    // is the chunk prefix the manifest already declares
    // (`VertexInfo::prefix`) — `vertex/<Type>/`, holding `chunk{k}.parquet`.
    // The two are written from the same `format!`, one line apart, because the
    // deletion and the promise are one fact and were two.
    for target in &targets {
        std::fs::remove_file(&target.vertex_parquet)
            .map_err(|e| miette::miette!("remove staged {}: {e}", target.vertex_parquet))?;
        for vertex in &mut status.vertices {
            if vertex.vertex_type == target.type_name {
                vertex.file = format!("vertex/{}/", target.type_name);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conns(pairs: &[(&str, &str)]) -> HashMap<String, creds::ConnectionCreds> {
        pairs
            .iter()
            .map(|(name, url)| {
                (
                    (*name).to_string(),
                    creds::ConnectionCreds {
                        url: (*url).to_string(),
                        secret: None,
                    },
                )
            })
            .collect()
    }

    /// The four `@conn` cases this file used to assert against its own resolver
    /// now live beside the rule itself, in `fossil_base::locator` — there is one
    /// implementation, so there is one place to test it. What is left here is
    /// the engine's own half: the projection the anchor is built from.
    #[test]
    fn the_creds_map_projects_onto_the_anchor_the_rule_takes() {
        let c = conns(&[("sales", "s3://bucket/prefix")]);
        let urls = connection_urls(&c);
        let dir = std::path::PathBuf::from("/programs/shop");
        assert_eq!(
            SourceAnchor::new(&dir, &urls).locator("@sales/2024/orders.csv"),
            "s3://bucket/prefix/2024/orders.csv"
        );
        assert_eq!(
            SourceAnchor::new(&dir, &urls).locator("data/items.csv"),
            "/programs/shop/data/items.csv"
        );
    }

    /// The cache's done-when, counted rather than timed: changing the CSV and
    /// re-running re-introspects; not changing it does not.
    ///
    /// `registrations()` moves only when a `DESCRIBE` actually ran, so the
    /// assertion is on the number of reads of the source and not on how long
    /// the second call took. The third write adds a column, which moves the
    /// size as well as the mtime — the token is both, so the test does not
    /// depend on the filesystem's clock resolution.
    #[test]
    fn a_source_is_re_introspected_when_it_changes_and_not_when_it_does_not() {
        let dir = tempfile::tempdir().expect("tempdir");
        let csv = dir.path().join("users.csv");
        std::fs::write(&csv, "id,name\n1,ada\n").expect("write csv");
        let program = "users := io.csv(\"users.csv\")\n";
        let no_creds = HashMap::new();

        let system = system::EngineSystem::for_program_dir(dir.path());
        let cache = system.descriptors().expect("the engine keeps a table");

        pre_introspect_and_register(
            &system,
            program,
            SourceAnchor::beside(dir.path()),
            &no_creds,
        );
        assert_eq!(
            cache.registrations(),
            1,
            "the first compile reads the source"
        );
        assert_eq!(cache.get("users.csv").expect("registered").columns.len(), 2);

        pre_introspect_and_register(
            &system,
            program,
            SourceAnchor::beside(dir.path()),
            &no_creds,
        );
        assert_eq!(
            cache.registrations(),
            1,
            "an untouched source must not be read a second time"
        );

        std::fs::write(&csv, "id,name,email\n1,ada,ada@example.org\n").expect("rewrite csv");
        pre_introspect_and_register(
            &system,
            program,
            SourceAnchor::beside(dir.path()),
            &no_creds,
        );
        assert_eq!(
            cache.registrations(),
            2,
            "a changed source must be read again"
        );
        assert_eq!(
            cache.get("users.csv").expect("registered").columns.len(),
            3,
            "and the new column is visible to the checker"
        );
    }

    /// The key is the URI, so two bindings over one file cost one read — the
    /// case a binding-name key charged twice for.
    #[test]
    fn two_bindings_over_one_file_introspect_once() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("u.csv"), "id\n1\n").expect("write csv");
        let program = "a := io.csv(\"u.csv\")\nb := io.csv(\"u.csv\")\n";

        let system = system::EngineSystem::for_program_dir(dir.path());
        let cache = system.descriptors().expect("the engine keeps a table");
        pre_introspect_and_register(
            &system,
            program,
            SourceAnchor::beside(dir.path()),
            &HashMap::new(),
        );

        assert_eq!(cache.registrations(), 1);
        assert_eq!(cache.len(), 1);
    }

    /// A source this host cannot `stat` gets an empty token, and an empty token
    /// is never fresh — so a remote object is re-introspected rather than
    /// trusted. The assertion is on the token, since the DESCRIBE of an
    /// unreachable URL fails and registers nothing.
    #[test]
    fn a_locator_that_cannot_be_stat_ed_yields_no_token() {
        assert_eq!(freshness_token("https://example.org/users.csv"), "");
        assert_eq!(freshness_token("/nonexistent/users.csv"), "");
    }

    #[test]
    fn the_token_moves_when_the_file_does() {
        let dir = tempfile::tempdir().expect("tempdir");
        let csv = dir.path().join("u.csv");
        std::fs::write(&csv, "id\n1\n").expect("write");
        let path = csv.to_string_lossy().into_owned();
        let first = freshness_token(&path);
        assert!(!first.is_empty(), "a local file has a token");
        assert_eq!(first, freshness_token(&path), "and it is stable");

        std::fs::write(&csv, "id,name\n1,ada\n").expect("rewrite");
        assert_ne!(first, freshness_token(&path));
    }
}
