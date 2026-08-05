# Fossil — Compiler Architecture

**Version:** 0.1 (design phase)
**Language name:** Fossil
**File extension:** `.fossil`

> **Qué de aquí es cierto hoy.** «Crate layering» y «Dependencies» describen el árbol de hoy y se
> mantienen contra él. Todo lo demás es el corpus de diseño original y hay puntos superados: **en
> caso de duda, ganan los ADRs**, y los que más contradicen este documento son ADR-0002 (quince
> crates, con `fossil-hir` absorbiendo tres), ADR-0044 (tres anillos) y ADR-0045 (dos bloques, y el
> veredicto crate a crate).

---

## La arquitectura en una frase

Compilador **incremental query-based** (Salsa 0.26) con **layered crate structure** estilo rust-analyzer, **lossless CST** para LSP, **bidirectional type checker** sobre input + output descriptors, lowering a **typed operator algebra** (conservative extension de Min Oo & Hartig 2025), codegen a **DuckDB SQL + Parquet/GraphAr manifest**. Runtime fuera del grafo Salsa. WASM-first como design constraint.

---

## Design principles (D1-D11)

| # | Principle |
|---|---|
| D1 | Static correctness — errores predecibles caught at compile time |
| D2 | Provider-driven typing — types from authoritative descriptors, no user annotation |
| D3 | RDF 1.2 + quads ciudadanos de primera clase |
| D4 | Functions y pipelines son valores composables |
| D5 | Portabilidad por transpilación a RML 2.0 |
| D6 | Streaming, larger-than-RAM, columnar (DuckDB + Parquet) |
| D7 | Accesible a data engineers, no solo PL specialists |
| D8 | Fundamentos formales (algemaploom algebra, ShEx-as-types, Milner soundness) |
| D9 | Invisible types, total guarantees — "If it compiles, it runs" |
| D10 | Cross-cutting concerns via attributes, not core extensions |
| D11 | Registry over syntax — funciones del registry, no construcciones específicas |

---

## Crate layering

**Este es el árbol de hoy, derivado de los `Cargo.toml`, no el reparto de diseño.** Son 25 crates.
El diagrama anterior dibujaba `fossil-typeck`, `fossil-types`, `fossil-resolve` y `fossil-codegen`,
que **no existen**: los tres primeros los absorbió `fossil-hir` (ADR-0002, y su `Cargo.toml` lo dice
en un comentario), y el codegen vive repartido entre `fossil-mir` y `fossil-df`. El resto del
documento sigue siendo de fase de diseño; esta sección no.

```
┌─────────────────────────────────────────────────────────────────┐
│  BINARIOS Y HOSTS                                                │
│  fossil-cli    fossil-lsp    fossil-wasm    fossil-mcp   xtask  │
│  fossil-graph-wasm    fossil-df-wasm                            │
└─────────────────────────────────────────────────────────────────┘
                          ▲
┌─────────────────────────────────────────────────────────────────┐
│  ORQUESTACIÓN Y EJECUCIÓN (fuera de Salsa)                       │
│  fossil-engine    (compile + run + catalog + check + refs)      │
│  fossil-runtime   (DuckDB nativo)                    [NATIVO]   │
│  fossil-df        (backend DataFusion del MIR)                  │
│  fossil-resolver  (rutas cloud + credenciales)       [NATIVO]   │
└─────────────────────────────────────────────────────────────────┘
                          ▲
┌─────────────────────────────────────────────────────────────────┐
│  ANÁLISIS (lo que consumen LSP y navegador)                      │
│  fossil-ide       (hover, completion, goto-def, code actions)   │
│  fossil-ide-db    (índices de símbolos)                         │
└─────────────────────────────────────────────────────────────────┘
                          ▲
┌─────────────────────────────────────────────────────────────────┐
│  COMPILADOR                                                      │
│  fossil-mir       (álgebra de operadores tipada)                │
│  fossil-registry  (catálogo stdlib)                             │
│  fossil-hir       (item tree + resolución + tipos + checker)    │
│  fossil-syntax    (CST lossless, parser)                        │
│  fossil-base      (trait Db de Salsa + System)                  │
└─────────────────────────────────────────────────────────────────┘
                          ▲
┌─────────────────────────────────────────────────────────────────┐
│  DESCRIPTORES Y CONTRATOS                                        │
│  fossil-descriptors-input     fossil-descriptors-output         │
│  fossil-shex                  fossil-graph-schema               │
│  fossil-sinks (modelo del manifiesto GraphAr)                   │
│  fossil-run-status (contratos de cable del CLI)                 │
└─────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────┐
│  LECTURA DEL CORPUS — no conoce el lenguaje                      │
│  fossil-graph  (seis verbos: schema, read, expand, path,        │
│                 aggregate, execute_sql)                         │
└─────────────────────────────────────────────────────────────────┘
```

