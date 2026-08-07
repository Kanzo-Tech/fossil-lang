# Fossil — Typed Operator Algebra

**Version:** 0.1 (design phase)

This document specifies the typed intermediate representation (MIR) of Fossil. The MIR is a **typed extension** of the operator algebra introduced by Min Oo & Hartig (ESWC 2025 Best Research Award) [1]. Their algebra defines the operational semantics of knowledge graph construction in an implementation-agnostic, language-agnostic way. We extend it conservatively with type annotations on operator schemas.

---

## 1. Foundational claim

> Fossil's MIR is a typed elaboration of the operator algebra of Min Oo & Hartig [2025]. The untyped fragment of our algebra is operationally equivalent to theirs; types add static guarantees without altering operational meaning.

This positioning supports paper §5 (Compilation) and provides formal grounding via the Min Oo & Hartig paper's existing soundness arguments.

---

## 2. Operator set

We adopt nine operators from [1] plus two refinements specific to typed sink emission.

### 2.1 Source

```
SourceOp(uri: Path, format: F, descriptor: Option<Descriptor>) 
    : () → Source<Record<R>>
```

`R` is the row type derived from the descriptor (CSVW, JSON Schema, etc.) or inferred from sampling. Origin of all row data in the plan.

### 2.2 Project

```
ProjectOp(input: Source<Record<R>>, cols: Set<FieldName>) 
    : Source<Record<R | cols>>
```

`R | cols` denotes the subset of R restricted to selected columns.

### 2.3 Extend

```
ExtendOp(input: Source<Record<R>>, field: FieldName, expr: Expression<τ>) 
    : Source<Record<R ∪ {field: τ}>>
```

Adds a computed field. The expression's type `τ` becomes the new field's type. This is where most function application happens.

### 2.4 Rename

```
RenameOp(input: Source<Record<R ∪ {old: τ}>>, old: FieldName, new: FieldName)
    : Source<Record<R ∪ {new: τ}>>
```

### 2.5 Filter

```
FilterOp(input: Source<Record<R>>, pred: Fn((Record<R>) → Bool))
    : Source<Record<R>>
```

Closure synthesis: when called as `filter(.field op v)`, the implicit closure is materialized per `type-system.md` §7.

### 2.6 Join

```
JoinOp(left: Source<Record<L>>, right: Source<Record<R>>, 
       condition: Fn((Record<L>, Record<R>) → Bool),
       join_kind: Inner | LeftOuter | RightOuter | Full)
    : Source<Record<{left_name: L, right_name: R}>>
```

Auto-prefix per D8: result row exposes `left_name.{...L}` and `right_name.{...R}`.

### 2.7 Union

```
UnionOp(left: Source<Record<R>>, right: Source<Record<R>>) 
    : Source<Record<R>>
```

Requires left/right schemas to unify.

### 2.8 GroupBy + Aggregate

```
GroupByOp(input: Source<Record<R>>, key: FieldName | List<FieldName>)
    : GroupedSource<Record<R>, key>

AggregateOp(grouped: GroupedSource<Record<R>, key>, 
            aggs: List<(field, Fn((Seq<τᵢ>) → τ_agg))>) 
    : Source<Record<{key fields} ∪ {agg fields}>>
```

Combined pattern: `s |> group_by(.k) |> aggregate(total = sum(.amount))`. Aggregation functions (sum, count, avg, min, max) live in stdlib `math/` namespace.

### 2.9 Distinct

```
DistinctOp(input: Source<Record<R>>, by: Option<FieldName | List<FieldName>>)
    : Source<Record<R>>
```

### 2.10 Triple emission (Fossil-specific extension)

Replaces Min Oo & Hartig's combined `SerializerOp` + `TargetOp` with explicit typed emission:

```
TripleEmitOp(input: Source<Record<R>>,
             subject_expr: Expression<Iri>,
             predicate: Iri,
             object_expr: Expression<τ_obj>,
             graph: Option<Iri>)
    : TripleStream
```

`τ_obj` must be one of `Iri`, `Literal<datatype>`, `LangLiteral`, `TripleTerm`, or a `Shape<S>` (blank node).

When `graph` is present, emits quads; otherwise default graph triples.

### 2.11 Sink lowering

