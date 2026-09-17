# @fossil-lang/draw

The half of drawing a corpus that is not a renderer. **Fossil ships no viewer**,
and this is not one: no canvas, no context, no GPU, no frame loop, no camera.

It answers the two questions a renderer has to ask before it can draw and that
only the corpus can answer.

## What is it drawn with

```ts
import { channelsFor, encodingFor } from '@fossil-lang/draw';

const channels = channelsFor(Object.values(manifests), 'Person');
const encoding = encodingFor({ types: corpus.types, channels, quasiIdentifiers });

encoding.fill    // the categorical to colour by, or null
encoding.brush   // the numeric column a histogram bins, or null
encoding.label   // the subject IRI column, when the payload carries one
encoding.columns // what a crossfilter view has to project, deduped, in reading order
```

A vertex type's `channels:` block has three states and all three are read: **no
key** (a corpus written before the field, and the derivation answers as it did
before the field existed), **an empty list** (the writer declares the type carries
no channel), **a list** (the answer). A declared channel outranks the privacy
block's `quasi_identifiers`, which outranks the bytes; a corpus with none of the
three gets a canvas of one colour rather than an exception.

## What is already loaded

```ts
import { residency } from '@fossil-lang/draw';

const held = residency();
held.hold(key, await corpus.frame({ ...box, pixels }));
held.held(key);                 // a rectangle already answered is not re-asked
held.visible(nextBox, 20_000);  // an interim picture out of answers in hand — no query
```

`/docs/design/camera` keeps residency (what is loaded), visibility (what is drawn
out of what is loaded) and the camera apart. This is the first two. It issues no
query, knows no rectangle it was not handed, and holds **answers** rather than
tiles — the bytes of a tile are never in the calling thread, they are in whatever
engine the door was given.

The interim is **superseded, never merged**: it is cut by a power-of-two stride on
`dense_id`, the same family the door's `4^k` is a subset of, so a vertex it drew
stays drawn at every finer cut. That is what makes leaving it on screen correct
rather than merely better than a gap.

## What this package is not

- **Not the door.** `@fossil-lang/corpus` is the artefact's contract; this is a
  policy over it. A corpus does not have an opinion about which of its columns
  deserves a histogram, and the 500,000-row residency budget is a number measured
  against one renderer on one corpus.
- **Not a renderer binding.** Nothing here names a renderer. Translating an
  `Encoding` and a `Visible` into some particular contract's arrays is a seam, and
  a seam belongs to whoever is on both sides of it — in fossil's own tree that is
  `apps/playground/src/tiles.ts`, which is the app and stays there.
- **Not wasm.** Nothing here instantiates a module, issues a request or needs an
  engine. Its one runtime dependency is `@fossil-lang/corpus`, for three string
  arrays off the writer's generated role table.
