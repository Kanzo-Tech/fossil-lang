import type {
  ConnectionResolver,
  Connector,
  ResolvedSource,
  SourceRef,
} from '@fossil-lang/types';

/**
 * Options for {@link createMockResolver}. Used in tests, Storybook, and
 * the Keasy migration smoke test where a deterministic canned response
 * per ref is needed.
 */
export interface MockResolverOpts {
  /** Map keyed on `${connector}/${path}` returning the canned ResolvedSource. */
  fixtures: Record<string, ResolvedSource>;
  /** Connectors to advertise via list(). If omitted, derives one connector per
   *  unique `connector` prefix in the fixtures map. */
  connectors?: Connector[];
}

/**
 * Deterministic mock resolver — returns canned {@link ResolvedSource} values
 * per `@connector/path` key. Suitable for tests, fixtures, and Storybook.
 */
export function createMockResolver(opts: MockResolverOpts): ConnectionResolver {
  return {
    async resolve(ref: SourceRef): Promise<ResolvedSource> {
      const key = `${ref.connector}/${ref.path}`;
      const fixture = opts.fixtures[key];
      if (!fixture) throw new Error(`MockResolver: no fixture for ${key}`);
      return fixture;
    },
    async list(): Promise<Connector[]> {
      if (opts.connectors) return opts.connectors;
      const seen = new Set<string>();
      const out: Connector[] = [];
      for (const key of Object.keys(opts.fixtures)) {
        const slash = key.indexOf('/');
        const name = slash >= 0 ? key.slice(0, slash) : key;
        if (!seen.has(name)) {
          seen.add(name);
          out.push({ name, type: 'examples' as const });
        }
      }
      return out;
    },
  };
}