```
SinkOp(stream: TripleStream, sink: SinkRef)
    : ()
```

Terminal operator. The sink implementation (GraphAr, Turtle, etc.) consumes the typed triple stream and produces output artifacts. Sink decomposition (vertex/edge tables, manifest generation) happens here per the `Sink` trait.

---

## 3. Type signatures and schema propagation

Each operator has a type signature describing input → output schema transformation. The schema is the row type within `Source<Record<R>>`.

**Schema propagation theorem (preservation):**

> *If every operator's input schema matches its signature's expected input, the operator's output schema is well-typed. By structural induction over the operator DAG, every node in the plan has a well-typed schema.*

**Practical consequence:** the type checker walks the operator graph in topological order, propagating schemas. Any mismatch (e.g., a Filter referencing a field not in its input schema) is a compile-time error.

---

## 4. Algebraic equivalences (rewriting rules)

Min Oo & Hartig prove several algebraic equivalences in [1]. We inherit them and add type-aware variants.

### 4.1 From [1] (untyped, preserved typed)

```
(R1)  filter(filter(s, p), q)  ≡  filter(s, λr. p(r) ∧ q(r))
(R2)  project(project(s, c₁), c₂)  ≡  project(s, c₁ ∩ c₂)
(R3)  project(filter(s, p), c)  ≡  filter(project(s, c), p)  if free(p) ⊆ c
(R4)  filter(union(s₁, s₂), p)  ≡  union(filter(s₁, p), filter(s₂, p))
(R5)  filter(join(s₁, s₂, c), p)  ≡  join(filter(s₁, p), s₂, c)  if free(p) ⊆ schema(s₁)
(R6)  rename(s, a, b)  ≡  extend(project(s, schema(s) \ {a}), b, ref(a))
                          when type considerations satisfied
```

### 4.2 Fossil-specific (typed)

```
(R7)  extend(s, f, e)  ≡  extend(s, f, partial_eval(e, schema(s)))
                          when e is partially evaluable
(R8)  filter(s, p) where p is statically true   ≡  s
(R9)  filter(s, p) where p is statically false  ≡  empty(schema(s))
(R10) group_by(group_by(s, k₁), k₂)  ≡  group_by(s, k₁ ∪ k₂)
                                        when k₁ ⊆ schema(s) and aggregations compose
```

**We do not implement these.** They are recorded because they are the algebra's properties, not
because they are our code: `rewrite.rs` held R1–R10, ran on every compile and always returned its
input, and it was removed along with the `eval.rs` that existed only to drive R7–R10 (ADR-0046 F5,
`apps/docs/content/docs/characteristics/pipeline.mdx`). The plan is DataFusion's — predicate
pushdown, join reordering and constant folding included. Ours is the type of the mapping.

---

## 5. Lowering from HIR to MIR

The HIR represents a complete Fossil source file (name-resolved, type-checked). Lowering to MIR is structural:

```
HIR Source declaration `s := csv("...")` 
    → MIR  SourceOp("...", CSV, descriptor)

HIR Pipeline `s |> filter(p)` 
    → MIR  FilterOp(SourceOp(...), p_closure)

HIR Pipeline `s |> join(t, on=c)` 
    → MIR  JoinOp(SourceOp(s), SourceOp(t), c_closure, Inner)

HIR Pipeline `s |> group_by(k) |> aggregate(...)` 
    → MIR  AggregateOp(GroupByOp(SourceOp(s), k), agg_specs)

HIR Mapping body `iri = expr; pᵢ = eᵢ`
    → MIR  for each pᵢ in body:
           TripleEmitOp(source, subject=iri_expr_lowered, 
                                predicate=pᵢ, 
                                object_expr=eᵢ_lowered,
                                graph=graph_iri_or_None)

HIR Annotation block on triple (s, p, o):
    → MIR  for each (a = v) in block:
           TripleEmitOp(source, subject=TripleTerm(s, p, o),
                                predicate=a,
                                object_expr=v_lowered,
                                graph=...)
           (recursive for nested annotations)
```

Function applications in property expressions become `ExtendOp`s preceding the `TripleEmitOp`, so function-applied values are available as fields before emission.

---

## 6. Lowering from MIR to DuckDB SQL

The MIR is target-agnostic at the operator level. Lowering to DuckDB SQL is performed per sink type.

