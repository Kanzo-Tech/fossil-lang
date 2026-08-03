# ADR 0040: Expose fossil through protocols only — the UI family retires to `@kanzo-tech/*`

**Date:** 2026-07-31
**Status:** proposed
**Decider:** Angel Iglesias (Kanzo)
**Cite:** ADR-0001 (LSP framework — `fossil-ide` logic vs `fossil-lsp` protocol); ADR-0039 (query surface / transport split; deck slide 22 "Serverless · Larger-than-RAM · Chunking"); ADR-0038 (cross-repo consumption, superseded by the npm publish); ADR-0031 (pnpm monorepo restructure); `kanzo-ui/BENCHMARKS.md` (measured graph scale, 2026-07-31); `kanzo-ui/DESIGN.md` admission rules.

## Context

Fossil already exposes everything a client needs, and it exposes it as protocol.
`fossil-lsp` advertises `hover`, `definition`, `completion`, `document_symbol`,
`code_action` and **`semanticTokens/full`**; ADR-0039 adds a closed surface of
fourteen typed graph verbs whose bindings are MCP, HTTP, CLI and an in-process TS
package. Those two surfaces are the product. Neither of them draws anything.

Beside them the repository also ships three React packages — `@fossil-lang/ui`
(Radix primitives and a second set of design tokens), `@fossil-lang/viewer` (a
cosmos.gl WebGL graph view) and `@fossil-lang/editor` (a CodeMirror 6 shell) —
plus `@fossil-lang/codemirror-fossil`, a CodeMirror language extension that
highlights Fossil syntax. These accumulated rather than being designed: the viewer's
own description records it as *"promoted from Keasy's discovery view"* in Phase 12,
and ADR-0038's history shows Phase 16-03 swapping keasy onto it afterwards. The
graph view therefore travelled keasy → fossil, and its ownership was settled by
migration order rather than by architecture. It came to rest in the one repository
that is neither its owner nor its consumer.

Each of the four now duplicates something that exists, and is better evidenced,
elsewhere:

- `@kanzo-tech/ui` is a design system with a token pipeline, a per-tenant palette
  document and an accessibility budget enforced by tests. `@fossil-lang/ui` is a
  second answer to that question with its own tokens.
- `@kanzo-tech/graph` owns the cosmos.gl layer behind a benchmark that measures the
  engine and the pipeline feeding it separately. Its numbers say where a live layout
  stops being viable — about 200,000 points — which is the larger-than-RAM boundary
  ADR-0039 argues from. `@fossil-lang/viewer` carries its own `CosmosGraph`,
  `GraphCanvas` and `getAdaptiveConfig` with no equivalent evidence.
- `@kanzo-tech/ui` already exposes a CodeMirror `EditorShell` on a Radix-free
  subpath, built for brand-agnostic hosts. `@fossil-lang/editor` is a second shell.
- **Highlighting is already a protocol answer.** `semanticTokens/full` is served by
  the LSP, so `codemirror-fossil` is a second implementation of a question the
  server answers authoritatively. The usual objection — that a local grammar paints
  faster than a round trip — is weak here, because `fossil-wasm` puts the server in
  the same tab as the editor.

`@fossil-lang/viewer` **already declares `@kanzo-tech/ui` as a dependency**, so half
of this boundary has been conceded in practice for some time.

The counter-argument was weighed and does not apply. ADR-0039 insists GraphAr-layout
knowledge live in exactly one place, and a client reading the manifest itself would
put it in two. But `@fossil-lang/graph` **is not UI and does not move**, and its
contract already delegates execution to a host-provided DuckDB-WASM callback. The
split drawn here is the one that package was designed for.

## Decision

We will expose fossil through protocols only — the LSP and the graph verb surface —
and ship no user interface from this repository.

Retire `@fossil-lang/ui`, `@fossil-lang/viewer`, `@fossil-lang/editor` and
`@fossil-lang/codemirror-fossil`. Consumers take `@kanzo-tech/ui` and
`@kanzo-tech/graph`; syntax highlighting comes from `semanticTokens`, which is
already implemented.

Keep everything that is not an interface: `@fossil-lang/graph`, `executor`, `wasm`,
`resolvers`, `introspect`, `types` and `examples`.

The viewer's cosmos.gl internals fold into `@kanzo-tech/graph` —
`getAdaptiveConfig` in particular is level-of-detail policy and belongs beside the
renderer that pays for it. Its tab arrangement (Graph / Turtle / Vertices / Edges)
is an arrangement, and arrangements live at the call site.

The resulting shape has three roles and no overlap: `@kanzo-tech/ui/analytics`
provides the DuckDB coordinator, `@kanzo-tech/graph` draws, and `@fossil-lang/graph`
translates verbs to SQL over the manifest.

## Consequences

**What becomes easier**

The boundary stops being a matter of taste and becomes a protocol. "Does this belong
in fossil?" is answered by "can it be expressed as LSP or as a verb?", which is a
question with an answer rather than an opinion.

One design system, so a tenant palette reaches every surface without a token bridge.
One cosmos.gl integration, held to one set of measurements instead of two
implementations drifting. Fossil's dependency surface loses React, Radix, CodeMirror
and a WebGL renderer — the honest footprint of a compiler with two protocol
surfaces.

**What becomes harder**

Release sequencing, and it is not a detail. `@fossil-lang/*` is published to npm and
keasy consumes it through the `alpha` dist-tag; `@kanzo-tech/*` is unpublished.
Nothing can be deleted before its replacement is installable, so the order is fixed:
publish `@kanzo-tech/ui` and `@kanzo-tech/graph` → migrate keasy's imports → retire
the fossil packages. Two repositories acquire the release coupling ADR-0031
deliberately kept them free of.

An editor built on `semanticTokens` alone also has one behaviour a local grammar does
not: a document is unhighlighted until the server has parsed it. In-tab WASM makes
that brief, but it is not zero, and it is the one user-visible regression in this
decision.

**New risks**

*Licensing was raised as the blocking risk and is settled.* ADR-0038 records fossil
as OSS and keasy as closed-source SaaS, so the question was whether an open-source
playground could depend on `@kanzo-tech/*` at all. It can: **the Kanzo packages will
be published publicly**, and all four repositories are owned by the same party, so
there is no third-party licence to negotiate and no dual-licensing to arrange. The
constraint this leaves is ordinary rather than structural — the publish must land
before the first deletion, which the sequencing above already requires.

`@kanzo-tech/graph` also inherits a consumer whose needs it has not met. Nothing in
it speaks the verb surface, and the bounded render path ADR-0039 requires —
`viewport`, with its level-of-detail switch to aggregate mode — is not implemented on
the rendering side. Today's `load()` materialises the whole relation, which is the
pattern ADR-0039 names as topping out near a million vertices. Measured: 389 ms at
200,000 nodes, of which 158 ms is remapping edges through an id→index map that the
verb surface's `dense_id` would make unnecessary. Absorbing the viewer without
adopting the bounded path moves the ceiling rather than raising it.
