/**
 * DuckDB SQL dialect for @codemirror/lang-sql.
 *
 * Foundation for PLAY-07 ("View Compiled SQL" panel). We layer DuckDB-specific
 * keywords + builtin function names over CodeMirror's `StandardSQL` dialect via
 * `SQLDialect.define` — the lexer recognises DuckDB extensions (PIVOT/UNPIVOT,
 * SUMMARIZE, QUALIFY, list/struct/map constructors, asof joins, lateral joins,
 * SEMI/ANTI joins) + DuckDB-only builtins (list_agg / regexp_extract /
 * date_trunc family / read_csv_auto family) so syntax highlighting reflects
 * what the compiler emits via `crates/fossil-codegen`.
 *
 * Per 09-CONTEXT.md (PLAY-07 locked decision): "CodeMirror 6 read-only with
 * SQL language extension `@codemirror/lang-sql`" + DuckDB dialect.
 * Per 09-RESEARCH.md Pattern 3.
 */

import { SQLDialect } from '@codemirror/lang-sql';

const duckdbKeywords =
  'pivot unpivot summarize qualify exclude replace ' +
  'list_value struct_pack map_from_entries asof ' +
  'positional_join lateral semi anti ';

const duckdbBuiltins =
  'list_aggr array_agg list_distinct unnest string_split regexp_extract ' +
  'epoch_ms date_trunc strftime strptime read_csv_auto read_parquet ' +
  'list_transform list_filter ';

/**
 * DuckDB SQL dialect — extends `StandardSQL` with DuckDB-specific keywords
 * + builtin function names. Pass to `sql({ dialect: DuckDB })`.
 */
export const DuckDB: SQLDialect = SQLDialect.define({
  keywords: duckdbKeywords,
  builtin: duckdbBuiltins,
});
