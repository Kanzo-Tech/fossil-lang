# `fossil-syntax` parser corpus

30-fixture parser corpus that drives `tests/parse_corpus.rs`. Layout follows
RESEARCH.md §Q10 (5 buckets × 6 fixtures = 30, plus a separate 10-mapping
invalidation pair in `crates/fossil-hir/tests/fixtures/`).

## Bucket layout

Each bucket has 6 fixtures: 3 happy-path (NN_*) and 3 recovery (NN_*_recovers).

| Bucket | Path | Theme |
|--------|------|-------|
| 1 | `01_pipeline_postfix/`     | Pratt L1 (`\|>`) + L9 (postfix, calls, member access, partial app) |
| 2 | `02_ternary_arithmetic/`   | Pratt L2 (ternary) through L7 (arithmetic) precedence walk |
| 3 | `03_mappings_annotations/` | Mapping headers with `in`/shape intersection + annotation blocks |
| 4 | `04_prefix_iri_triple/`    | Imports, exported definitions, IRI templates, RDF 1.2 triple terms |
| 5 | `05_toplevel_indent/`      | Multiple top-level items, record literals, INDENT/DEDENT edge cases |

## File pairs

Every `NN_<name>.fossil` source has a paired `NN_<name>.cst.txt` snapshot:

- **Wave 0** (plan 02-01) ships placeholder snapshots — every `.cst.txt`
  contains the single line `PLACEHOLDER — Wave 1 wires fossil_syntax::parse`.
  The 30 driver tests still PASS because placeholder equals placeholder; the
  point of Wave 0 is to lock the corpus layout and the test wiring.
- **Wave 1** (plans 02-02 + 02-03) implements the Pratt sub-parser and the
  full item parser, then regenerates real CST snapshots via
  `UPDATE_EXPECT=1 cargo test -p fossil-syntax --test parse_corpus`.
- **Wave 4** (plan 02-07) audits that NO fixture still carries the
  `PLACEHOLDER` text — every `.cst.txt` must reflect a real CST.

## Regenerating snapshots

```bash
UPDATE_EXPECT=1 cargo test -p fossil-syntax --test parse_corpus
```

Per CLAUDE.md Style: snapshot tests use `expect-test` for parser CSTs and
SQL codegen output (not `insta`). `insta` is reserved for cases where
sub-second iteration matters less than diff ergonomics.

## Convention for new fixtures

If a sixth bucket is added, the rule is: 3 happy + 3 recovery per bucket.
The driver `parse_corpus.rs` uses a `fixture_test!` macro with `(name, bucket,
stem)` arguments, so adding a fixture is a one-line addition to the macro
table.
