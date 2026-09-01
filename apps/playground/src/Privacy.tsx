/**
 * The declared bound, re-derived — the seventh thing this tab was asked to show.
 *
 * ## Why this is not a badge
 *
 * A corpus states what its bytes guarantee in a `privacy:` block, and the interesting property
 * of that block is not that it exists. It is that **a recipient can recompute every number in
 * it from the files alone** — no fossil, no policy document, no trust in the producer. Rendering
 * `k: 5 ✓` would restate the producer's claim in a larger font, and the producer is exactly the
 * party a recipient cannot check. So this panel does the recomputation, in the tab, with the
 * DuckDB the streaming panel already booted, over the same million rows it is already reading,
 * and shows the two columns beside each other.
 *
 * What that demonstrates is a property of the FORMAT rather than a property of this corpus. It
 * is the same check `apps/corpus/guards/check.mjs` runs natively as `declared-privacy`, done by
 * the reader instead of the writer.
 *
 * ## And why the green tick is still nearly worthless here, which the panel says out loud
 *
 * This corpus declares `k: 5` and reaches 5,000 over a million records — it clears its bound by
 * three orders of magnitude, and it has no `generalization` field, which reads as `none`: the
 * quasi-identifiers were published exactly as the program produced them. Nothing was traded to
 * get here. A reader who sees only "satisfied" learns the opposite of what happened, so the
 * margin and the generalisation are given the same weight as the tick, and the three things the
 * bound is silent about are stated rather than implied.
 *
 * The honest bottom of the panel is the corpus THIS TAB writes, which declares
 * `bound: undeclared` — see `Undeclared` below.
 */
import { useCallback, useEffect, useState } from 'react';

import type { Bench } from './bench.js';
import {
  agrees,
  margin,
  readDeclaration,
  recompute,
  type Agreement,
  type Declaration,
  type Recomputed,
} from './bound.js';
import './bound.css';

const n = (value: number) => value.toLocaleString();

export interface PrivacyProps {
  bench: Bench;
  /** The payload as the streaming panel registered it with DuckDB. */
  payload: string;
  /** False until the footer has been read — which is also when `payload` became registered. */
  ready: boolean;
}

