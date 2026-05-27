/**
 * Toolbar (Phase 14 plan 14-04) — extracted from FossilPlayground's inline
 * <header>. Composes Run / Reset / Cite (BibtexModal) using `@fossil-lang/ui`
 * Tooltip primitives.
 *
 * Theme is NOT in this toolbar — the playground component receives `theme`
 * as a prop from the host (a `<KanzoThemeProvider/>` wrap or explicit prop)
 * per ADR-0035. Hosts compose their own theme switcher above the playground.
 */
import {
  Tooltip,
  TooltipTrigger,
  TooltipContent,
  TooltipProvider,
} from '@fossil-lang/ui';
import { ARIA_LABELS } from '../a11y/index.js';
import { BibtexModal } from '../bibtex/BibtexModal.js';

export interface ToolbarProps {
  onRun: () => void;
  onReset: () => void;
  running: boolean;
  permalink: string | undefined;
}

export function Toolbar(props: ToolbarProps): JSX.Element {
  const { onRun, onReset, running, permalink } = props;
  return (
    <TooltipProvider delayDuration={300}>
      <header
        className="fossil-playground__toolbar"
        role="banner"
        data-testid="playground-toolbar"
        style={{
          display: 'flex',
          gap: '0.5rem',
          padding: '0.5rem',
          alignItems: 'center',
        }}
      >
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type="button"
              onClick={onRun}
              disabled={running}
              aria-label={ARIA_LABELS.runButton}
              aria-busy={running || undefined}
              data-testid="toolbar-run"
            >
              {running ? 'Running…' : 'Run'}
            </button>
          </TooltipTrigger>
          <TooltipContent side="bottom">
            Compile + execute the mapping
          </TooltipContent>
        </Tooltip>
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type="button"
              onClick={onReset}
              aria-label={ARIA_LABELS.resetButton}
              data-testid="toolbar-reset"
            >
              Reset playground
            </button>
          </TooltipTrigger>
          <TooltipContent side="bottom">
            Reset playground state (DuckDB + compiler)
          </TooltipContent>
        </Tooltip>
        <BibtexModal permalink={permalink} />
      </header>
    </TooltipProvider>
  );
}
