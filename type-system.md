# Fossil — Type System

**Version:** 0.1 (design phase)

This document specifies the static semantics of Fossil. The grammar (`grammar.bnf`) defines well-formed syntax; the type system defines well-formed *meaning*. The type checker enforces what the grammar cannot.

---

## 1. Foundational principles

The type system enacts D1 (static correctness), D2 (provider-driven), D9 (invisible types, total guarantees), and D8 (formal foundations).

**Soundness commitment:** following Milner [1978] — *well-typed Fossil mappings cannot fail at execution due to type or shape violations*. The user writes no types; the compiler derives them, propagates them, and verifies them.

**Bidirectional checking.** Type information flows two ways:
- **Forward** from input descriptors (CSVW, JSON Schema, XSD, SQL catalogs, Parquet) into the mapping.
- **Backward** from output descriptors (ShEx target shapes) into the mapping's property assignments.

The checker verifies that the two flows meet consistently at each property assignment.

---

## 2. Type ADT

The type lattice. Implemented as interned `Ty` values for O(1) equality (rustc `Ty<'tcx>` pattern).

```
τ  ::=  Primitive(p)                  primitive types
     |  Optional(τ)                   T? — nullable wrapper
     |  Seq(τ)                        T* — repeated/sequence
     |  Record({fᵢ : τᵢ})              tabular row type
     |  Iri                            absolute or templated IRI
     |  IriTemplate                   IRI template before substitution
     |  Shape(S)                       satisfies ShEx shape S
     |  Fn((τ₁, ..., τₙ) → τ_r)        function signature
     |  TripleTerm                     RDF 1.2 triple-as-term
     |  Error(ErrorGuaranteed)         taint marker after diagnostic emit

p  ::=  String | Integer | Float | Bool
     |  Date | DateTime | Time | gYear
     |  AnyURI
```

**No sum types in MVP.** ShEx `OneOf` is not supported; compile error suggests splitting into multiple mappings.

**No type variables / generics in MVP.** Functions are monomorphic. Overloading resolved via registry lookup.

**Refinement types** are present implicitly via shape facets — type `Ty` with associated facet constraints. The checker carries facet predicates as side info during checking but does not expose them in the surface `Ty` ADT.

---

## 3. Typing judgments

We use three judgment forms:

```
Γ ⊢ e : τ              expression e has type τ in environment Γ
Γ; ρ ⊢ e : τ            same, with row context ρ available for FieldRefs
Γ ⊢ M ok                mapping M is well-formed
```

`Γ` is the typing environment: prefix bindings, identifier bindings, function signatures from the registry.

`ρ` is the row context: a record type describing the fields available at the current point in a source pipeline or mapping body. Introduced when entering a source operation or mapping body.

---

## 4. Inference rules

### 4.1 Literals

```
                                                
   T-Int:    ─────────────────────                  T-Float: ──────────────────────
              Γ ⊢ n : Integer                                  Γ ⊢ f : Float

   T-Str:    ───────────────────                   T-Bool:  ──────────────────
              Γ ⊢ s : String                                  Γ ⊢ b : Bool
```

### 4.2 Identifiers and field access

```
                  x : τ ∈ Γ
   T-Var:   ────────────────
                 Γ ⊢ x : τ


                  ρ has field `name` of type τ
   T-Field: ────────────────────────────────────
                  Γ; ρ ⊢ .name : τ


                  Γ; ρ ⊢ .name : Record({sub : τ, ...})
   T-FieldChain: ──────────────────────────────────────
                  Γ; ρ ⊢ .name.sub : τ
```

### 4.3 Environment variables

```
   T-Env:    ────────────────────────
              Γ ⊢ $NAME : String
```

(Env vars are always typed `String`. Runtime resolves; unset env var = runtime error.)

### 4.4 Function calls

```
                  Γ ⊢ f : Fn((τ₁, ..., τₙ) → τ_r)
                  Γ ⊢ eᵢ : τᵢ' such that τᵢ' <: τᵢ for all i
   T-Call:  ──────────────────────────────────────────────
                  Γ ⊢ f(e₁, ..., eₙ) : τ_r
```

