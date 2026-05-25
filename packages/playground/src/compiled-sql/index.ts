/**
 * Barrel for the Compiled-SQL panel module.
 *
 * Public API (re-exported from `@fossil-lang/playground`):
 *   - CompiledSqlPanel — React component rendering DuckDB SQL read-only.
 *   - CompiledSqlPanelProps — its prop shape.
 *   - DuckDB — the SQLDialect for callers that want to compose their own
 *     CodeMirror extensions outside this panel.
 *
 * Foundation for PLAY-07.
 */

export {
  CompiledSqlPanel,
  type CompiledSqlPanelProps,
} from './CompiledSqlPanel.js';
export { DuckDB } from './duckdb-dialect.js';
