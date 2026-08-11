# Fossil — Standard Library API

**Version:** 0.1 (design phase)

This document specifies the standard library of Fossil v0.1. ~55 functions across 9 namespaces. Each function has: precise signature, semantics, and lowering strategy (how it compiles to DuckDB SQL).

## Conventions

**Type notation:**
- `T?` = `Optional<T>` (may be null)
- `T*` = `Seq<T>` (sequence, zero or more)
- `T+` = `Seq<T>` with cardinality ≥ 1
- `T | U` = sum type (only in stdlib internals, not exposed)
- `forall T.` = parametric (compiler resolves at call site via inference; not visible to user)

**Null handling:** functions taking `T` (non-optional) error at compile time if argument is `T?`. Use `default(x, fallback)` to lift `T?` to `T` first.

**Lowering kinds:**
- **builtin** — maps to DuckDB SQL builtin function directly
- **udf** — Rust UDF registered with DuckDB at runtime startup
- **inline** — compiled inline to SQL expression (not a function call)
- **plan** — affects MIR/plan structure, not just SQL expression
- **runtime** — implemented in `fossil-runtime`, outside SQL

---

## `core/` — the two that are not a term algebra

This namespace held eight. Six were RDF term constructors — `iri`, `triple`,
`blank`, `literal`, `typed`, `emit` — and no program in the corpus ever called
one; three could not even lower. They are gone, and with them the surface
`<<s p o>>` that `triple` mirrored.

### `lang`
```
lang :: (String, String) -> LangLiteral
lang :: (String, String, dir: String) -> LangLiteral
```
Construct a language-tagged literal. Optional `dir` argument for RDF 1.2 direction-tagged literals (`ltr` or `rtl`).
- **Lowering:** inline; emits literal with `lang_tag` column and optional `direction` column.

### `require`
```
require :: forall T. T? -> T
```
Assert that an optional value is present. Returns the underlying value if non-null; runtime error if null. Used when shape requires a value but the source declares nullable.
- **Lowering:** inline `CASE WHEN x IS NULL THEN error_runtime() ELSE x END`.

---

## `seq/` — Source pipeline operations

These operate on `Source<Record<R>>` values and form the primary mechanism for transforming sources.

### `filter`
```
filter :: forall R. (Source<Record<R>>, Fn((Record<R>) -> Bool)) -> Source<Record<R>>
```
Keep only rows for which the predicate holds.
- **Lowering:** plan — `FilterOp`. SQL: `WHERE <pred_compiled>`.

### `map`
```
map :: forall R, S. (Source<Record<R>>, Fn((Record<R>) -> Record<S>)) -> Source<Record<S>>
```
Transform each row. The mapper produces a new row structure. Typically used via implicit closure.
- **Lowering:** plan — multiple `ExtendOp`s + `ProjectOp`.

### `flatten`
```
flatten :: forall T. Source<Record<{value: Seq<T>}>> -> Source<Record<{value: T}>>
```
Unnest a sequence column into multiple rows. One-to-many.
- **Lowering:** plan — emits `UNNEST` in SQL.

### `take`
```
take :: forall R. (Source<Record<R>>, Integer) -> Source<Record<R>>
```
First N rows.
- **Lowering:** inline `LIMIT n`.

### `drop`
```
drop :: forall R. (Source<Record<R>>, Integer) -> Source<Record<R>>
```
Skip first N rows.
- **Lowering:** inline `OFFSET n`.

### `distinct`
```
distinct :: forall R. Source<Record<R>> -> Source<Record<R>>
distinct :: forall R. (Source<Record<R>>, by: FieldName | List<FieldName>) -> Source<Record<R>>
```
Deduplicate rows. With `by`, deduplicate based on those columns (keeps first occurrence per group).
- **Lowering:** `DISTINCT` or `DISTINCT ON (cols)`.

### `sort`
```
sort :: forall R. (Source<Record<R>>, by: FieldName | List<FieldName>) -> Source<Record<R>>
sort :: forall R. (Source<Record<R>>, by: ..., desc: Bool) -> Source<Record<R>>
```
Order rows. `desc=true` reverses.
- **Lowering:** `ORDER BY cols [DESC]`.

### `project`
```
project :: forall R, R'. (Source<Record<R>>, List<FieldName>) -> Source<Record<R'>>
```
Keep only specified columns. R' ⊆ R.
- **Lowering:** `ProjectOp` → SQL `SELECT cols`.