Named arguments resolve by name to the declared parameter name in the signature.

### 4.5 Partial application

```
                  Γ ⊢ f : Fn((τ₁, ..., τₙ) → τ_r)
                  for each argᵢ: either eᵢ ≡ '_' (placeholder)
                                  or Γ ⊢ eᵢ : τᵢ' with τᵢ' <: τᵢ
                  k placeholders among args
   T-Partial: ──────────────────────────────────────────────────
                  Γ ⊢ f(args) : Fn((τⱼ for placeholder positions) → τ_r)
```

### 4.6 Pipeline

```
                  Γ ⊢ e : τ      Γ ⊢ f : Fn((τ) → τ')
   T-Pipe:   ────────────────────────────────────────
                  Γ ⊢ e |> f : τ'
```

The pipeline operator passes the LHS as the **first argument** to the RHS function.

### 4.7 Arithmetic, comparison, boolean

```
                  Γ ⊢ a : Numeric    Γ ⊢ b : Numeric
   T-Arith:  ─────────────────────────────────────
                  Γ ⊢ a op b : promoted(a, b)
                  
                  (op ∈ {+, -, *, /, %})


                  Γ ⊢ a : τ    Γ ⊢ b : τ    τ is comparable
   T-Comp:   ──────────────────────────────────────────────
                  Γ ⊢ a op b : Bool
                  
                  (op ∈ {==, !=, <, <=, >, >=})


                  Γ ⊢ a : Bool    Γ ⊢ b : Bool
   T-And:    ─────────────────────────────────
                  Γ ⊢ a and b : Bool


                  Γ ⊢ a : Bool
   T-Not:    ───────────────────
                  Γ ⊢ not a : Bool
```

`Numeric` is the join of `Integer` and `Float`. `promoted(Integer, Float) = Float`.

### 4.8 Ternary

```
                  Γ ⊢ c : Bool     Γ ⊢ a : τ     Γ ⊢ b : τ
   T-Tern:   ───────────────────────────────────────────
                  Γ ⊢ c ? a : b : τ
```

Both branches must have the same type. No implicit coercion.

### 4.9 Templates and interpolation

```
                  for each interp eᵢ in template t:
                      Γ ⊢ eᵢ : τᵢ where τᵢ is stringifiable
                  context dictates: target_type ∈ {String, Iri}
   T-Template:  ─────────────────────────────────────────
                  Γ ⊢ `...${e}...` : target_type
```

Templates produce `String` by default. In contexts requiring `Iri` (e.g., `iri =` assignments, IRI-typed properties), the template is interpreted as `Iri` and interpolations are URL-encoded.

### 4.10 Triple terms

```
                  Γ ⊢ s : Iri      Γ ⊢ p : Iri      Γ ⊢ o : Iri ∪ Literal ∪ TripleTerm
   T-Triple: ────────────────────────────────────────────────────────────────────
                  Γ ⊢ <<s p o>> : TripleTerm
```

### 4.11 Record literals (blank nodes)

```
                  Γ ⊢ vᵢ : τᵢ for each (kᵢ = vᵢ) in record
   T-Record: ────────────────────────────────────────────
                  Γ ⊢ {k₁ = v₁, ..., kₙ = vₙ} : Record({kᵢ : τᵢ})
```

When assigned to a property typed `Shape(S)` where S is a blank-node shape, the record is checked against S's property types.

### 4.12 Source operations (pipeline over sources)

```
                  Γ ⊢ s : Source<Record<R>>
                  Γ ⊢ op : Fn((Source<Record<R>>, ...) → Source<Record<R'>>)
   T-SrcOp:  ────────────────────────────────────────────────────────────
                  Γ ⊢ s |> op(...) : Source<Record<R'>>
```

Specific operators (filter, join, project, aggregate, etc.) refine R → R' per their semantics.

---

## 5. Mapping typing

