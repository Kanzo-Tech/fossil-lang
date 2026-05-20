# DuckDB-WASM parity harness (SC#1 manual tier — ADR-0014)

This is the **manual, on-demand** tier of the two-tier cross-engine SQL parity
strategy that discharges SC#1's claim:

> each generated SQL statement executes identically (same result-set bytes) on
> native DuckDB 1.10502 **and** DuckDB-WASM 1.33.x.

Running the full DuckDB-WASM bundle (~6.4 MB MVP) in CI on every PR is too
heavy, so the parity is split into tiers (ADR-0014):

| Tier | Where | When | What it proves |
|------|-------|------|----------------|
| 1. Snapshot | `crates/fossil-codegen/tests/corpus.rs` | every PR (`cargo test`) | the 30-mapping corpus' generated SQL text is stable |
| 2. Native exec | `crates/fossil-runtime/tests/corpus_exec.rs` | every PR (`cargo test`) | the executable subset runs on native `duckdb` 1.10502, asserts result bytes, and **writes** `native_baseline.json` + `corpus_sql.json` |
| 3. WASM exec (THIS) | `tests/wasm_parity/run-parity.mjs` | **manual, phase close** | the SAME SQL runs on DuckDB-WASM, reproduces the digests, and diffs them against the native baseline |

## The two-tier contract

Tier 2 (`corpus_exec.rs`) is the **producer**: every time it runs it (re-)writes
two checked-in artifacts under `crates/fossil-codegen/tests/wasm_parity/`:

- **`native_baseline.json`** — one object per natively-executed mapping:
  ```json
  { "name": "<mapping>", "sql_sha256": "<sha256 of the SQL text>",
    "row_count": <int>, "result_sha256": "<sha256 of the ordered result set>" }
  ```
- **`corpus_sql.json`** — `{ "<mapping>": "<SQL text>" }`, so the SQL stays
  **single-sourced** from the Rust corpus (this harness never re-derives it).

Tier 3 (this harness) is the **consumer**: it reads both artifacts, re-runs each
SQL on DuckDB-WASM, recomputes the digests with the identical serialization, and
asserts equality — exiting non-zero on any divergence.

### The result-set serialization (kept in sync with `corpus_exec.rs`)

Both engines must produce the SAME `result_sha256`. The serialization is:

1. each corpus SQL ends in a deterministic `ORDER BY` so row order is stable;
2. every projected column is `CAST(... AS VARCHAR)` so cell text is engine-portable;
3. each cell → its UTF-8 text (NULL → the empty string);
4. the cells of one row are joined with `|` (U+007C);
5. the rows are joined with `\n` (U+000A);
6. the resulting string is hashed with SHA-256.

`sql_sha256` is the SHA-256 of the exact SQL text — verified FIRST so a digest
mismatch can never be blamed on the two engines running different SQL.

## Version asymmetry (RESEARCH Pitfall 6)

Native is `duckdb` 1.10502; WASM is DuckDB-WASM 1.33.x. The versions are
intentionally different — the baseline + this harness are exactly the mechanism
that **catches** any divergence between them. The corpus sticks to portable SQL
(`read_csv_auto`, `CAST AS VARCHAR`, `ORDER BY`, `GROUP BY`, `UNION`,
`DISTINCT ON`).

> Note on the npm pin: the DuckDB-WASM 1.33.x line is currently published only as
> the npm `latest` dev pre-release (`1.33.1-devN.0`); the last fully-stable npm
> tag is `1.32.0`. `package.json` pins `latest` (a 1.33.x build). Bump to a
> 1.33.x final tag when one ships.

## How to run (the phase-close gate)

```bash
cd tests/wasm_parity
npm install            # fetches @duckdb/duckdb-wasm 1.33.x (~6.4 MB)
node run-parity.mjs    # exits 0 iff every entry matches native_baseline.json
```

`node_modules/` and `package-lock.json` are gitignored — only the harness
sources (`package.json`, `run-parity.mjs`, this README) are committed.

## Regenerating the baseline

Re-run the native tier (it rewrites both artifacts):

```bash
cargo test -p fossil-runtime --test corpus_exec
```

Then re-run the harness to confirm WASM still matches. A baseline diff in a PR
means the corpus SQL or fixtures changed — review it like any other snapshot.

## When the run is recorded

`node run-parity.mjs` exiting 0 is the documented **phase-close** evidence; its
PASS is recorded in the Phase 4 SUMMARY (plan 04-08). It is NOT part of
`cargo test` / CI.
