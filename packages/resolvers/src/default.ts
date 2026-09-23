import type {
  ConnectionResolver,
  SourceRef,
  ResolvedSource,
  Connector,
} from '@fossil-lang/types';

/**
 * Connector-name validation rule (borrowed from Keasy):
 * `/^[a-z0-9][a-z0-9_-]*$/i` — applied at parseSourceRef time. Resolvers
 * MUST reject invalid names with a clear error rather than silently coercing.
 */
const CONNECTOR_NAME_REGEX = /^[a-z0-9][a-z0-9_-]*$/i;

/**
 * Options for {@link createDefaultResolver}.
 *
 * The Tier-1 default resolver — works standalone, no credentials, suitable for
 * landing/docs/paper-demo hosts. (Tier 2 is a host-mediated resolver injected
 * by a host that has a credentials story of its own.) Three independent paths,
 * each opt-in:
 *
 *  - `examples`: a fixture map keyed by path (`@examples/<path>`). Values
 *    can be string contents or pre-constructed Blobs (e.g., for binary
 *    parquet bundles). The resolver creates a `blob:` URL on demand.
 *  - `publicBuckets`: HTTPS host-prefix allowlist for direct CORS fetches
 *    (`@public/<full-url>`). The allowlist prevents the resolver from
 *    laundering arbitrary URLs — only explicitly trusted prefixes pass.
 *  - `allowLocalFiles`: enables `@uploads/<key>` references the host
 *    seeds via {@link DefaultResolverHandle.addUpload} (typically wired
 *    to a `<input type="file">` element in the host UI).
 */
export interface DefaultResolverOpts {
  /** Bundled example fixture map: `examples/<path>` → string contents or Blob. */
  examples?: Record<string, string | Blob>;
  /** Allowed public CORS-enabled HTTPS host prefixes (e.g., `'https://my-data.example.com/'`). */
  publicBuckets?: string[];
  /** When `true`, the resolver accepts `@uploads/<key>` references that the host
   *  has previously seeded via {@link DefaultResolverHandle.addUpload}. */
  allowLocalFiles?: boolean;
}

/**
 * Resolver instance + side-channel for the host to register browser-file
 * uploads (the only Tier-1 path that needs imperative state). The host wires
 * the file input UI; the component sees only the {@link ConnectionResolver}
 * surface (resolve + list).
 */
export interface DefaultResolverHandle extends ConnectionResolver {
  /** Register a browser-uploaded File/Blob under a logical name. Throws if
   *  `allowLocalFiles` was false. */
  addUpload(path: string, blob: Blob): void;
}

/**
 * Construct the default Tier-1 {@link ConnectionResolver} — the browser-local
 * tier, which needs no credentials and no host backend.
 *
 * The returned resolver implements all three Tier-1 paths:
 *   - Tier 1.a (bundled examples) — `@examples/<path>` → blob URL from `opts.examples`
 *   - Tier 1.b (local uploads)    — `@uploads/<path>` → blob URL from `addUpload`-seeded Map
 *   - Tier 1.c (public CORS HTTP) — `@public/<url>`   → URL passthrough if on allowlist
 *
 * The resolver retains NO credential-shape state. Even when the host passes a
 * presigned URL (e.g., S3 `?X-Amz-Credential=AKIA...`), the URL is returned in
 * the ephemeral `ResolvedSource` only; no `ResolvedSource` is cached. This
 * property is asserted structurally by the CONN-01 invariant test in
 * `tests/no-credentials-leak.test.ts`.
 */
export function createDefaultResolver(
  opts: DefaultResolverOpts = {},
): DefaultResolverHandle {
  // Module-private upload store. Keys are logical paths; values are Blobs
  // (binary data, never strings). The Blob's contents never enter SQL —
  // the io.* constructor in the .fossil mapping reads the blob: URL.
  const uploads = new Map<string, Blob>();

  function inferFormat(path: string): ResolvedSource['format'] {
    if (path.endsWith('.csv')) return 'csv';
    if (path.endsWith('.json')) return 'json';
    if (path.endsWith('.parquet')) return 'parquet';
    return undefined;
  }

  return {
    async resolve(ref: SourceRef): Promise<ResolvedSource> {
      // Tier 1.a — bundled example
      if (ref.connector === 'examples' && opts.examples?.[ref.path] !== undefined) {
        const data = opts.examples[ref.path]!;
        const blob = typeof data === 'string' ? new Blob([data]) : data;
        return { url: URL.createObjectURL(blob), format: inferFormat(ref.path) };
      }
      // Tier 1.b — local upload
      if (ref.connector === 'uploads' && opts.allowLocalFiles) {
        const blob = uploads.get(ref.path);
        if (!blob) throw new Error(`No upload registered for path: ${ref.path}`);
        return { url: URL.createObjectURL(blob), format: inferFormat(ref.path) };
      }
      // Tier 1.c — public CORS allowlist
      if (ref.connector === 'public' && opts.publicBuckets) {
        const url = ref.path; // expected form: full URL after `@public/`
        const allowed = opts.publicBuckets.some((prefix) => url.startsWith(prefix));
        if (!allowed) {
          throw new Error(
            `URL not in publicBuckets allowlist: ${url}. Add the host prefix to DefaultResolverOpts.publicBuckets to allow it.`,
          );
        }
        return { url, format: inferFormat(url) };
      }
      throw new Error(
        `Cannot resolve @${ref.connector}/${ref.path} — no matching Tier-1 connector configured.`,
      );
    },

    async list(): Promise<Connector[]> {
      const out: Connector[] = [];
      if (opts.examples) {
        out.push({ name: 'examples', type: 'examples', label: 'Bundled examples' });
      }
      if (opts.allowLocalFiles) {
        out.push({ name: 'uploads', type: 'upload', label: 'Local file uploads' });
      }
      if (opts.publicBuckets?.length) {
        out.push({ name: 'public', type: 'public_http', label: 'Public CORS HTTPS' });
      }
      return out;
    },

    addUpload(path: string, blob: Blob): void {
      if (!opts.allowLocalFiles) {
        throw new Error('addUpload called but allowLocalFiles is false');
      }
      uploads.set(path, blob);
    },
  };
}

/**
 * Parse `@connector/path` text → {@link SourceRef}. Throws on:
 *   - missing leading `@`
 *   - missing `/`
 *   - connector name failing {@link CONNECTOR_NAME_REGEX}
 *
 * An invalid connector name must produce a clear error rather
 * than silently coercing — downstream `@`-autocomplete in the editor relies
 * on the parse contract to surface did-you-mean suggestions.
 */
export function parseSourceRef(raw: string): SourceRef {
  if (!raw.startsWith('@')) throw new Error(`SourceRef must start with @: ${raw}`);
  const rest = raw.slice(1);
  const slash = rest.indexOf('/');
  if (slash < 0) throw new Error(`SourceRef must contain /: ${raw}`);
  const connector = rest.slice(0, slash);
  const path = rest.slice(slash + 1);
  if (!CONNECTOR_NAME_REGEX.test(connector)) {
    throw new Error(
      `Invalid connector name: ${JSON.stringify(connector)} — must match ${CONNECTOR_NAME_REGEX}`,
    );
  }
  return { raw, connector, path };
}
