import { readFileSync } from "node:fs";
import { join } from "node:path";
import { repoRoot } from "@/lib/repo";

interface Table {
  formula: string;
  vectors: Array<Record<string, string | number>>;
  [key: string]: unknown;
}

/**
 * The published vectors, rendered from the file the checker reads.
 *
 * `guards/vectors.json` is the deliverable — the table a second implementation is checked against,
 * and the thing that gets copied. Transcribing it into prose would create a second copy that agrees
 * on the day it is written, which is exactly the failure the vectors exist to prevent: Iceberg
 * publishes its bucket transform with a table and every port agrees; PMTiles links Wikipedia for its
 * Hilbert curve and every port differs.
 *
 * So the page renders the file, and `published-vectors` executes the same file.
 */
export function VectorTable({
  of,
}: {
  of: "tile_of" | "tile_url" | "declared_count" | "level_plan" | "morton2" | "quantize";
}) {
  // Off the repo root, not off `process.cwd()`: the file lives with the checker that executes it,
  // and this page renders it from there rather than keeping a copy on this side of the tree.
  const path = join(repoRoot, "apps", "corpus", "guards", "vectors.json");
  const table = JSON.parse(readFileSync(path, "utf8"))[of] as Table;
  const columns = Object.keys(table.vectors[0]).filter((key) => key !== "why");

  return (
    <div className="my-6 overflow-x-auto">
      <p className="mb-2 text-sm text-fd-muted-foreground">
        <code>{table.formula}</code>
      </p>
      <table>
        <thead>
          <tr>
            {columns.map((column) => (
              <th key={column}>
                <code>{column}</code>
              </th>
            ))}
            <th>why this one is a border</th>
          </tr>
        </thead>
        <tbody>
          {table.vectors.map((vector) => (
            <tr key={columns.map((c) => vector[c]).join("/")}>
              {columns.map((column) => (
                <td key={column}>
                  <code>{String(vector[column])}</code>
                </td>
              ))}
              <td>{String(vector.why)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
