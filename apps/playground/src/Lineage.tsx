/**
 * The lineage panel: what this program references, and what running it would need.
 *
 * Self-contained on purpose — one import and one element in `App.tsx` is the whole of its
 * wiring, and it carries its own stylesheet rather than extending `styles.css`, because the
 * editor half of this app is being replaced underneath it.
 *
 * The design job here is to make a NEGATIVE observable: that this answer cost no engine, no
 * credential and no run. A negative cannot be shown by drawing it, so the panel shows the
 * three things that stand in for it — the elapsed time of the call, the keystroke counter
 * proving it re-answers undebounced, and the byte ledger of what it declined to load.
 */
import { useMemo } from 'react';

import {
  engineDeferred,
  lineageOf,
  providersFor,
  providerTable,
  surfaceStats,
  type ProviderInfo,
} from './refs.js';
import './lineage.css';

const KB = (bytes: number) => (bytes < 1024 * 1024 ? `${(bytes / 1024).toFixed(0)} kB` : `${(bytes / 1024 / 1024).toFixed(1)} MB`);

export interface LineageProps {
  /** The program as written. Not the absolutised one — lineage reports what the AUTHOR wrote. */
  program: string;
  /** False until `initFossilWasm` has resolved; `refs` is not callable before that. */
  ready: boolean;
  /** Measured bundle sizes, for the ledger. `null` means "never loaded", which is the point. */
  checkerBytes: number | null;
  duckdbBytes: number | null;
  executorBytes: number | null;
}

export default function Lineage({ program, ready, checkerBytes, duckdbBytes, executorBytes }: LineageProps) {
  // No debounce, deliberately. See `lineage.ts`: `refs` builds a throwaway db per call and
  // touches no persistent workspace, so unlike `check` it cannot be re-entered into a poisoned
  // state. Recomputing in a memo means it re-answers on the render of every keystroke.
  const lineage = useMemo(() => (ready ? lineageOf(program) : null), [program, ready]);

  // How many times the surface has actually been asked, counted at the call site rather than
  // here — see `surfaceStats`. The number climbing per character with the timing beside it
  // staying flat is the parse-only claim as an observation.
  const stats = surfaceStats();

  const table = ready ? providerTable() : [];
  const ledger = engineDeferred(checkerBytes, duckdbBytes, executorBytes);

  if (!lineage) {
    return (
      <div className="lin">
        <h2>references — parse-only</h2>
        <p className="lin-note">…waiting for the checker bundle</p>
      </div>
    );
  }

  const aliased = lineage.needs.filter((n) => n.alias !== null);
  const direct = lineage.needs.find((n) => n.alias === null);

  return (
    <div className="lin">
      <h2>references — parse-only</h2>

      <p className="lin-claim">
        Answered by parsing the text above. No engine started, no credential read, no source
        opened — <strong>{lineage.refs.length}</strong> reference
        {lineage.refs.length === 1 ? '' : 's'} in{' '}
        <strong>{lineage.elapsedMs.toFixed(2)} ms</strong>.
      </p>

      {lineage.error && <p className="lin-err">the parser refused this text: {lineage.error}</p>}

      {lineage.refs.length === 0 && !lineage.error ? (
        <p className="lin-note">this program references nothing external</p>
      ) : (
        <table className="lin-table">
          <thead>
            <tr>
              <th>connection</th>
              <th>path</th>
              <th>role</th>
              <th>read by</th>
            </tr>
          </thead>
          <tbody>
            {lineage.refs.map((row, i) => {
              const can = providersFor(row.path, table);
              return (
                <tr key={`${row.connection}/${row.path}/${row.role}/${i}`}>
                  <td>
                    {row.connection === null ? (
                      <span className="lin-dim">direct</span>
                    ) : (
                      <span className="lin-conn">@{row.connection}</span>
                    )}
                  </td>
                  <td className="lin-path">{row.path}</td>
                  <td>
                    <span className={`lin-role lin-role-${row.role}`}>{row.role}</span>
                  </td>
                  <td className="lin-dim">{can.map((p) => `io.${p.name}`).join(' ') || '—'}</td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}

      <h2>what a run would need</h2>
      {aliased.length === 0 ? (
        <p className="lin-note">
          no <code>@conn</code> aliases — every path is direct, so a host would ask for no
          credentials{direct ? ` to open these ${direct.refs.length}` : ''}. Write{' '}
          <code>io.csv("@warehouse/users.csv")</code> and the alias appears here, before the run.
        </p>
      ) : (
        <ul className="lin-needs">
          {aliased.map((need) => (
            <li key={need.alias}>
              <span className="lin-conn">@{need.alias}</span>
              <span className="lin-dim">
                {' '}
                — credentials for {need.refs.length} reference{need.refs.length === 1 ? '' : 's'}:{' '}
                {need.refs.map((r) => r.path).join(', ')}
              </span>
            </li>
          ))}
        </ul>
      )}

      <h2>what it cost to know</h2>
      <dl className="lin-ledger">
        <div>
          <dt>asked</dt>
          <dd>
            {stats.calls} time{stats.calls === 1 ? '' : 's'}, undebounced
          </dd>
        </div>
        <div>
          <dt>slowest</dt>
          <dd>{stats.worstMs.toFixed(2)} ms</dd>
        </div>
        <div>
          <dt>spent</dt>
          <dd>{ledger.spentBytes === null ? '—' : `${KB(ledger.spentBytes)} (the checker, already resident)`}</dd>
        </div>
        <div>
          <dt>not loaded</dt>
          <dd>
            {ledger.parts.map((p) => `${p.name} ${p.bytes === null ? 'unfetched' : KB(p.bytes)}`).join(', ')}
          </dd>
        </div>
      </dl>
      <p className="lin-note">
        The checker is debounced at 120 ms and guarded, because <code>updateFile</code> mutates a
        persistent workspace that cannot be re-entered. This panel is neither:{' '}
        <code>refs</code> is a free function over a throwaway db, so the counter above climbs
        once per keystroke and the timing beside it does not move.
      </p>

      <h2>providers — {table.length}</h2>
      <p className="lin-note">
        The registry, projected. A connector picker is populated from exactly this, and the
        extensions are what filter a file list. No db, no parse.
      </p>
      <ul className="lin-providers">
        {table.map((p: ProviderInfo) => (
          <li key={p.name}>
            <code>io.{p.name}</code>
            <span className={`lin-kind lin-kind-${p.kind}`}>{p.kind}</span>
            <span className="lin-dim">{p.extensions.map((e) => `.${e}`).join(' ') || 'no extensions'}</span>
          </li>
        ))}
      </ul>
    </div>
  );
}
