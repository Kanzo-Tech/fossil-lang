/**
 * The loop, in one screen.
 *
 * Left: the program, in `@kanzo-tech/ui`'s `CodeEditor` with fossil's language layer
 * inside it — highlighting, squiggles, hover, completion and go-to-definition, all five
 * answered by the compiler in this tab. Right: what happened — diagnostics, then the files
 * the run wrote, then the rows read back out of them.
 *
 * The bar has not moved: `examples/hello.fossil` must produce the same five subjects here
 * that `fossil run` produces natively. What changed is that the editor is now an editor.
 * This file wires it and owns none of it — the extensions are
 * `@fossil-lang/codemirror-fossil`'s and the answers are `crates/fossil-ide`'s, reached
 * through `check.ts`.
 */
import { CodeEditor } from '@kanzo-tech/ui/editor';
import { fossil } from '@fossil-lang/codemirror-fossil';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import * as checker from './check.js';
import * as corpus from './corpus.js';
import * as duck from './duckdb.js';
import { CSV_BYTES, CSV_PATH, PROGRAM, PROGRAM_PATH } from './example.js';
import { describeCsv } from './descriptor.js';
import Lineage from './Lineage.js';
import * as runner from './run.js';
import Streaming from './Streaming.js';

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
  const [definition, setDefinition] = useState<string | null>(null);
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

  // The language layer, built once. `CodeEditor` reconfigures its `Compartment` on the
  // REFERENTIAL identity of `extensions`, so an inline array would rebuild the editor's
  // language on every render — the empty dependency list is load-bearing, not tidiness.
  //
  // Nothing here debounces. `fossil()`'s linter waits out its own `delay` and then waits
  // for the check to return before scheduling the next one, so the coalescing an LSP
  // client does with `didChange` is in the library rather than in this file. It used to
  // be here, as a `setTimeout` plus a `busy` flag in `check.ts`, guarding a wasm
  // re-entrancy defect that poisoned the workspace permanently — see `check.ts` for
  // where that went.
  //
  // Four sources at four rates, one workspace, and every one of them takes the text:
  // `check.ts`'s `sync` is what makes that safe to say and cheap to do.
  const extensions = useMemo(
    () =>
      fossil({
        tokenize: checker.tokenize,
        tokenKinds: checker.tokenKinds,
        uri: PROGRAM_PATH,
        check: checker.checkText,
        // The panel below renders the same rows the squiggles do, from one check.
        onDiagnostics: (rows) => setDiagnostics([...rows]),
        hover: checker.hoverAt,
        complete: checker.completeAt,
        definition: checker.definitionAt,
        // One pane, so a definition in `hello.shex` cannot be a jump — and two of the
        // four positions goto-def recognises resolve into exactly that file. Reporting
        // where it is beats moving the cursor to the same coordinates in the wrong
        // buffer, which is what a host that ignored `uri` would do.
        onNavigate: (target) =>
          setDefinition(
            target === null
              ? 'no definition at the cursor'
              : `${target.uri}:${target.range.start.line + 1}:${target.range.start.character + 1}`,
          ),
      }),
    [],
  );

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

      // The files are the layout pass's output, not the executor's staged tree: the pass
      // runs inside `fossil-df-wasm` now, so staging is a registration and nothing more.
      // The `retile()` stand-in that stood here is gone — see the seam note in `corpus.ts`.
      await corpus.stage(result.files);

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
          <CodeEditor
            value={program}
            onChange={setProgram}
            extensions={extensions}
            lineNumbers
            minHeight="24rem"
            maxHeight="60vh"
            invalid={checker.hasErrors(diagnostics)}
          />
          <div>
            <button onClick={onRun} disabled={!runnable}>
              {phase === 'running' ? 'running…' : 'Run'}
            </button>
            <span className="note">
              hover for a type · ctrl-space to complete · alt-. or ⌘-click for the definition
            </span>
          </div>
          {definition && <p className="log">definition → {definition}</p>}
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

          {/*
            The parse-only half. Reads `program` and nothing else from this component — the
            bundle sizes are read straight off the modules that measured them, so no state is
            added here for it. Self-contained by design: see `Lineage.tsx`.
          */}
          <Lineage
            program={program}
            ready={phase !== 'booting'}
            checkerBytes={checker.checkerCost()?.bytes ?? null}
            duckdbBytes={duck.duckdbCost()?.bytes ?? null}
            executorBytes={runner.executorCost()?.bytes ?? null}
          />

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

        {/*
          The larger-than-the-tab half, and it is deliberately its own section rather than a
          tail on the run output: it reads a corpus this program did not write, at a size the
          five-row demo cannot reach. Its only dependency on the app shell is that DuckDB has
          booted — it needs a Parquet reader for the footer and brings none of its own.
        */}
        <section>
          <Streaming ready={phase !== 'booting'} />
        </section>
      </main>
    </>
  );
}