The headline judgment: `Γ ⊢ M ok` for a complete mapping.

```
                  Γ ⊢ source_expr : Source<Record<R>>
                  shape S ∈ Γ (target shape resolved)
                  Γ; row : R ⊢ iri_expr : Iri
                  for each (pᵢ = eᵢ) in body:
                      shape S declares pᵢ : τᵢ' with cardinality cᵢ
                      Γ; row : R ⊢ eᵢ : τᵢ
                      compatible(τᵢ, τᵢ', cᵢ)
                  if S is closed:
                      every predicate in body ∈ S's declared properties
                  iri_expr position satisfied exactly once
   T-Mapping: ──────────────────────────────────────────────────────────────────
              Γ ⊢ "Name : S in g from source_expr  { iri = iri_expr; pᵢ = eᵢ }" ok
```

**`compatible(τ, τ', c)`** is the property-type compatibility relation. Defined as:
- If `c` allows `0`: `τ` may be `Optional<τ'>` or `τ' <: τ`
- If `c` requires `1+`: `τ' <: τ` (subtype) or coercion via stdlib function
- If `c` allows multi: `Seq<τ'> <: τ` or singleton lifting

**`compatible` returns proof obligations** for refinements (facets) that cannot be statically discharged. These become runtime checks emitted into generated SQL.

---

## 6. Annotations (statements about statements)

```
                  Γ; row : R ⊢ "p = e" produces triple T_base
                  for each annotation aⱼ = vⱼ in block:
                      Γ; row : R ⊢ vⱼ : σⱼ
                      target shape declares aⱼ : σⱼ' with cardinality (over T_base subject)
                      compatible(σⱼ, σⱼ', card)
   T-Annot:  ───────────────────────────────────────────────────────────────
                  Γ; row : R ⊢ "p = e { aⱼ = vⱼ }" produces T_base plus
                              {<<T_base.subject T_base.pred T_base.object>> aⱼ vⱼ}
```

Annotations are recursive: an annotation's value can itself have annotations.

---

## 7. Implicit closure synthesis (the crucial rule)

When a function argument is expected to be a closure (Fn-typed), and the argument expression contains free `.field` references, the compiler synthesizes a single-parameter lambda.

```
                  Γ ⊢ f : Fn((Fn((τ_row) → τ_pred), ...) → τ_r)
                  Γ; row : τ_row ⊢ e : τ_pred
                  e contains at least one free FieldRef
                  row is fresh (not in Γ)
   T-Closure: ──────────────────────────────────────────────────
                  Γ ⊢ f(e, ...) : τ_r
                  
                  (closure synthesized: (row) → e[FieldRefs ↦ row.field])
```

**This is the only form of lambda in Fossil.** No user-defined multi-argument lambdas. Single-parameter closures only, only synthesized automatically when expected.

Implication: source operators (`filter`, `map`, `sort`, `distinct`, etc.) automatically receive properly-typed closures when invoked with field-referencing expressions.

```
users |> filter(.age >= 18)
   = filter(users, (row) → row.age >= 18)      // implicit
```

---

## 8. Bidirectional algorithm

The type checker uses two modes:

**Synthesis mode** `synth(Γ, ρ, e) → τ`:
- Compute the type of `e` purely from its structure.
- Used at expression entry points where no expected type exists.

**Checking mode** `check(Γ, ρ, e, τ_expected) → ()`:
- Verify that `e` has type compatible with `τ_expected`.
- Used at property assignments, function arguments.

Mode switches:
- At property assignment `p = e`: `τ_expected` comes from target shape. Use `check`.
- At source operation `e |> filter(pred)`: synthesize source type from `e`; then `check` `pred` against `Fn((row_type) → Bool)`.
- At pipeline `e |> f`: synthesize `e`, then `check` argument compatibility with `f`'s signature.

### Algorithm sketch