**Regla cardinal:** cada layer depende solo de las que tiene debajo. Nunca skip-level access.

**Y hoy hay una arista que la incumple**, con su ADR abierto: `fossil-base` depende de
`fossil-descriptors-input` (`fossil-base/src/system.rs:21`, por `InferredDescriptor`), así que el
sustrato conoce un tipo concreto de un proveedor concreto. ADR-0045 §2 la documenta y ADR-0045 §11
de la secuencia la borra.

**El reparto que este diagrama no muestra, y que gobierna hoy:** ADR-0044 corta por *dónde corre el
código* (tres anillos: lenguaje / nativo / wasm) y ADR-0045 por *de qué trata* (dos bloques:
«fossil» y «graph»). Los dos ejes componen y ninguno subsume al otro; la tabla de ADR-0045 §7 los
cruza.

---

## Three extension points

The language has three orthogonal extension dimensions, each implemented as a trait.

### 1. Input descriptors (formerly "type providers")

Trait at `fossil-descriptors/input/`:

```rust
pub trait InputDescriptor: Send + Sync {
    type Schema;
    
    fn name(&self) -> &str;
    
    fn parse_descriptor(&self, raw: &[u8]) 
        -> Result<Self::Schema, DescriptorError>;
    
    fn infer_from_data(&self, source_path: &str) 
        -> Result<Self::Schema, DescriptorError>;
    
    fn type_for_field(&self, schema: &Self::Schema, field: &str) 
        -> Option<Ty>;
}
```

Implementations: CSVW, JSON Schema, XSD, SQL catalog, Parquet schema.

### 2. Output descriptors (formerly "shape resolver")

Trait at `fossil-descriptors/output/`:

```rust
pub trait OutputDescriptor: Send + Sync {
    type Schema;
    
    fn name(&self) -> &str;
    
    fn parse_descriptor(&self, raw: &[u8]) 
        -> Result<Self::Schema, DescriptorError>;
    
    fn type_for_property(&self, schema: &Self::Schema, 
                          shape: ShapeRef, predicate: &Iri) 
        -> Option<PropertyType>;
    
    fn cardinality(&self, schema: &Self::Schema, 
                    shape: ShapeRef, predicate: &Iri) 
        -> Cardinality;
    
    fn is_closed(&self, schema: &Self::Schema, shape: ShapeRef) -> bool;
}
```

