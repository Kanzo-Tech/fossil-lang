/**
 * The loop, in one screen.
 *
 * Left: the program, in a textarea, re-checked on every keystroke. Right: what happened —
 * diagnostics, then the files the run wrote, then the rows read back out of them.
 *
 * A textarea and a table is the whole of the first cut on purpose. The owner's bar is that
 * it genuinely runs: `examples/hello.fossil` must produce the same five subjects here that
 * `fossil run` produces natively. A CodeMirror editor with `fossil-ide`'s hover and
 * completion behind it is the obvious next thing and it is strictly the second thing —
 * `@fossil-lang/wasm` already exposes `tokenize` and `semanticLegend` for it.
 */
import { useCallback, useEffect, useRef, useState } from 'react';

import * as checker from './check.js';
import * as corpus from './corpus.js';
import * as duck from './duckdb.js';
import { CSV_BYTES, CSV_PATH, PROGRAM } from './example.js';
import { describeCsv } from './descriptor.js';
import * as runner from './run.js';

type Phase = 'booting' | 'ready' | 'running' | 'done' | 'failed';

interface Table {
  columns: string[];
  rows: string[][];
}

const KB = (bytes: number) => `${(bytes / 1024).toFixed(0)} kB`;

export default function App() {
  const [program, setProgram] = useState(PROGRAM);
  const [phase, setPhase] = useState<Phase>('booting');
  const [diagnostics, setDiagnostics] = useState<checker.CheckRow[]>([]);
  const [log, setLog] = useState<string[]>([]);
  const [files, setFiles] = useState<{ path: string; bytes: number }[]>([]);
  const [table, setTable] = useState<Table | null>(null);
  const [error, setError] = useState<string | null>(null);
  const booted = useRef(false);

  const say = useCallback((line: string) => setLog((prior) => [...prior, line]), []);

  // Boot: DuckDB first (the checker needs an introspected descriptor before its first
  // useful answer), then the checker, then the descriptor, then the first check.
  useEffect(() => {
    if (booted.current) return;
    booted.current = true;
    (async () => {
      try {
        await duck.boot();
        const dc = duck.duckdbCost();
        if (dc) say(`duckdb-wasm  ${KB(dc.bytes)}  ${dc.ms} ms`);

        await checker.load(PROGRAM);
        const cc = checker.checkerCost();
        if (cc) say(`fossil-wasm  ${KB(cc.bytes)}  ${cc.ms} ms`);

        // Introspect the CSV and tell the compiler what it found — the browser's version of
        // what `fossil-cli` does with `fossil-introspect` before every compile.
        const descriptor = await describeCsv(CSV_PATH, CSV_BYTES);
        checker.registerDescriptor(descriptor);
        say(`introspected ${CSV_PATH}: ${descriptor.columns.map((c) => `${c.name}:${c.primitive}`).join(', ')}`);

        setDiagnostics(checker.check());
        setPhase('ready');
      } catch (cause) {
        setError(String(cause));
        setPhase('failed');
      }
    })();
  }, [say]);

  // Debounced, the way an LSP client coalesces `didChange` rather than emitting one per
  // character — and here it is not only manners: see `check.ts`'s `busy` guard for the
  // re-entrancy the wasm surface cannot survive. 120 ms is below the threshold where an
  // editor stops feeling live and well above a fast typist's inter-key interval.
  const pending = useRef<number | undefined>(undefined);

  const onEdit = (next: string) => {
    setProgram(next);
    if (phase === 'booting') return;
    window.clearTimeout(pending.current);
    pending.current = window.setTimeout(() => {
      try {
        setDiagnostics(checker.update(next));
      } catch (cause) {
        setError(String(cause));
      }
    }, 120);
  };

  const onRun = async () => {
    setPhase('running');
    setError(null);
    setTable(null);
    setFiles([]);
    try {
      const started = performance.now();
      const result = await runner.run(program);
      const ec = runner.executorCost();
      if (ec) say(`fossil-df-wasm  ${KB(ec.bytes)}  ${ec.ms} ms`);
      say(`ran in ${Math.round(performance.now() - started)} ms → ${result.files.length} files`);

      setFiles(result.files.map((f) => ({ path: f.path, bytes: f.bytes.byteLength })));

      await corpus.stage(result.files);
      // The stand-in for the layout pass. See the seam note in `corpus.ts` — this is the
      // one thing in the app that is not what the native pipeline does.
      await corpus.retile(result.report);

      // The door: does the manifest address itself, and can the reader open it? This is the
      // corpus half of the claim — `openCorpus` computes every tile URL by arithmetic before
      // its first request, and it cannot tell that the tiles live in a worker's memory
      // rather than behind HTTP. A refusal here is worth reading: it names the exact file it
      // addressed and why, which is how the container mismatch above was found.
      try {
        const opened = await corpus.open();
        const described = opened.types.vertices
          .map((v) => `${v.type} ×${v.count}${v.indexed ? ' (indexed)' : ''} [${v.fields.map((f) => f.name).join(' ')}]`)
          .join('; ');
        say(`openCorpus opened: ${described || 'no vertex types'}`);
        const first = opened.types.vertices[0];
        if (first?.identity) {
          const one = await opened.node('https://example.org/user/3');
          say(`corpus.node("…/user/3") → ${one ? JSON.stringify(one.fields) : 'null'}`);
        }
      } catch (cause) {
        say(`openCorpus refused: ${String(cause)}`);
      }

      const type = result.report.vertices[0]?.type;
      if (type) setTable(await duck.queryForDisplay(corpus.subjectsQuery(type)));
      setPhase('done');
    } catch (cause) {
      setError(String(cause));
      setPhase('failed');
    }
  };

  const runnable = (phase === 'ready' || phase === 'done' || phase === 'failed') && !checker.hasErrors(diagnostics);

  return (
    <>
      <header>
        <h1>fossil playground</h1>
        <span className="note">
          check → run → query, all in this tab. The CSV is never uploaded because there is nowhere to upload it to.
        </span>
      </header>
      <main>
        <section>
          <h2>program — {phase === 'booting' ? 'loading the checker…' : 'hello.fossil'}</h2>
          <textarea value={program} onChange={(e) => onEdit(e.target.value)} spellCheck={false} />
          <div>
            <button onClick={onRun} disabled={!runnable}>
              {phase === 'running' ? 'running…' : 'Run'}
            </button>
          </div>
        </section>

        <section>
          <h2>diagnostics</h2>
          {diagnostics.length === 0 ? (
            <p className="clean">{phase === 'booting' ? '…' : 'no diagnostics — the program type-checks'}</p>
          ) : (
            <ul className="diagnostics">
              {diagnostics.map((row, i) => (
                <li key={i} className={`sev-${row.severity}`}>
                  {row.range.start.line + 1}:{row.range.start.character + 1} {row.message}
                </li>
              ))}
            </ul>
          )}

          {error && <p className="log bad">{error}</p>}

          {files.length > 0 && (
            <>
              <h2>corpus written</h2>
              <ul className="files">
                {files.map((f) => (
                  <li key={f.path}>
                    {f.path} — {f.bytes} B
                  </li>
                ))}
              </ul>
            </>
          )}

          {table && (
            <>
              <h2>read back</h2>
              <div className="scroll">
                <table className="rows">
                  <thead>
                    <tr>
                      {table.columns.map((c) => (
                        <th key={c}>{c}</th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {table.rows.map((row, i) => (
                      <tr key={i}>
                        {row.map((cell, j) => (
                          <td key={j}>{cell}</td>
                        ))}
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </>
          )}

          <h2>what it cost</h2>
          <pre className="log">{log.join('\n') || '…'}</pre>
        </section>
      </main>
    </>
  );
}