```
function typecheck_mapping(M):
  source_ty = synth(Γ_global, ∅, M.source_expr)
  require source_ty = Source<Record<R>>, else error
  
  target_shape = resolve_shape(M.shape_expr)
  if target_shape uses OneOf, error and suggest split
  
  iri_ty = synth(Γ_global, {row: R}, M.iri_expr)
  require iri_ty = Iri, else error
  
  for (p, e) in M.body.properties:
    expected = property_type_in_shape(target_shape, p)
    check(Γ_global, {row: R}, e, expected)
    
    for (a, av) in annotations_of(p, e):
      annot_expected = annotation_type(target_shape, p, a)
      check(Γ_global, {row: R}, av, annot_expected)
  
  if target_shape is closed:
    for p in M.body.predicates:
      require p ∈ target_shape.declared_predicates, else error
  
  for required_p in target_shape.required_predicates:
    require required_p ∈ M.body.predicates, else error
```

---

## 9. Subtyping

Minimal subtyping rules in MVP:

```
   S-Refl:    ──────────              (reflexivity)
                τ <: τ

   S-Opt:     ────────────────         (lifting to optional)
                τ <: Optional<τ>

   S-OptCov:  τ₁ <: τ₂                 (Optional covariance)
              ──────────────────
              Optional<τ₁> <: Optional<τ₂>

   S-SeqCov:  τ₁ <: τ₂                 (Seq covariance)
              ──────────────
              Seq<τ₁> <: Seq<τ₂>

   S-IntFlt:  ──────────────────       (Integer is subtype of Float for arithmetic)
                Integer <: Float
```

**No** auto-lifting `τ <: Seq<τ>` (would mask cardinality bugs).
**No** function subtyping in MVP (exact signature match).

---

## 10. Soundness (informal statement)

**Theorem (Soundness, informal).** If `Γ ⊢ M ok` for a mapping `M`, and execution of `M` against a data source whose actual types match the input descriptor's declared types terminates, then the generated RDF graph satisfies the target shape declared in `M`.

**Proof sketch (future work):** standard progress + preservation pattern. Progress: every typed `M ok` produces a valid lowering to operator algebra. Preservation: each operator preserves the schema typing. At the sink, the schema satisfies the target shape's constraints.

**Caveats:**
- Soundness is *modulo input data conformance* to the input descriptor. If the actual CSV has columns with mismatched types vs. CSVW declaration, runtime errors are possible. Provider integrity is assumed.
- Refinement constraints that emit proof obligations are not statically discharged; they become runtime checks. Soundness holds when those runtime checks pass.

---

## 11. ShEx subset supported

Fossil's `OutputDescriptor` for ShEx (via `rudof`) supports the following ShEx 2.1 features:

**Supported:**
- Triple constraints with datatype predicates
- Cardinality: `?`, `*`, `+`, `{n}`, `{n,m}`, exact `[n]`
- Shape references `@<OtherShape>`
- Node kind constraints: `IRI`, `BNode`, `Literal`
- Value sets (`[v₁ v₂ ...]`)
- Datatype facets (`MININCLUSIVE`, `MAXLENGTH`, `PATTERN`, etc.)
- Closed shapes (`CLOSED`)
- Extension (`extends`) — flattened to combined shape at check time
- Imports

**Not supported in MVP:**
- `OneOf` (disjunctive shapes) — compile error suggesting split
- Recursive shapes via direct cycles — only acyclic shape graphs
- `EachOf` complex compositions with nested grouping
- ShapeMap-specific features

---

## 12. References

- Milner, R. (1978). *A theory of type polymorphism in programming.* J. Comput. Syst. Sci.
- Pierce, B. (2002). *Types and Programming Languages.* MIT Press.
- Petricek, T. et al. (2016). *Types from data: making structured data first-class citizens in F#.* PLDI '16.
- Staworko, S., Boneva, I. (2015). *Complexity and Expressiveness of ShEx for RDF.* ICDT '15.
- Boneva, I., Labra Gayo, J. E., Prud'hommeaux, E. (2017). *Semantics and validation of shapes schemas for RDF.* ISWC '17.
