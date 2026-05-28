'use client';

/**
 * KeasyShell — Keasy-styled host chrome wrapping `<FossilEditor/>` +
 * `<FossilViewer/>`.
 *
 * Per 17-CONTEXT.md (REL-03 multi-host fixture, locked decisions):
 *   "A NEW page in `apps/landing` ... that renders <FossilEditor/> +
 *    <FossilViewer/> wrapped in a host shell that mimics Keasy's layout/
 *    spacing/typography (shadcn token overrides + Keasy-style chrome —
 *    sidebar, header, content area)."
 *   "It does NOT contain Keasy source code — only Keasy's *visual look*."
 *
 * Layout shape mimics the Phase 16 Zed-style discovery workspace (Keasy
 * commit `5fcdccf`): left sidebar (logo + nav stack) + top header (title +
 * primary action) + main content area (editor pane | viewer pane). All
 * tokens read through the shadcn vocabulary that `<KanzoThemeProvider/>`
 * (from `@kanzo/theme`) cascades into descendants — same vocabulary
 * Keasy's own `globals.css` consumes, so the visual rhyme is structural
 * (no copied CSS).
 */

import type { CSSProperties, ReactNode } from 'react';
import styles from './keasy-shell.module.css';

export interface KeasyShellProps {
  /** Theme discriminator for the `data-theme` attribute (the
   *  Cosmos.gl + CodeMirror inner components key off this). */
  theme: 'light' | 'dark';
  /** Primary action on the header. Wired to the page's compile-and-run
   *  pipeline by the page-level container. */
  onRun: () => void;
  /** Disables the Run button while a Run is in flight (e.g. WASM still
   *  loading, or a prior Run hasn't yet returned). */
  running?: boolean;
  /** Optional Run-error message — surfaces as a Keasy-styled destructive
   *  banner above the editor pane. */
  errorMessage?: string | null;
  /** The editor pane content (rendered in the LEFT content cell). */
  editorPane: ReactNode;
  /** The viewer pane content (rendered in the RIGHT content cell). */
  viewerPane: ReactNode;
  /** Optional title rendered in the header. Defaults to "Hello mapping". */
  title?: string;
  /** Inline style applied at the shell root — used by the page-level
   *  container to inject the resolved `--fossil-*` CSS vars
   *  (`cssVarsToStyle(useTheme(theme).cssVars)`) so the keasy-shell
   *  CSS module's `var(--fossil-colors-*)` lookups resolve to the
   *  correct light/dark palette. */
  rootStyle?: CSSProperties;
}

/**
 * Render the chrome. Stateless — the page-level container owns Run state.
 */
export function KeasyShell({
  theme,
  onRun,
  running = false,
  errorMessage = null,
  editorPane,
  viewerPane,
  title = 'Hello mapping',
  rootStyle,
}: KeasyShellProps): JSX.Element {
  return (
    <div
      data-testid="multi-host-shell"
      data-theme={theme}
      className={styles.shell}
      style={rootStyle}
    >
      <aside className={styles.sidebar} aria-label="Workspace navigation">
        <div className={styles.sidebarLogo} aria-label="Embedded host">
          Fossil × Host
        </div>
        <div>
          <div className={styles.sidebarSectionLabel}>Workspace</div>
          <nav className={styles.sidebarNav} aria-label="Primary navigation">
            <a className={styles.sidebarNavItem} aria-current="page" href="#">
              Mappings
            </a>
            <a className={styles.sidebarNavItem} href="#">
              Sources
            </a>
            <a className={styles.sidebarNavItem} href="#">
              Discoveries
            </a>
            <a className={styles.sidebarNavItem} href="#">
              Jobs
            </a>
            <a className={styles.sidebarNavItem} href="#">
              Settings
            </a>
          </nav>
        </div>
      </aside>

      <header className={styles.header}>
        <div className={styles.headerTitle}>{title}</div>
        <button
          className={styles.headerRunButton}
          onClick={onRun}
          disabled={running}
          type="button"
        >
          {running ? 'Running…' : 'Run mapping'}
        </button>
      </header>

      <main className={styles.contentArea}>
        <div className={styles.pane} data-testid="multi-host-editor-pane">
          <div className={styles.paneHeader}>Mapping</div>
          {errorMessage ? (
            <div role="alert" className={styles.errorBanner}>
              {errorMessage}
            </div>
          ) : null}
          <div className={styles.paneBody}>
            <div className={styles.editorMount}>{editorPane}</div>
          </div>
        </div>
        <div className={styles.pane} data-testid="multi-host-viewer-pane">
          <div className={styles.paneHeader}>Output</div>
          <div className={styles.paneBody}>{viewerPane}</div>
        </div>
      </main>
    </div>
  );
}
