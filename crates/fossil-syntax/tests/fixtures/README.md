# `fossil-syntax` parser corpus

18-fixture parser corpus that drives `tests/parse_corpus.rs`. Layout follows
RESEARCH.md §Q10, minus one bucket (plus a separate 10-mapping invalidation pair
in `crates/fossil-hir/tests/fixtures/`).

**Every fixture is written in the spelling `grammar.bnf` specifies.** That is a
guard, not a claim: `no_fixture_spells_a_retired_form` parses all 18 and fails if
any one of them produces a `RetiredSpelling` diagnostic. Regenerating a snapshot
with `UPDATE_EXPECT=1` therefore cannot bake a dead form into the baseline.

| Bucket | Path | Theme |
|--------|------|-------|
| 1 | `01_pipeline_postfix/`     | Pratt `\|>` + postfix (calls, member access) |
| 2 | `02_ternary_arithmetic/`   | Ternary through arithmetic, the precedence walk |
| 3 | `03_mappings_annotations/` | Mapping headers, property keys, string interpolation |
| 5 | `05_toplevel_indent/`      | Multiple top-level items, INDENT/DEDENT edge cases |

## The counts, and what moved them

30 → 28 → 20 → 18. Two went with partial application (`map(_, f)`) and `@export`;
eight with the forms the HIR never read; three with the CURIE and the backtick.

**Bucket 4 (`04_prefix_iri_triple/`) is gone entirely** — imports, IRI templates
and RDF 1.2 triple terms, and every one of the three left the language. Its last
two fixtures and the one deleted from bucket 1 are named in `parse_corpus.rs`'s
module header, each with the form it proved and the `grammar.bnf` tombstone that
retired it. A fixture whose subject the language no longer has is deleted rather
than rewritten: converting it would give it a history it does not have.

`|>` is the one retired spelling still WRITTEN here, and deliberately: ruling 7
of 2026-08-11 kills it and step 6 of `SURFACE-PLAN.md` removes it, so bucket 1
spells what this parser still accepts. It goes when the verbs become a catalogue.

## File pairs

Every `NN_<name>.fossil` source has a paired `NN_<name>.cst.txt` snapshot.

## Regenerating snapshots

```bash
UPDATE_EXPECT=1 cargo test -p fossil-syntax --test parse_corpus
```

Per CLAUDE.md Style: snapshot tests use `expect-test` for parser CSTs and
SQL codegen output (not `insta`). `insta` is reserved for cases where
sub-second iteration matters less than diff ergonomics.

## Convention for new fixtures

If a bucket is added, the rule is: 3 happy + 3 recovery per bucket. The driver
uses a `fixture_test!` macro with `(name, bucket, stem)` arguments, so adding a
fixture is a one-line addition to the macro table — plus the count in
`no_fixture_spells_a_retired_form`, which is asserted so the table and the disk
cannot drift apart.