export default function Privacy({ bench, payload, ready }: PrivacyProps) {
  const [found, setFound] = useState<Recomputed | null>(null);
  const [verdict, setVerdict] = useState<Agreement | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const declaration: Declaration = readDeclaration(bench.indexText);

  const onCheck = useCallback(async () => {
    if (declaration.state !== 'declared') return;
    setBusy(true);
    setError(null);
    try {
      const result = await recompute(payload, declaration.bound);
      setFound(result);
      setVerdict(agrees(declaration.bound, result));
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }, [declaration, payload]);

  // The declaration changes only when the corpus does, so a stale recomputation would be a lie
  // with a tick beside it rather than merely out of date.
  useEffect(() => {
    setFound(null);
    setVerdict(null);
  }, [bench.indexText]);

  if (declaration.state === 'absent') {
    return (
      <div className="pri">
        <h2>the declared bound — absent</h2>
        <p className="pri-note">
          This corpus carries no <code>privacy:</code> key, so it was written before the
          convention existed and <strong>nothing is claimed</strong>. That is not the same as
          declaring no bound, and neither is the same as "public".
        </p>
      </div>
    );
  }

  if (declaration.state === 'undeclared') {
    return (
      <div className="pri">
        <h2>the declared bound — none</h2>
        <p className="pri-note">
          The producer knows the convention and declares <code>bound: undeclared</code> over this
          release. It has said so about the release, not about the data.
        </p>
      </div>
    );
  }

  const bound = declaration.bound;

  return (
    <div className="pri">
      <h2>the declared bound, and the same numbers re-derived</h2>

      <p className="pri-note">
        <code>graph.graph.yml</code> states what these bytes guarantee. The claim the format makes
        is not that the statement is true — it is that <strong>a recipient can recompute it from
        the files alone</strong>. The button does that here, with the DuckDB already open on this
        page, over the same million rows the canvas is panning: no fossil, no policy document, no
        trust in whoever wrote the corpus.
      </p>

      <div className="pri-cols">
        <dl className="pri-declared">
          <div className="pri-head">declared</div>
          <div><dt>bound</dt><dd>{bound.bound}</dd></div>
          <div><dt>k</dt><dd>{n(bound.k)}</dd></div>
          <div><dt>reached</dt><dd>{n(bound.reached)}</dd></div>
          <div><dt>population</dt><dd>{n(bound.population)}</dd></div>
          <div><dt>suppressed</dt><dd>{n(bound.suppressed)}</dd></div>
          <div><dt>absent qi</dt><dd>{bound.absentQuasiIdentifier}</dd></div>
          <div><dt>generalization</dt><dd>{bound.generalization}</dd></div>
        </dl>

        <dl className="pri-found">
          <div className="pri-head">re-derived, here</div>
          {found ? (
            <>
              <div><dt>classes</dt><dd>{n(found.classes)}</dd></div>
              <div>
                <dt>reached</dt>
                <dd className={verdict?.reached ? 'ok' : 'bad'}>
                  {n(found.reached)} {verdict?.reached ? '= declared' : '≠ declared'}
                </dd>
              </div>
              <div>
                <dt>population</dt>
                <dd className={verdict?.population ? 'ok' : 'bad'}>
                  {n(found.population)} {verdict?.population ? '= declared' : '≠ declared'}
                </dd>
              </div>
              <div><dt>incomplete tuples</dt><dd>{n(found.incomplete)}</dd></div>
              <div>
                <dt>reached ≥ k</dt>
                <dd className={verdict?.satisfied ? 'ok' : 'bad'}>{verdict?.satisfied ? 'yes' : 'NO'}</dd>
              </div>
              <div><dt>cost</dt><dd>{found.ms.toFixed(0)} ms, one scan</dd></div>
            </>
          ) : (
            <div className="pri-empty">
              <dd>{busy ? 'recomputing over every tile…' : 'not computed — nothing has been read'}</dd>
            </div>
          )}
        </dl>
      </div>

      <div className="pri-actions">
        <button onClick={onCheck} disabled={!ready || busy}>
          {busy ? 'recomputing…' : 'Re-derive it from the bytes'}
        </button>
        <span className="pri-note">
          {ready ? 'one GROUP BY over the whole payload, read as one relation' : 'read the footer first'}
        </span>
      </div>

      {error && <p className="pri-bad">{error}</p>}

      {found && (
        <>
          <pre className="pri-sql">{found.sql}</pre>
          <p className="pri-note">
            <strong>The class is the release, not the tile.</strong> That query reads every tile
            of the payload as one relation. Aggregating per tile is the easiest wrong answer
            available here — it is more parallel, it looks the same, and every class it finds is a
            subset of a real one, so it reports a <em>k</em> that is too small and looks
            conservative while never having been checked against the release.
          </p>
        </>
      )}

      {found && verdict && (
        <p className={verdict.all ? 'pri-verdict ok' : 'pri-verdict bad'}>
          {verdict.all
            ? `the manifest and the bytes agree — and that is the whole of what this proves`
            : `the manifest and the bytes DISAGREE`}
        </p>
      )}

      {/*
        The part that keeps the tick honest. Without it this panel is the badge it was written
        not to be: a large green number over a corpus that never had to work for it.
      */}
      {found && (
        <div className="pri-caveats">
          <h3>what the green does not mean</h3>
          <ul>
            <li>
              <strong>It cleared the bar by {Math.round(margin(bound, found)).toLocaleString()}×.</strong>{' '}
              <code>k</code> is {n(bound.k)} and the smallest class holds {n(found.reached)}. A bound
              this far from binding says the data was never near it — not that it was carefully
              protected. The number to watch is a <em>reached</em> close to <em>k</em>.
            </li>
            <li>
              <strong>Nothing was generalised: <code>{bound.generalization}</code>.</strong> The
              quasi-identifiers were published exactly as the program produced them. A{' '}
              <code>reached</code> over raw postcodes and the same <code>reached</code> over
              postcodes truncated to three characters are not the same release — the second traded
              resolution the first still has — and recomputing <em>k</em> gives the same number
              either way. This field is the only one that tells them apart.
            </li>
            <li>
              <strong>A demanding <code>k</code> does not get refused, it gets generalised.</strong>{' '}
              The top of every hierarchy is <code>*</code>, and one class holding everybody
              satisfies any bound it is large enough for — so asking for more produces a valid,
              useless release whose <code>reached</code> looks just like a real one. Utility is not
              something a verifier can measure, so the writer does not refuse it; what it does is
              refuse to let the flattening be invisible, by reporting the level each column
              reached. A coarsest and a finest that are equal, short of the declared count, is a
              column every row of which publishes one value.
            </li>
            <li>
              <strong>The quasi-identifier set may simply be wrong.</strong>{' '}
              <code>{bound.quasiIdentifiers.join(' ')}</code> is a judgement about a jurisdiction, a
              recipient and what else was released — not a property of a column — and no amount of
              scanning recovers it. It is the one field above that a reader cannot derive, and the
              whole bound turns on it. A corpus declaring a set of one reaches a large <em>k</em>{' '}
              honestly and protects nobody.
            </li>
            <li>
              <strong>The subject may not be opaque, and attribute disclosure is untouched.</strong>{' '}
              The bound is over the quasi-identifier tuple, not the record: nothing on disk
              distinguishes <code>person/8a3f…</code> from <code>person/nhs-4433221</code>. And every
              record in a class of 5,000 sharing one diagnosis satisfies <em>k</em>=5,000 and
              discloses the diagnosis of all 5,000.
            </li>
          </ul>
        </div>
      )}
    </div>
  );
}

/**
 * What the corpus this tab just wrote declares, which is nothing — and why.
 *
 * Rendered beside the run output rather than here, because the honest statement about the
 * browser path belongs next to the browser path's own artifact. `fossil-df-wasm`'s executor
 * takes no policy: `FossilExecutor.run` has no parameter for one, and the only call site of
 * `generalize::apply` / `privacy::verify` in the workspace is `fossil-df`'s `run_to_dir`, which
 * is `#[cfg(not(target_arch = "wasm32"))]`. So a browser run emits `bound: undeclared`, always,
 * and no program written in this tab can currently say otherwise.
 *
 * The algorithms are already in the wasm module — `fossil-df` depends on `fossil-kanon` and
 * `fossil-policy` unconditionally and neither `generalize` nor `privacy` is cfg-gated. What is
 * missing is the API seam and a way for the browser to reach a policy document. Saying so is
 * better than a panel that quietly shows the bench corpus's bound and lets a reader assume the
 * tab produced it.
 */
export function Undeclared() {
  return (
    <p className="pri-note pri-inline">
      <strong>
        This corpus declares <code>bound: undeclared</code>
      </strong>{' '}
      — and every corpus written in this tab does. The k-anonymity writer is compiled into the
      executor, but <code>FossilExecutor.run</code> takes no policy and the one call site that
      derives and verifies a bound is native-only, so the browser has no way to hand it one. The
      panel below re-derives a bound over a corpus that <em>was</em> written with one.
    </p>
  );
}
