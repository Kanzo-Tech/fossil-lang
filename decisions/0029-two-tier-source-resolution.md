# ADR 0029: Two-tier source resolution via injected `ConnectionResolver`

**Date:** 2026-05-24
**Status:** accepted
**Decider:** Ángel Iglesias Préstamo
**Cite:**
- `.planning/research/playground-sources-design.md` §3
- `.planning/research/playground-sources-data-tools.md` §"Synthesis — patterns recurrentes" (dbt source/profile split; `env_var` lingua franca; OAuth2 PKCE + backend; Superset re-encrypt-secrets)
- `.planning/research/playground-sources-rdf-tools.md` §"Synthesis" + §"Implications" (universal pattern: credentials never in mapping/query text; Matey as the honest browser-side compromise; WESO/RDFShape chose backend)
- `memory/reference_keasy_connectors.md` — the Keasy Connector / PathResolver / `@nombre/path` model that anchors the surface choice
- ADR-0006 (`OutputDescriptorKind` enum dispatch + descriptor-as-argument pattern — same shape applied to sources)
- ADR-0020 (descriptor accessor wiring through `HirDb` extension trait — the threading mechanism reused here)
- ADR-0028 (React library — the home of the resolver injection point)
- Phase 7 SC#4 + PLAY-12 (memory leak mitigation) — context for the source/upload limits this design addresses
- PROJECT.md "Out of Scope (Milestone 1)" — `io/sql` and `io/http` as source constructors (this ADR explains how `@source/path` is NOT a violation of that scope)

## Context

The original Phase 7 plan treated sources as inline panels (paste CSV;
paste CSVW; paste ShEx). The 2026-05-24 redesign conversation rejected
that model in favour of `@nombre/path` references resolved at runtime,
following the Keasy Connector pattern. The forces in tension:

1. **The component is a reusable React library** (ADR-0028) with multiple
   hosts (OSS landing, Keasy, paper demos, docs). Each host has a
   different credentials story: Keasy has a backend vault, the landing
   has nothing, paper demos have a local fixture. The component cannot
   own a credentials model that fits all three.

2. **Browser-only credential handling is fundamentally insecure** for
   anything beyond demos. Storing S3 access keys in localStorage means
   any XSS leaks them; any inline use means they appear in network
   tabs. Per the RDF-tools research, every browser-only tool that
   touched non-public data either (a) accepted localStorage-plaintext
   as the trade-off (Stardog Studio, Neo4j Browser) or (b) eventually
   shipped a proxy backend (RDFShape, Matey).

3. **Hard-coding a backend dependency disqualifies hosts that don't
   have one** — the OSS landing, paper demos, and docs sites cannot
   stand up a vault just to show a mapping. Public-only sources MUST
   work without any backend at all.

4. **The compiler IR must not carry credential material.** Every
   surveyed tool agrees: credentials never appear in the mapping text;
   only logical names do. This is enforced at the language level —
   `@connector/path` is a logical reference, the resolver supplies
   the URL with credentials embedded (presigned URL, signed cookie,
   etc.) at execution time.

5. **`io/sql` and `io/http` remain Out of Scope** per PROJECT.md.
   `@connector/path` is NOT a new source constructor — it is a
   reference syntax orthogonal to the `io/*` family. Currently it
   appears as a string literal inside existing `io/parquet(...)` /
   `io/csv(...)` / `io/json(...)` calls (e.g.
   `io/parquet("@my-conn/file.parquet")`). The resolver maps the
   string to a fetchable URL the existing `io/*` constructors
   consume. No new IR ops, no new grammar productions.

## Decision

Source resolution is **two-tier with host-injected `ConnectionResolver`**.

**Tier 1 — browser-local.** Bundled examples, browser file uploads, and
public CORS-enabled HTTPS URLs work without any host setup. Tier 1 uses
no credentials. The default resolver shipped in `@fossil-lang/resolvers`
covers this tier.

**Tier 2 — host-mediated.** The component accepts a `ConnectionResolver`
prop. The host implements the resolver to fit its security model. Keasy
enchufa its existing backend (Connectors + ChaCha20Poly1305 vault +
presigned URL signer). The landing uses Tier 1. Paper demos use a local
fixture resolver. The component NEVER sees plaintext credentials.

The `ConnectionResolver` interface (TypeScript):

```typescript
interface SourceRef {
  raw: string;        // text after `@`, e.g. "my-conn/file.parquet"
  connector: string;  // "my-conn"
  path: string;       // "file.parquet"
}

interface ResolvedSource {
  url: string;                            // browser-fetchable
  format?: 'csv' | 'json' | 'parquet';    // codegen hint
  schema?: SourceSchema;                  // optional IDE preview
}

interface ConnectionResolver {
  resolve(ref: SourceRef): Promise<ResolvedSource>;
  list(): Promise<Connector[]>;
  openManager?(): void;                          // optional host UI hook
  subscribe?(listener: (e) => void): () => void; // optional cache invalidation
}
```

