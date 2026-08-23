# The conformance corpus

The guards next door check that a corpus **is** what the conventions say. This directory checks that
two readers **address** it the same way, which is a different failure and a quieter one: a URL
composed from a stale copy of a convention 404s at runtime, in a browser, with no type error and
nothing red.

```bash
node verify.mjs                                      # from apps/corpus/conformance/
node writer.mjs --fossil ../../../target/release/fossil
```

Exit `0` when every address reproduces, `1` when one does not, `2` when a harness could not run.

## What is here

| | |
| --- | --- |
| `expected.json` | **the deliverable.** Every address that must compose, and every one that must be refused. |
| `corpus/` | 300 vertices in five tiles of 64, both orientations tiled. Real Parquet, real manifests. |
| `manifests/` | four manifest sets with no payload, each holding a case a whole corpus cannot. |
| `reader.mjs` | the addressing, in plain Node with no npm and no build. One reader, two harnesses. |
| `verify.mjs` | the reader against `expected.json` — catches **two readers drifting apart**. |
| `writer.mjs` | the reader against a corpus `fossil run` just wrote — catches **both readers being wrong together**. |

The other implementation of the table is `packages/graph/tests/conformance.test.ts`, which runs the
published `resolveCorpus`. **Neither implementation wrote it**, and a change on either side that
moves an address moves it away from the other.

## Why a table is not enough, and what `writer.mjs` adds

`expected.json` was written by whoever read the conventions last, and the corpus under it was
written by `guards/fixture.mjs`. Two readers agreeing about a corpus no compiler produced is a
closed loop with the *writer* outside it: a convention both readers copied from the same stale
sentence stays green forever, and so does a change to fossil's tiling that neither reader heard
about. `writer.mjs` runs `fossil run` and points `reader.mjs` — the same reader, not a third copy —
at the bytes that come out.

Five breaks it has been seen to catch, each on a corpus fossil wrote: a tile with a hole in the
middle, a tile missing from the tail, a `chunk_size` the placement disagrees with, an orientation
ordered on the wrong endpoint column, and a payload file no address reaches.

**Three things it cannot reach.** `fossil run` has no `--chunk-size` — `DEFAULT_CHUNK_SIZE` is 4,096
and nothing on the CLI moves it — so a reader that hard-codes 4,096 passes it, and the 64-row case
below stays with the fixture. The cross-type, CSR-only and unaddressable manifests are shapes fossil
has no program to emit. And it compares a reader against a writer: if both are wrong in the same way
the loop is still closed, which is what `expected.json` is for. Neither harness subsumes the other.

`--corpus <dir>` skips the run and checks a tree that is already there. That is how the harness is
proved red, and it is the only form that reaches a corpus **some other writer** produced.

## The cases

- **`corpus`** — the whole artefact, and the only one with bytes. `node ../guards/check.mjs corpus`
  passes all fifteen guards, so every address the table lists is checked to name a file that is on
  disk. The last tile is deliberately partial — 44 rows of 64 — because a corpus whose count divides
  the tile size exactly never exercises the boundary.
- **`csr-only`** — one `adj_lists` entry, `aligned_by: src`. This is the shape every corpus written
  before the target half was tiled has. A reader that composes `by_target/tile{k}.parquet` from the
  convention rather than from the manifest gets a 404; the address has to come back as *absent*.
- **`unaddressable`** — both orientations declared, and the target one carries no `prefix`. `prefix`
  is the one part of a tile's URL a reader cannot compute, so an entry without one declares a
  capability with no address behind it — and reads exactly like the tiled case to anything counting
  entries.
- **`cross-type`** — `Author authored Paper`, the two types on different tile sizes. `by_target` tile
  1 of that edge is `Paper`'s tile 1, which has nothing to do with `Author`'s tile 1, so a window
  over `Author` must not read it. This is where one shift for both endpoints stops being right.
- **`mismatched-tiling`** — an edge declaring tiles of 8,192 against a vertex type cut at 4,096.
  Every URL still composes and most of the files it names still exist, which is why it is refused
  when the corpus is opened rather than at the first request.

## Why `chunk_size` is 64

4,096 is a measured trade-off between requests and bytes on a corpus of millions, not an invariant —
the convention is that `chunk_size` is a power of two. A corpus that declares another one is
addressed by exactly the same arithmetic, and **a reader that hard-codes the shift passes every test
written at 4,096.** So the corpus here declares 64, which makes it small enough to read by hand and
makes the shift a thing that has to come from the manifest.

## Rewriting the corpus

```bash
node ../guards/fixture.mjs corpus --vertices 300 --clusters 16 --layout files --chunk-size 64
node ../guards/check.mjs corpus
node verify.mjs
```

`--layout files` because the file-per-tile container is the one with a per-tile URL to compose. The
other container — one file, one row group per tile — is addressed by the row-group ordinal, and no
manifest field distinguishes the two; that is the live design question on
`/docs/conventions/addressing`, not something a reader resolves.
