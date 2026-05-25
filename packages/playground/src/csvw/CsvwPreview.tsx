/**
 * CsvwPreview — editable CSVW JSON-LD descriptor panel.
 *
 * PLAY-09 + PLAY-11 surface. The parent owns:
 *   - `inferred`: the auto-inferred table (set after `inferCsvw()` completes);
 *   - `value`: the current CSVW JSON string (controlled by the parent so it
 *     can round-trip through the permalink + flow into `compileFile`);
 *   - `dirty`: TRUE once the user has edited; the parent uses this to
 *     suppress background re-inference (RESEARCH.md Pitfall 4 — never
 *     clobber a user edit with a background inference);
 *   - `onChange`: called with the new JSON string on every edit;
 *   - `onReset`: called when the user clicks "Reset to inferred"; the parent
 *     restores `value` from `inferred` and flips `dirty` back to false.
 *
 * Layout: a structured form rather than a raw JSON textarea — the locked
 * CONTEXT.md decision was "Plain `useState` controlled inputs OK for ~10-field
 * forms" + "editable panel — user can refine column types, drop columns".
 *
 * One row per column:
 *   - text input for the column name;
 *   - <select> dropdown for the datatype (the same XSD short names that
 *     `duckdbTypeToCsvw` emits);
 *   - drop button to remove the column from the descriptor.
 *
 * A11Y:
 *   - `role="region"` + `aria-label="CSVW descriptor"` so the landmark shows
 *     up in axe-core + screen-reader region lists.
 *   - Each interactive control carries a stable `aria-label` naming the
 *     column ordinal it belongs to (so "Column 0 datatype" is announced).
 *   - The Reset button is only rendered when `dirty && inferred` — a no-op
 *     button would otherwise be a WCAG focus-order distraction.
 *
 * Per 09-07-PLAN Task 1 + 09-CONTEXT.md "CSVW inference + preview".
 */

import type { CsvwTable, CsvwColumn } from './infer.js';
import { applyCsvw, parseCsvw } from './apply.js';

/**
 * Supported CSVW datatype short-names. Mirrors the codomain of
 * `duckdbTypeToCsvw` so the dropdown can always represent the auto-inferred
 * value. Order is alphabetical to keep the dropdown predictable.
 */
const SUPPORTED_DATATYPES: readonly string[] = [
  'boolean',
  'date',
  'dateTime',
  'dateTimeStamp',
  'decimal',
  'double',
  'duration',
  'float',
  'hexBinary',
  'integer',
  'nonNegativeInteger',
  'string',
  'time',
];

export interface CsvwPreviewProps {
  /**
   * The auto-inferred descriptor (set by the parent after `inferCsvw()`
   * resolves). `null` until inference completes. The component itself never
   * runs inference — it only renders + edits.
   */
  inferred: CsvwTable | null;

  /**
   * Current CSVW JSON string. Controlled. The component parses this on every
   * render via `parseCsvw`; if parsing fails (typo in the persisted JSON) the
   * empty-state placeholder renders.
   */
  value: string;

  /** Called with the new JSON string whenever the user edits a field. */
  onChange: (next: string) => void;

  /**
   * TRUE once the user has edited the inferred descriptor. The parent
   * suppresses re-inference while this is TRUE so a background inference
   * never clobbers a manual edit (RESEARCH.md Pitfall 4).
   */
  dirty: boolean;

  /**
   * Called when the user clicks the "Reset to inferred" button. The parent
   * restores `value` from `inferred` and flips `dirty` back to false.
   */
  onReset: () => void;

  /** Theme discriminator — drives `data-theme` for CSS-vars cascades. */
  theme?: 'light' | 'dark';
}

/** Editable CSVW descriptor panel. See module doc-comment. */
export function CsvwPreview({
  inferred,
  value,
  onChange,
  dirty,
  onReset,
  theme = 'light',
}: CsvwPreviewProps): JSX.Element {
  const parsed = parseCsvw(value);

  const updateColumn = (idx: number, patch: Partial<CsvwColumn>): void => {
    if (!parsed) return;
    const next: CsvwTable = {
      ...parsed,
      tableSchema: {
        ...parsed.tableSchema,
        columns: parsed.tableSchema.columns.map((c, i) =>
          i === idx ? { ...c, ...patch } : c,
        ),
      },
    };
    onChange(applyCsvw(next));
  };

  const dropColumn = (idx: number): void => {
    if (!parsed) return;
    const next: CsvwTable = {
      ...parsed,
      tableSchema: {
        ...parsed.tableSchema,
        columns: parsed.tableSchema.columns.filter((_, i) => i !== idx),
      },
    };
    onChange(applyCsvw(next));
  };

  return (
    <div
      role="region"
      aria-label="CSVW descriptor"
      data-testid="csvw-preview"
      data-theme={theme}
      className="fossil-csvw-preview"
    >
      <header className="fossil-csvw-preview__header">
        <span>
          CSVW descriptor {dirty ? '(edited)' : '(auto-inferred)'}
        </span>
        {dirty && inferred ? (
          <button
            type="button"
            onClick={onReset}
            data-testid="csvw-reset"
            aria-label="Reset to inferred CSVW"
          >
            Reset to inferred
          </button>
        ) : null}
      </header>
      {!parsed ? (
        <p role="status" data-testid="csvw-empty">
          No CSVW descriptor. Paste a CSV source to auto-infer.
        </p>
      ) : (
        <table className="fossil-csvw-preview__table">
          <thead>
            <tr>
              <th scope="col">Column name</th>
              <th scope="col">Datatype</th>
              <th scope="col" aria-label="Actions" />
            </tr>
          </thead>
          <tbody>
            {parsed.tableSchema.columns.map((col, idx) => (
              <tr key={idx} data-testid={`csvw-row-${idx}`}>
                <td>
                  <input
                    type="text"
                    value={col.name}
                    onChange={(e) =>
                      updateColumn(idx, { name: e.target.value })
                    }
                    aria-label={`Column ${idx} name`}
                    data-testid={`csvw-col-name-${idx}`}
                  />
                </td>
                <td>
                  <select
                    value={col.datatype}
                    onChange={(e) =>
                      updateColumn(idx, { datatype: e.target.value })
                    }
                    aria-label={`Column ${idx} datatype`}
                    data-testid={`csvw-col-type-${idx}`}
                  >
                    {SUPPORTED_DATATYPES.includes(col.datatype) ? null : (
                      // The persisted descriptor may carry a datatype outside
                      // the canonical set (legacy CSVW, a user-typed value).
                      // Surface it as an extra option so the <select>'s value
                      // attribute matches a real <option> — without this,
                      // React warns "Use the `defaultValue` or `value` props
                      // on <select> instead of setting `selected` on
                      // <option>" + the dropdown silently snaps to the first
                      // option, losing the existing value.
                      <option value={col.datatype}>{col.datatype}</option>
                    )}
                    {SUPPORTED_DATATYPES.map((t) => (
                      <option key={t} value={t}>
                        {t}
                      </option>
                    ))}
                  </select>
                </td>
                <td>
                  <button
                    type="button"
                    onClick={() => dropColumn(idx)}
                    aria-label={`Drop column ${col.name}`}
                    data-testid={`csvw-col-drop-${idx}`}
                  >
                    ×
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