Reference syntax in `.fossil` mappings, v0.1: `@connector/path` as an
opaque string passed into existing `io/*` source constructors. Naming
rule (borrowed from Keasy): `^[a-z0-9][a-z0-9-_]*$i`. First-class grammar
promotion is deferred to a dedicated future phase (will require type-
checker integration, did-you-mean for connector names, resolver-aware
schema introspection).

The compiler treats `@`-prefixed strings as opaque tokens at parse / type-
check time; the resolver is invoked at codegen time (component-side, not
compiler-side) to substitute the resolved URL into the emitted SQL.

The same `@connector/path` grammar serves both tiers. A host can later
switch from a Tier-1 default resolver to a Tier-2 backend resolver
without changing a single mapping.

## Consequences

**Positive.**

- The component is host-agnostic and credential-free. Keasy enchufa a
  vault, the landing enchufa public buckets, paper demos enchufa
  local fixtures — same mapping text in all three.
- Credentials never appear in the compiler IR, the LSP, the playground
  UI, or any persisted artefact (permalinks, BibTeX exports, screenshots).
- Tier 1 covers the OSS demo path without any backend obligation —
  the playground works standalone for public data on day one.
- Keasy's existing backend (Connectors + vault + presigned URLs) becomes
  one of many possible Tier-2 implementations. Other hosts can adopt
  whatever vault they already have (AWS Secrets Manager, HashiCorp
  Vault, Azure Key Vault, env-var-driven dev mock).
- Mappings are portable across security postures — the same `.fossil`
  file runs against public buckets, Keasy-backed connectors, or a dev
  mock without textual change.
- Aligns with the universal pattern surveyed across 11 data tools
  (dbt, Observable, Hex, Steampipe, etc.) — separation of *logical
  source name* from *credential material*.

**Negative.**

- The component cannot perform schema introspection without a working
  resolver. Tier-1 default behaviour is "no schema until file uploaded
  or example loaded". Tier-2 schemas depend on the host implementation
  (Keasy can introspect via its backend; a minimal resolver returns
  no schema). Mitigated by the optional `schema?` field on
  `ResolvedSource` — hosts that can provide schemas do, hosts that
  cannot don't, and the IDE features (autocomplete for shape
  properties; goto-source) gracefully degrade.
- `@connector/path` syntax is essentially novel in the RML/SPARQL
  community (RDF-tools research: only TigerGraph's `LOAD "$alias:path"`
  is precedent). Documentation must explain the pattern; the audience
  has no muscle memory to fall back on. Mitigated by examples and a
  dedicated docs section.
- The resolver is async — every source reference invokes a Promise
  during codegen. For browser-side hot-reload UX this is fine
  (resolution is batched per Run); for hypothetical streaming use
  cases the model would need extension.
- A mapping with N source references → N resolver calls per Run.
  Resolver implementations are expected to batch / cache; the
  component does not enforce caching (host responsibility).

**Neutral.**

- The compiler is unchanged: `@`-prefixed strings are opaque values
  to the parser, type checker, MIR, and SQL codegen. The substitution
  happens in the component before the SQL is dispatched to DuckDB-WASM.
- The 10MB browser CSV cap (PLAY-12, was Phase-7 plan 07-08) is moot
  for Tier-2 sources (the cap exists for direct paste/upload, not
  for resolver-driven URLs). It remains relevant for Tier-1 file
  uploads only.
- `io/sql` and `io/http` stay Out of Scope. The resolver is a runtime
  resolution layer over existing `io/parquet|csv|json` constructors,
  not new source kinds.

## Alternatives considered

1. **Hard-code a backend dependency (the playground talks to a fixed
   Fossil server API).** Rejected — disqualifies hosts without a
   backend (landing, demos, docs); couples release cadence; conflicts
   with the reusable-library shape (ADR-0028).

2. **Browser-only with IndexedDB + WebCrypto for credentials.** Considered;
   acceptable for dev-mode "save my dev token" but not for production
   private-data flows (XSS exposure, no rotation story). May appear
   as an opt-in resolver impl in `@fossil-lang/resolvers` for the
   landing's "advanced mode", clearly labelled dev-only — but not
   the default path.

3. **Public-only forever (no Tier 2).** Rejected — Keasy's primary
   use case is private-data mappings; reducing the playground to
   public-only would make the Keasy migration impossible.

4. **First-class `@connector/path` grammar in v0.1.** Rejected —
   requires type checker changes, IR additions, did-you-mean infra,
   schema-aware autocomplete — out of scope for the Phase 8 rebuild.
   Deferred to a dedicated future phase with explicit type-checker
   integration work.

5. **Inline credential blocks in mappings (`@connector/path with
   aws_key='...'`).** Rejected categorically — credentials in mapping
   text violates every surveyed precedent and produces immediate
   security incidents on copy-paste.