### `join`
```
join :: forall L, R. (Source<Record<L>>, Source<Record<R>>,
                       on: Fn((Record<L>, Record<R>) -> Bool))
                      -> Source<Record<{<left_name>: L, <right_name>: R}>>
join :: forall L, R. (Source<Record<L>>, Source<Record<R>>,
                       on: ..., kind: JoinKind) -> ...
```
Relational join. `kind` defaults to `Inner`; options are `Inner`, `Left`, `Right`, `Full`. Auto-prefix output with source names per D8.
- **Lowering:** `JoinOp` → SQL `JOIN ... ON ...`.

### `union`
```
union :: forall R. (Source<Record<R>>, Source<Record<R>>) -> Source<Record<R>>
```
Schema-compatible union. Both sources must have identical row types.
- **Lowering:** SQL `UNION ALL`.

### `group_by`
```
group_by :: forall R, K. (Source<Record<R>>, key: FieldName | List<FieldName>) 
            -> GroupedSource<Record<R>, K>
```
Group rows by key. Result is a `GroupedSource` that must be consumed by `aggregate` next.
- **Lowering:** plan — marks input for `GROUP BY` clause in subsequent aggregate.

### `aggregate`
```
aggregate :: forall R, K, A. (GroupedSource<Record<R>, K>, 
                              aggs: List<(FieldName, AggFn)>) 
              -> Source<Record<{K fields, A fields}>>
```
Apply aggregations to grouped source. Each agg pair specifies output column name and aggregation function over a column.
- **Lowering:** SQL `SELECT key_cols, agg_exprs FROM ... GROUP BY key_cols`.

### `count`
```
count :: forall R. Source<Record<R>> -> Integer       (top-level)
count :: () -> AggFn<R, Integer>                       (in aggregate context)
```
Top-level: count rows. In `aggregate(...)`: count rows in each group.
- **Lowering:** `COUNT(*)` or `COUNT(*) OVER ...` per context.

---

## `clean/` — String cleaning and normalization

### `trim`
```
trim :: String -> String
```
Remove leading and trailing whitespace.
- **Lowering:** builtin (`TRIM(s)`).

### `lower`
```
lower :: String -> String
```
Lowercase the string. UTF-8 aware.
- **Lowering:** builtin (`LOWER(s)`).

### `upper`
```
upper :: String -> String
```
Uppercase. UTF-8 aware.
- **Lowering:** builtin (`UPPER(s)`).

### `slug`
```
slug :: String -> String
```
URL-friendly slugification: lowercase, replace whitespace and non-alphanumeric with `-`, collapse multiple `-` into one.
- **Lowering:** udf (`fossil_slug`).

### `normalize_unicode`
```
normalize_unicode :: (String, form: String) -> String
```
Apply Unicode normalization. `form` is one of `"NFC"`, `"NFD"`, `"NFKC"`, `"NFKD"`.
- **Lowering:** udf (`fossil_unicode_norm`).

### `strip_html`
```
strip_html :: String -> String
```
Remove HTML tags, return text content.
- **Lowering:** udf (`fossil_strip_html`).

---

## `anon/` — Anonymization functions

### `hash`
```
hash :: String -> String
hash :: (String, salt: String) -> String
hash :: (String, salt: String, algo: String) -> String
```
Hash a string. Default algorithm `"sha256"`, default salt empty. `algo` options: `"sha256"`, `"sha512"`, `"blake3"`. Returns hex-encoded hash.
- **Lowering:** udf (`fossil_hash`). For `algo = "sha256"` with no salt, DuckDB builtin `sha256(s)` may be used.

### `hmac`
```
hmac :: (String, key: String) -> String
hmac :: (String, key: String, algo: String) -> String
```
HMAC-keyed hash. Default `algo = "sha256"`. Returns hex-encoded.
- **Lowering:** udf (`fossil_hmac`).

### `redact`
```
redact :: String -> String
redact :: (String, mask: String) -> String
```
Replace string content with a fixed mask (default `"[REDACTED]"`).
- **Lowering:** inline (`'[REDACTED]'` or literal).

---

## `validate/` — Validation predicates and converters

### `email`
```
email :: String -> String       (validate; runtime error if invalid)
email :: String? -> Email?      (overload; preserves Optional)
```
Validate that the string is a well-formed email address (RFC 5322 simplified). Returns the input unchanged if valid; runtime error if invalid.
- **Lowering:** udf (`fossil_validate_email`).

### `url`
```
url :: String -> String
```
Validate URL syntax (RFC 3986).
- **Lowering:** udf (`fossil_validate_url`).