Implementations: ShEx (primary, via WESO's `rudof`), SHACL Core (fase 2).

**Symmetric mental model:** input descriptors describe source data structure; output descriptors describe target graph structure. Type checker uses both bidirectionally.

### 3. Sinks

Trait at `fossil-sinks/`:

```rust
pub trait Sink: Send + Sync {
    fn name(&self) -> &str;
    
    fn vertex_edge_decomp(&self, plan: &MirGraph) -> SinkPlan;
    
    fn manifest(&self, plan: &SinkPlan) -> Vec<u8>;
    
    fn sql_for(&self, plan: &SinkPlan) -> Vec<SqlStatement>;
}
```

Implementations: GraphAr/Parquet (primary), Turtle / JSON-LD / N-Quads (fase 2).

### Plus: Function registry (parallel structure)

Not a trait but a registry pattern. See `fossil-registry/`:

```rust
pub struct FossilRegistry {
    builtins: HashMap<Symbol, BuiltinFn>,
    compositions: HashMap<Symbol, NamedComposition>,
}

pub struct BuiltinFn {
    pub sig: FnSig,
    pub impl_kind: BuiltinImpl,
}

pub enum BuiltinImpl {
    DuckDbBuiltin(String),       // SQL builtin (e.g., trim, md5)
    RustUdf(UdfFunction),        // Rust-implemented UDF
}
```

(External functions via `extern` declarations: fase 2.)

---

## Salsa database

`fossil-base` defines the database trait. Hosts implement specific capabilities.

```rust
pub trait HasInputDescriptors {
    fn input_descriptor(&self, kind: &str) -> Option<&dyn InputDescriptor>;
}

pub trait HasOutputDescriptors {
    fn output_descriptor(&self, kind: &str) -> Option<&dyn OutputDescriptor>;
}

pub trait HasRegistry {
    fn registry(&self) -> &FossilRegistry;
}

#[salsa::db]
pub trait Db: HasInputDescriptors 
              + HasOutputDescriptors
              + HasRegistry 
              + salsa::Database {}

#[salsa::db]
#[derive(Clone)]
pub struct FossilDb {
    storage: salsa::Storage<Self>,
    input_descriptors: HashMap<String, Box<dyn InputDescriptor>>,
    output_descriptors: HashMap<String, Box<dyn OutputDescriptor>>,
    registry: FossilRegistry,
}
```

Pattern: DataFusion `SessionState` + rust-analyzer `RootDatabase`. Builder pattern for construction.

---

## Salsa inputs and tracked queries

### Inputs

```rust
#[salsa::input(debug)]
pub struct SourceFile {
    #[returns(ref)] pub text: String,
    #[returns(ref)] pub uri: String,
}

#[salsa::input(debug)]
pub struct DescriptorFile {
    #[returns(ref)] pub contents: Vec<u8>,
    #[returns(ref)] pub uri: String,
    pub kind: DescriptorKind,        // csvw | jsonschema | xsd | shex | ...
}

#[salsa::input(debug)]
pub struct ProjectConfig {
    pub stdlib_version: String,
    pub sinks: Vec<SinkConfig>,
}

#[salsa::input(debug)]
pub struct Lockfile {
    pub entries: Vec<LockedFunction>,
}
```

### Tracked queries (memoized derived values)

**Parser:**
```rust
#[salsa::tracked]
pub fn parse(db: &dyn Db, file: SourceFile) -> Cst;

#[salsa::tracked]
pub fn item_tree(db: &dyn Db, file: SourceFile) -> ItemTree;
// stable under body edits — critical for incremental invalidation
```

**Name resolution:**
```rust
#[salsa::tracked]
pub fn def_map(db: &dyn Db, file: SourceFile) -> DefMap;

#[salsa::tracked]
pub fn resolve_prefix(db: &dyn Db, file: SourceFile, name: Symbol) -> Option<Iri>;
```

**Descriptor layer:**
```rust
#[salsa::tracked]
pub fn input_schema(db: &dyn Db, descriptor: DescriptorFile) 
    -> Result<InputSchema, DescriptorError>;

#[salsa::tracked]
pub fn output_schema(db: &dyn Db, descriptor: DescriptorFile)
    -> Result<OutputSchema, DescriptorError>;
```

**Type checking:**
```rust
#[salsa::tracked]
pub fn type_of_reference(db: &dyn Db, mapping: MappingLoc, field: Symbol) -> Ty;

#[salsa::tracked]
pub fn function_signature(db: &dyn Db, fn_ref: FunctionRef) -> FnSig;

#[salsa::tracked]
pub fn typecheck_mapping(db: &dyn Db, mapping: MappingLoc) -> Result<()>;
// Pushes diagnostics via accumulator
```

**Lowering and codegen:**
```rust
#[salsa::tracked]
pub fn lower_to_mir(db: &dyn Db, mapping: MappingLoc) -> MirGraph;

#[salsa::tracked]
pub fn optimize_mir(db: &dyn Db, mir: MirGraph) -> MirGraph;

#[salsa::tracked]
pub fn codegen_sql(db: &dyn Db, mapping: MappingLoc) -> SqlPlan;
```

### Accumulators

```rust
#[salsa::accumulator]
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub message: String,
    pub span: Span,
    pub severity: Severity,
    pub kind: DiagKind,
}
```

Any query can `diagnostic.accumulate(db)`. Host collects with `query::accumulated::<Diagnostic>(db, ...)`.

---

## Per-item granularity

Critical pattern for LSP performance. Queries indexed by entities the user perceives as units:

```rust
#[salsa::interned]
pub struct MappingLoc<'db> {
    pub file: SourceFile,
    pub index: usize,
}

#[salsa::interned]
pub struct FunctionLoc<'db> {
    pub file: SourceFile,
    pub index: usize,
}

#[salsa::interned]
pub struct SourceLoc<'db> {  // a `users := csv(...)` definition
    pub file: SourceFile,
    pub index: usize,
}
```

Editing one mapping's body invalidates only `typecheck_mapping(M)` and downstream. Sibling mappings, functions, source defs remain cached.

---

## Lossless CST

Using `rowan` crate (same as rust-analyzer). Preserves:
- Whitespace + comments
- Exact source positions
- Error nodes from parser recovery

Enables LSP features requiring precise spans: hover ranges, refactoring, formatter, code actions.

```rust
pub struct Cst {
    root: SyntaxNode,
}

// AST is a typed view over CST
pub struct Mapping(SyntaxNode);

impl Mapping {
    pub fn name(&self) -> Option<Identifier> { ... }
    pub fn shape(&self) -> Option<ShapeExpr> { ... }
    pub fn graph(&self) -> Option<IriExpr> { ... }
    pub fn source(&self) -> Option<Expression> { ... }
    pub fn body(&self) -> Option<MappingBody> { ... }
}
```

---

## ErrorGuaranteed taint pattern

Borrowed from rustc. When type-check fails, the resulting `Ty` is `Ty::Error(ErrorGuaranteed)`. Downstream uses unify with `Error` without emitting new diagnostics.

```rust
pub enum TyKind {
    Primitive(PrimitiveType),
    Optional(Ty),
    Seq(Ty),
    Record(RecordFields),
    Iri,
    Shape(ShapeId),
    Fn(Vec<Ty>, Ty),
    Error(ErrorGuaranteed),    // taint marker
}

pub struct ErrorGuaranteed {
    _proof: PhantomData<*const ()>,  // only constructible after emit_error
}
```

**Result:** N independent errors produce N diagnostics, not N². No cascading. UX feels rustc-quality.

---

## Cancellation

Salsa 0.26 has `CancellationToken`. LSP server cancels in-flight queries when user edits.

```rust
fn on_edit(&mut self, edit: Edit) {
    self.db.cancel_pending();
    self.db.update_source(edit.file, edit.new_text);
}
```

Designed from day 1. Costly to retrofit later.

---

## Implicit closure synthesis

When a function expects `T -> U` and the argument is an expression containing free `FieldRef`s, the compiler synthesizes a single-parameter lambda. Detailed in `type-system.md`.

*Aterrizó en `fossil-hir/src/check.rs` (`synthesize_closure`), no en un crate propio; el boceto de
abajo conserva la forma, no las firmas.*

```rust
fn maybe_lift_closure(
    arg_expr: &Expression,
    expected_ty: &Ty,
    row_context: &RowContext,
) -> Result<Closure, TypeError> {
    if let Ty::Fn(params, ret) = expected_ty {
        if params.len() == 1 && contains_field_refs(arg_expr) {
            return Ok(synthesize_closure(arg_expr, params[0], row_context));
        }
    }
    type_check_as_concrete(arg_expr, expected_ty)
}
```

---

## Runtime boundary

**Salsa termina cuando se emite el plan.** Runtime executes outside the query graph.

```rust
// fossil-runtime/src/lib.rs

pub fn execute(plan: &SqlPlan, sinks: &[SinkPlan]) 
    -> Result<(), RuntimeError> 
{
    let conn = duckdb::Connection::open_in_memory()?;
    register_udfs(&conn)?;
    for stmt in &plan.statements {
        conn.execute(&stmt.sql, params![])?;
    }
    for sink in sinks {
        write_manifest(&sink.manifest_path, &sink.manifest)?;
    }
    Ok(())
}
```

**Salsa db can be dropped after compile.** Runtime takes over with pure side-effecting code.

---

## LSP integration

```rust
pub struct FossilLspServer {
    db: FossilDb,
    vfs: Vfs,
}

impl LanguageServer for FossilLspServer {
    fn hover(&self, params: HoverParams) -> Option<Hover> {
        let file = self.vfs.find(&params.uri)?;
        let cst = parse(&self.db, file);
        let node = cst.node_at(params.position)?;
        let ty = type_at(&self.db, node)?;
        Some(Hover { contents: format_type(ty) })
    }
    
    fn diagnostics(&self, file: SourceFile) -> Vec<Diagnostic> {
        typecheck_file::accumulated::<Diagnostic>(&self.db, file)
    }
}
```

Same db powers CLI and LSP. Difference: LSP uses cancellation aggressively.

---

## WASM packaging

```rust
#[wasm_bindgen]
pub struct FossilPlayground {
    db: FossilDb,
}

#[wasm_bindgen]
impl FossilPlayground {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self { ... }
    
    pub fn check(&mut self, source: &str) -> JsValue {
        let file = SourceFile::new(&self.db, source.into(), "main.fossil".into());
        let diags = typecheck_file::accumulated::<Diagnostic>(&self.db, file);
        serde_wasm_bindgen::to_value(&diags).unwrap()
    }
    
    pub fn compile(&mut self, source: &str) -> JsValue {
        // Returns SQL + manifest as JSON
    }
}
```

DuckDB-WASM executes the SQL output. Mosaic visualizes results in playground.

---

## Compile pipeline visualized

```
.fossil file
     ↓
[parse]         → Cst (lossless, rowan)
     ↓
[item_tree]     → ItemTree (top-level summary, stable)
     ↓
[def_map]       → DefMap (name resolution, prefix expansion)
     ↓
[type_of_*]     → Ty (forward propagation from input descriptors)
     ↓
[target_shape]  → ShExShape (via rudof; output descriptor)
     ↓
[typecheck]     → Result<(), Diagnostics> (bidirectional verify)
     ↓
[lower_to_mir]  → MirGraph (typed operator algebra)
     ↓
[optimize]      → MirGraph (algebraic rewriting passes)
     ↓
[codegen]       → SqlPlan + GraphArManifest
     ↓
═══ Salsa boundary ═══
     ↓
[runtime]       → DuckDB executes → Parquet output
```

---

## Dependencies

**Las versiones vivas están en `[workspace.dependencies]` y en la tabla de `CLAUDE.md`; esto es la
intención de diseño, con las tres elecciones que se revirtieron marcadas.**

```toml
# fossil-syntax
rowan = "0.16"           # lossless CST
logos = "0.16"           # lexer

# fossil-base
salsa = "0.26"           # incremental queries

# fossil-descriptors-input
# sqlx — DESCARTADO: no compila a WASM; prohibido en deny.toml
# serde_yml — DESCARTADO: RUSTSEC-2025-0068; se usa serde_yaml_ng
arrow = "..."            # Parquet schema
serde_yaml_ng = "0.10"   # CSVW JSON-LD

# fossil-descriptors-output / fossil-shex
rudof = "..."            # ShEx parser + validator (WESO group, in-house)

# fossil-mir
sqlparser = "0.59"       # SQL AST construction

# fossil-df
datafusion = "..."       # el backend que sí compila a wasm32

# fossil-runtime / fossil-engine / fossil-mcp
duckdb = "1.10502"       # ejecución nativa (bundled)

# fossil-lsp
# tower-lsp — DESCARTADO: sin mantenimiento; se usa lsp-server (ADR-0001)
lsp-server = "0.7"
miette = "7.6"

# fossil-wasm
wasm-bindgen = "=0.2.120"
```

---

## Reference codebases

1. **rust-analyzer** (`crates/ide/`, `crates/hir-def/`, `crates/hir-ty/`) — canonical reference for Salsa + LSP + multi-crate compiler. https://github.com/rust-lang/rust-analyzer

2. **ty (Astral, formerly red-knot)** — Python type checker on Salsa. More recent, more readable codebase. https://github.com/astral-sh/ty

3. **rudof** (WESO group) — Rust ShEx implementation. Used as our `OutputDescriptor` for ShEx. Coauthor Labra Gayo from same group. https://github.com/rudof-project/rudof

4. **algemaploom-rs** (Min Oo & Hartig) — untyped operator algebra reference. We extend with types. https://github.com/RMLio/algemaploom-rs

---

## Why this architecture is solid

- **Salsa guarantees incrementality.** LSP sub-100ms response for typical mappings. Editing one property invalidates only affected queries.
- **Layered crates with depend-only-downward** prevents circular dependencies, maintains cohesion.
- **Lossless CST enables every IDE feature** without re-parsing.
- **ErrorGuaranteed taint** prevents cascading errors — UX feels rustc-quality.
- **Three orthogonal extension traits** preserve core stability while permitting growth.
- **Runtime outside the compile DAG** enables parallel execution, alternative backends, mockable tests.
- **WASM-first design constraint** from day 1 — no painful retrofit.
- **Symmetric input/output descriptor abstraction** unifies forward/backward type checking under one mental model.
