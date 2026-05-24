import type {
  ConnectionResolver,
  Connector,
  ResolvedSource,
  SourceRef,
} from '@fossil-lang/types';

/**
 * Strict-passthrough resolver that ONLY accepts public CORS-enabled HTTPS URLs
 * matching one of the supplied host prefixes — no examples, no uploads. For
 * hosts that want a pure allowlist with zero side state.
 *
 * Format inference is by extension (`.csv`, `.json`, `.parquet`). Anything
 * else returns `format: undefined` and the io/* constructor in the .fossil
 * mapping decides how to interpret the URL.
 */
export function createPublicHttpResolver(
  allowedHostPrefixes: string[],
): ConnectionResolver {
  return {
    async resolve(ref: SourceRef): Promise<ResolvedSource> {
      if (ref.connector !== 'public') {
        throw new Error(
          `createPublicHttpResolver only handles @public/<url>; got @${ref.connector}/...`,
        );
      }
      const url = ref.path;
      const allowed = allowedHostPrefixes.some((prefix) => url.startsWith(prefix));
      if (!allowed) throw new Error(`URL not allowlisted: ${url}`);
      const format = url.endsWith('.csv')
        ? 'csv'
        : url.endsWith('.json')
          ? 'json'
          : url.endsWith('.parquet')
            ? 'parquet'
            : undefined;
      return { url, format };
    },
    async list(): Promise<Connector[]> {
      return [{ name: 'public', type: 'public_http', label: 'Public HTTPS allowlist' }];
    },
  };
}