### `uuid`
```
uuid :: String -> String
```
Validate UUID format.
- **Lowering:** udf (`fossil_validate_uuid`).

### `iso_date`
```
iso_date :: String -> String
```
Validate ISO 8601 date format.
- **Lowering:** udf (`fossil_validate_iso_date`).

### `regex`
```
regex :: (String, pattern: String) -> String
```
Validate the input matches the regex pattern. Returns input if match, error otherwise.
- **Lowering:** builtin (`regexp_matches(s, pattern)` check + identity).

---

## `parse/` — Parsing and type conversion

### `integer`
```
integer :: String -> Integer
```
Parse string to integer. Runtime error if not a valid integer.
- **Lowering:** inline (`CAST(s AS BIGINT)` with NULL check + runtime error wrapping).

### `float`
```
float :: String -> Float
```
Parse string to float. Runtime error if invalid.
- **Lowering:** inline (`CAST(s AS DOUBLE)`).

### `decimal`
```
decimal :: String -> Float       (no Decimal type in MVP, returns Float)
```
- **Lowering:** inline (`CAST(s AS DECIMAL(38,18))` then to DOUBLE).

### `date`
```
date :: (String, format: String) -> Date
```
Parse string with explicit format string (strftime-compatible). E.g., `"YYYY-MM-DD"`.
- **Lowering:** builtin (`strptime(s, format)`).

### `datetime`
```
datetime :: (String, format: String) -> DateTime
```
Same as `date` but produces full datetime.
- **Lowering:** builtin (`strptime(s, format)`).

### `json`
```
json :: forall T. (String, schema: JsonSchema) -> T
```
Parse a JSON string into a structured value matching the schema. The schema is provided as type annotation in context (typically from the target shape).
- **Lowering:** builtin (`json_extract` + recursive parsing).

### `csv_row`
```
csv_row :: (String, delimiter: String) -> Record<{c1: String, c2: String, ...}>
```
Parse a CSV row string into a record. Mostly used for inline CSV data in test cases.
- **Lowering:** inline (`split_part` chain).

---

## `math/` — Arithmetic and aggregation

### Aggregation forms (used inside `aggregate`)

### `sum`
```
sum :: forall T : Numeric. Fn((Record<R>) -> T) -> AggFn<R, T>
```
Sum values across grouped rows. Implicit closure: `sum(.field)` sums field values.
- **Lowering:** `SUM(field_expr)` in GROUP BY.

### `avg`
```
avg :: forall T : Numeric. Fn((Record<R>) -> T) -> AggFn<R, Float>
```
Average. Always returns Float.
- **Lowering:** `AVG(field_expr)`.

### `min`
```
min :: forall T : Comparable. Fn((Record<R>) -> T) -> AggFn<R, T>
```
Minimum.
- **Lowering:** `MIN(field_expr)`.

### `max`
```
max :: forall T : Comparable. Fn((Record<R>) -> T) -> AggFn<R, T>
```
Maximum.
- **Lowering:** `MAX(field_expr)`.

### Scalar arithmetic

### `abs`
```
abs :: forall T : Numeric. T -> T
```
- **Lowering:** builtin (`abs(x)`).

### `round`
```
round :: Float -> Integer
round :: (Float, digits: Integer) -> Float
```
- **Lowering:** builtin (`round(x[, digits])`).

---

## `str/` — String operations

### `length`
```
length :: String -> Integer
```
Character count (Unicode-aware, code points).
- **Lowering:** builtin (`length(s)` in DuckDB; UTF-8 character count).

### `slice`
```
slice :: (String, start: Integer) -> String
slice :: (String, start: Integer, end: Integer) -> String
```
Substring. `start` is inclusive, `end` exclusive. Negative indices count from end (Python-like).
- **Lowering:** builtin (`substring(s, start[, end-start])` with negative handling).

### `contains`
```
contains :: (String, needle: String) -> Bool
```
- **Lowering:** builtin (`contains(s, needle)`).

### `starts_with`
```
starts_with :: (String, prefix: String) -> Bool
```
- **Lowering:** builtin (`starts_with(s, prefix)`).

### `ends_with`
```
ends_with :: (String, suffix: String) -> Bool
```
- **Lowering:** builtin (`ends_with(s, suffix)`).

### `replace`
```
replace :: (String, from: String, to: String) -> String
replace :: (String, pattern: Regex, to: String) -> String
```
Literal or regex replacement.
- **Lowering:** builtin (`replace` or `regexp_replace`).