### 6.1 Source → SQL

```
SourceOp("users.csv", CSV, _) → 
    CREATE VIEW users AS 
    SELECT * FROM read_csv_auto('users.csv', sample_size=-1);
```

### 6.2 Filter → SQL

```
FilterOp(s, p_closure) →
    SELECT * FROM <s_sql> WHERE <p_closure_compiled>
```

The closure's body is translated to SQL expression with field references becoming column references.

### 6.3 Join → SQL

```
JoinOp(s_left, s_right, c, Inner) →
    SELECT * FROM <s_left_sql> 
    INNER JOIN <s_right_sql> ON <c_compiled>
```

Auto-prefix is implemented via `SELECT s_left.* AS "s_left.field"` style. Actual representation depends on column aliasing strategy.

### 6.4 Project → SQL

```
ProjectOp(s, cols) → 
    SELECT <cols_compiled> FROM <s_sql>
```

### 6.5 Aggregate → SQL

```
GroupByOp + AggregateOp(grouped, aggs) → 
    SELECT <key_fields>, <agg_exprs> 
    FROM <input_sql> 
    GROUP BY <key_fields>
```

### 6.6 Extend (function application) → SQL UDF or builtin

For functions implemented as DuckDB SQL builtins (`trim`, `lower`, etc.):
```
ExtendOp(s, f, clean.trim(x)) → 
    SELECT *, trim(x) AS "f" FROM <s_sql>
```

For Rust UDFs registered with DuckDB:
```
ExtendOp(s, f, hash(x, salt=...)) → 
    SELECT *, hash_udf(x, '<salt>') AS "f" FROM <s_sql>
```

The registry maps function names to either inline SQL templates or UDF call patterns.

### 6.7 TripleEmit → SQL + manifest

`TripleEmitOp` is decomposed per the active sink. For GraphAr:

```
TripleEmitOp(input, subj, pred, obj, graph) → 
    Depending on object type:
      - Iri object: edge table entry
      - Literal object: vertex property
      - TripleTerm object: nested table with parent triple reference
    
    Emitted as COPY TO 'graphar_path/vertices/<type>.parquet' (FORMAT PARQUET)
                  COPY TO 'graphar_path/edges/<type>.parquet' (FORMAT PARQUET)
```

The sink's `vertex_edge_decomp` method drives the actual SQL generation.

---

## 7. Visualization

The MIR is naturally visualizable as a DAG. Each operator is a node; edges represent dataflow. This was inherited from algemaploom's DOT visualization capability, extended with type labels:

```
SourceOp(users.csv)         SourceOp(orders.csv)
   ↓ : Record<{id,...}>        ↓ : Record<{user_id,...}>
   FilterOp(.status=="active")
   ↓ : Record<{id,...,status:String}>
   JoinOp(on user_id=id)
   ↓ : Record<{users.id,...,orders.amount:Decimal}>
   ExtendOp("iri", template(...))
   ↓ : Record<{...,iri:Iri}>
   TripleEmitOp(subject=.iri, predicate=ex:total, object=.orders.amount)
   ↓ : TripleStream
   SinkOp(graphar)
```

Useful for IDE feature: "show plan" command to visualize compilation result. Also for debugging.

---

## 8. References

[1] Min Oo, S., & Hartig, O. (2025). *An Algebraic Foundation for Knowledge Graph Construction.* ESWC 2025. Best Research Award. arXiv:2503.10385.

[2] algemaploom-rs implementation: https://github.com/RMLio/algemaploom-rs

---

## 9. Conservative extension claim (for paper)

We claim Fossil's typed operator algebra is a *conservative extension* of Min Oo & Hartig's algebra:

1. Every operator in their algebra has a corresponding typed operator in ours.
2. Their algebraic equivalences (R1–R6) hold in our typed version (preservation by induction).
3. The untyped projection of our algebra (erasing all type annotations) is operationally equivalent to theirs.
4. Type errors caught by our algebra correspond to runtime errors that their algebra would surface (or worse, silently produce malformed RDF).

**Implication for soundness:** their algebra's operational semantics carry over directly. Our type system adds compile-time guarantees but doesn't change runtime behavior of well-typed programs. The soundness theorem of `type-system.md` §10 inherits their operational framework.
