/**
 * The verbs — `fossil-graph`'s SQL, dispatched through `fossil-graph-wasm` — over the views this
 * registers for them: `schema`, `relations` and the escape hatch.
 */

import type { CorpusAddressing } from './address.js';
import { createGraphClient, type GraphClient } from './client.js';
import type { CorpusField, CorpusRelation } from './corpus.js';
import type {
  ExecuteSqlParams,
  ExecuteSqlResult,
  SchemaParams,
  SchemaResult,
} from './generated.js';
import type { QueryFn } from './query.js';
import { distinct, ident, list, lit } from './sql.js';

export interface Verbs {
  schema(params?: SchemaParams): Promise<SchemaResult>;
  relations(): Promise<readonly CorpusRelation[]>;
  executeSql(params: ExecuteSqlParams): Promise<ExecuteSqlResult>;
}

export function verbsOf(reads: {
  readonly query: QueryFn;
  readonly addressing: CorpusAddressing;
  readonly manifestFiles: Record<string, string>;
  readonly catalog: string;
  readonly payloadFiles: ReadonlyMap<string, readonly string[]>;
  readonly fieldsOf: (type: string) => readonly CorpusField[];
}): Verbs {
  const { query, addressing, manifestFiles, catalog, payloadFiles, fieldsOf } = reads;
  const relationOf = (name: string): string => `${ident(catalog)}.${ident(name)}`;

  /**
   * The verb half, booted on first use and not before.
   *
   * **The verbs name tables and this corpus is files, so something has to bridge that.** Every
   * statement `fossil-graph` composes reads `FROM "Person"` or `FROM "Person_knows_Person"` — it
   * is single-source with the Rust precisely because it does not know where the bytes are. So the
   * door registers the views the verbs expect, over the paths the manifest already gave it.
   *
   * **`TEMP`, and that is not a detail.** A plain `CREATE OR REPLACE VIEW "Person"` would
   * overwrite a host's own table of that name — `Person` is not an unlikely name for one, and
   * `tests/e2e.test.ts` creates exactly it. A temp view is resolved before `main` and dropped with
   * the connection, so opening a corpus shadows the host's catalog for as long as the corpus is
   * open and destroys nothing in it.
   *
   * **The edge orientation is a glob and it is the only one in the reader.** A vertex type's tiles
   * are enumerated from `vertex_count` and `chunk_size`, which is what the declared count is for;
   * an adjacency's are not, because a tile whose vertices have no edges is a file that was never
   * written and a run of them is a gap no arithmetic predicts. `read_parquet` over an enumerated
   * list containing one absent file is an error, not an empty relation. The rule `open.ts` states —
   * never glob — is about the vertex payload, where a glob picks up the staged
   * single-file copy beside the tiles and counts every row twice; `<adjacency>/chunk*.parquet` has
   * no such sibling inside it.
   *
   * **`chunk*` and not `tile*`**: the adjacency is the projection at `scale: 1` and spells its
   * files like every other projection. `tile{k}` is the identity index's alone, being the one
   * artefact that is not a projection.
   */
  let transport: Promise<GraphClient> | null = null;

  const verbs = (): Promise<GraphClient> => {
    transport ??= (async (): Promise<GraphClient> => {
      await query(`ATTACH IF NOT EXISTS ':memory:' AS ${ident(catalog)}`);
      for (const type of addressing.types) {
        await query(
          `CREATE OR REPLACE VIEW ${relationOf(type.type)} AS ` +
            `SELECT * FROM read_parquet(${list(distinct(payloadFiles.get(type.type)!))})`,
        );
      }
      for (const edge of addressing.edges) {
        // The source-ordered orientation, because that is the one a verb reads the whole relation
        // from. An edge type publishing none gets no view rather than a path that 404s — and the
        // verb that reaches for it fails by name, which is the diagnosis.
        const adjacency = edge.adjacency('src');
        if (adjacency === null) continue;
        const source =
          adjacency.container === 'rowgroups'
            ? lit(adjacency.tileUrl(0))
            : lit(`${adjacency.prefix}chunk*.parquet`);
        await query(
          `CREATE OR REPLACE VIEW ` +
            `${relationOf(`${edge.srcType}_${edge.edgeType}_${edge.dstType}`)} AS ` +
            `SELECT * FROM read_parquet(${source})`,
        );
      }
      return createGraphClient({ query, manifestFiles, catalog });
    })();
    return transport;
  };

  return {
    async schema(params = {}) {
      return (await verbs()).schema(params);
    },
    async relations() {
      const answered = await (await verbs()).schema({});
      const vertices = answered.vertices.map(
        (v): CorpusRelation => ({
          kind: 'vertex',
          name: v.name,
          sql: relationOf(v.name),
          rows: v.count,
          files: payloadFiles.get(v.name) ?? [],
          columns: fieldsOf(v.name),
        }),
      );
      const edges = answered.edges.flatMap((e): CorpusRelation[] => {
        const address = addressing.edges.find(
          (a) => a.edgeType === e.name && a.srcType === e.source_type && a.dstType === e.target_type,
        );
        if (!address || address.adjacency('src') === null) return [];
        return [
          {
            kind: 'edge',
            name: e.table_name,
            sql: relationOf(e.table_name),
            rows: e.count,
            files: address.adjacencyFiles('src'),
            edgeType: e.name,
            srcType: e.source_type,
            dstType: e.target_type,
          },
        ];
      });
      return [...vertices, ...edges];
    },
    async executeSql(params) {
      return (await verbs()).executeSql(params);
    },
  };
}