### `split`
```
split :: (String, sep: String) -> Seq<String>
```
- **Lowering:** builtin (`string_split(s, sep)`).

### `concat`
```
concat :: forall n. (String × n) -> String
```
Concatenate multiple strings. Note: template literals are usually preferred; this is for variadic concat.
- **Lowering:** builtin (`concat(s1, s2, ...)`).

---

## `io/` — Source constructors

These are not "functions" in the strict sense — they construct Sources at parse-time and are recognized by the compiler specially. Listed here for completeness.

### `csv`
```
csv :: String -> Source<Record<R>>
csv :: (String, schema: String) -> Source<Record<R>>     (CSVW descriptor path)
csv :: (String, delimiter: String, header: Bool) -> Source<Record<R>>
```
- **Lowering:** plan — `SourceOp(path, CSV, ...)`. SQL: `read_csv_auto(path, ...)`.

### `json`
```
json :: String -> Source<Record<R>>
json :: (String, schema: String) -> Source<Record<R>>    (JSON Schema descriptor)
```
- **Lowering:** plan — `SourceOp(path, JSON, ...)`. SQL: `read_json_auto(path)`.

### `parquet`
```
parquet :: String -> Source<Record<R>>
```
Schema embedded in Parquet file; no descriptor needed.
- **Lowering:** plan — SQL `read_parquet(path)`.

### `sql`
```
sql :: String -> Source<Record<R>>
sql :: (connection_string: String, table: String) -> Source<Record<R>>
sql :: (connection_string: String, query: String) -> Source<Record<R>>
```
DB connection. Schema introspected via SQL catalog at compile time (sqlx-style; offline manifest for WASM context).
- **Lowering:** plan — `SourceOp(uri, SQL, table_or_query)`. SQL: depends on backend.

### `http`
```
http :: String -> Source<Record<R>>
http :: (url: String, auth: Auth, headers: Record<...>) -> Source<Record<R>>
```
Fetch from REST endpoint. Schema inferred from response or provided via `schema:` option (OpenAPI/JSON Schema).
- **Lowering:** runtime — fetched during execution into a temp DuckDB table, then operated on.

---

## Function categories summary

| Namespace | Count | Lowering profile |
|---|---|---|
| `core/`       | 2 | inline |
| `seq/`        | 12 | mostly plan (operator algebra) |
| `clean/`      | 6 | mostly builtin, some udf |
| `anon/`       | 3 | udf |
| `validate/`   | 5 | udf |
| `parse/`      | 7 | mostly builtin |
| `math/`       | 6 | builtin |
| `str/`        | 8 | builtin |
| `io/`         | 5 | plan (source constructors) |
| **Total**     | **~59** | |

---

## Implementation strategy

**Phase A (MVP critical path):**
1. `seq/` — entire namespace (pipeline operations). No mapping works without these.
2. `core/` — `lang`, `require`.
3. `io/csv` — minimum source.
4. `clean/{trim, lower}` + `parse/{integer, float, date}` — most common transforms.

**Phase B (round out):**
5. Remaining `clean/`, `parse/`, `str/`, `math/`.
6. `validate/` namespace.

8. `anon/` namespace.
9. `io/{json, parquet}`.

**Phase C (post-MVP):**
10. `io/{sql, http}` — require provider infrastructure for compile-time introspection.

---

## Notes for type checker integration

- **Implicit closures** apply throughout: any function taking `Fn((T) -> U)` accepts an expression with free `.field` references. `type-system.md` §7.
- **Generic functions** (`forall T.`) resolve at call site by inference. The user does not see polymorphism in surface syntax. Compiler chooses concrete types from context.
- **Refinement-typed return values:** functions like `validate.email` may return refined `String where matches email_regex` for downstream checking. MVP simplification: refinement is enforced at function boundary (runtime check) but not propagated through.

---

## Notes for the registry implementation

Each function is registered with `FunctionRegistry` at runtime startup with:
```rust
RegistryEntry {
    name: "clean.trim",
    signature: parse_sig("String -> String"),
    impl_kind: BuiltinImpl::DuckDbBuiltin("trim".to_string()),
}
```

UDFs are registered with DuckDB:
```rust
duckdb_conn.create_scalar_function(
    "fossil_slug",
    vec![DataType::Utf8],
    DataType::Utf8,
    rust_impl_slug,
)?;
```

The codegen consults the registry to know whether to inline a function call, use a SQL builtin name, or call a UDF.
