'use client';

/**
 * PLAY-05 Examples dropdown — enumerates `EXAMPLES_MANIFEST` from
 * `@fossil-lang/examples` and emits the selected example id to the host.
 *
 * Pure UI — does NOT fetch example contents itself; the host
 * (`PlaygroundHost.tsx`) listens to `onChange` and re-seeds the playground
 * with the chosen example's mapping + descriptors. Keeping I/O out of the
 * selector means the component is trivially unit-testable AND the host
 * controls the URL-fragment / permalink interaction (which is host
 * concern, not playground-library concern per the locked decision in
 * 09-CONTEXT.md).
 *
 * The component is intentionally a plain `<select>` rather than a Radix
 * primitive: the landing app's bundle budget is tight (500 KB gzip), and
 * the dropdown is a leaf UI surface that doesn't need a portal-mounted
 * popover. Native `<select>` is accessible-by-default + zero JS payload.
 */
import { EXAMPLES_MANIFEST } from '@fossil-lang/examples';

export interface ExampleSelectorProps {
  /**
   * Currently selected example id. Empty string `''` means "Custom" (the
   * user's hand-edited source, no example loaded).
   */
  value: string;
  /**
   * Fires with the newly-selected example id (or `''` when the user picks
   * "Custom"). Host is responsible for fetching contents + seeding state.
   */
  onChange: (id: string) => void;
}

export function ExampleSelector(props: ExampleSelectorProps): JSX.Element {
  const { value, onChange } = props;
  return (
    <label
      className="fossil-example-selector"
      data-testid="example-selector"
      style={{
        display: 'inline-flex',
        alignItems: 'center',
        gap: '0.5rem',
        marginBottom: '0.75rem',
        fontSize: '0.875rem',
      }}
    >
      <span>Example:</span>
      <select
        value={value}
        onChange={(e) => onChange(e.target.value)}
        aria-label="Load curated example"
        data-testid="example-selector-dropdown"
        style={{
          padding: '0.25rem 0.5rem',
          fontSize: '0.875rem',
        }}
      >
        <option value="" data-testid="example-option-custom">
          — Custom —
        </option>
        {EXAMPLES_MANIFEST.examples.map((ex) => (
          <option
            key={ex.id}
            value={ex.id}
            data-testid={`example-option-${ex.id}`}
          >
            {ex.label}
          </option>
        ))}
      </select>
    </label>
  );
}
