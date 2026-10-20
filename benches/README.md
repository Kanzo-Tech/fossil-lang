# Benchmark: fossil against the RML processors

The same graph built from the same inputs by fossil and by three RML processors. Each run measures
time, memory and output size, and the triples are compared, because a speed figure for two programs
that build different graphs measures nothing.

## What it answers

1. **Cost.** For each engine and input size: the wall time, the CPU time, the peak memory and the
   size of what was written.
2. **Equivalence.** Whether fossil and each processor define the same set of triples. They are compared
   as RDF terms, and again as values (IRIs percent-decoded, numbers compared as numbers), so that a
   difference in spelling is not mistaken for a different graph.
3. **Where fossil's time goes.** The fossil hosts report `serve_ms` (reading the inputs), `run_ms`
   (compiling, running the queries and encoding the output) and `write_ms` (putting the files on
   disk).

## Engines

| engine | version | how it runs |
|---|---|---|
| `fossil` | this checkout | `fossil-driver/`: `fossil_df::Executor` natively, the host `crates/fossil-df/tests/support/native.rs` describes — inputs served from memory, graph written into memory, then copied to disk |
| `fossil-node` | this checkout | `engines/fossil-node.mjs`: `@fossil-lang/executor` in Node, the wasm32 build that runs in production — one thread, linear memory up to 4 GiB |
| `rmlmapper` | 8.1.0 | `java -Xmx8g -jar rmlmapper-8.1.0-r380-all.jar -m mapping.rml.ttl -s nquads` |
| `morph-kgc` | 2.10.0 | its default of two processes per CPU |
| `morph-kgc-1p` | 2.10.0 | `number_of_processes=1`, which separates its parallelism from its algorithm |
| `sdm-rdfizer` | 4.7.5.15.4.1 | `remove_duplicate: yes`, one output file |

`setup.sh` installs the three processors at these versions. Each processor runs the mapping in
`mapping.rml.ttl`. Morph-KGC reads neither RML-IO source description, so `engines/morph_dialect.py`
rewrites only the source description into the form Morph-KGC reads:

- a relative path becomes the path literal it reads;
- a CSVW table becomes its own "tabular view" (an `rml:query` run on DuckDB), because its fallback
  delimiter sniffer takes the `.` in `Person.id` for the delimiter.

## Workloads

### Synthetic: four programs from `docs/programs`

The programs run unchanged; only their data is scaled, by `workloads/synthetic/generate.py`,
deterministically from `--seed`. `rows` is the size of the main source.

| workload | what it exercises | sources | RML counterpart (`workloads/synthetic/rml/`) |
|---|---|---|---|
| `hello` | one subject template and one property | 1 | one triples map |
| `reviews` | an edge built from the target's template, no join | 2 | a template object map |
| `shop` | a filter, and an edge through a join on a foreign key | 2 | a referencing object map with a join condition |
| `relationship` | LDBC-SNB's `\|`-delimited files, two many-to-many relations | 5 | a triples map per relationship file, with a CSVW dialect |

The mappings are written by hand in RML-Core (`http://w3id.org/rml/`). Where a mapping and its
program are not the same text, the generator makes them define the same graph:

- **`shop`'s `where`.** RML-Core has no filter, so every generated user is an adult and the filter
  keeps every row.
- **`relationship`'s joins.** The program joins `person.csv` to each relationship file; the mapping
  reads the relationship file alone. Every id resolves, so the two agree.
- **LDBC's repeated header.** `person_knows_person.csv` is headed `Person.id|Person.id_1`, the name
  fossil's profiler gives the second column, because no RML processor can address two columns that
  share a name.

### GTFS-Madrid-Bench

The usual benchmark for knowledge-graph construction (Chaves-Fraga et al., *Journal of Web
Semantics* 65, 2020), in its CSV distribution:

- **Data.** `workloads/gtfs/generate.sh` runs the benchmark's own generator from its Docker image,
  pinned by digest. `rows` is the scale factor.
- **Mapping.** `upstream.rml.ttl` is the benchmark's `mappings/gtfs-csv.rml.ttl`, unchanged, in
  legacy RML.
- **Program.** `gtfs.fossil` and `gtfs.shex` are written here so that they define the same graph,
  with one fossil mapping per upstream triples map.

Materialisation only: the benchmark's queries evaluate virtual knowledge graphs and are out of
scope. At scale 1 the input is ten files and 63,250 rows.

Two departures, both on both sides, so the processors still run the upstream mapping:

- **Five predicates are cut** (`fragment.py`): `routeType`, `locationType`, `wheelchairAccessible`
  (on two maps), `pickupType` and `dropOffType`. Each is an IRI minted from a template over a code
  column (`…/RouteType/{route_type}`). fossil cannot turn text into an IRI that is not a subject,
  and the checker refuses the program with `PropertyMismatch { expected: AnyUri, actual: String }`.
  The cut is what fossil can express today, and the five come back when it can mint such an IRI.
- **`shapes_0` is a quadratic self-join.** It links a shape to its points by joining `SHAPES.csv`
  to itself on `shape_id`: 107 million pairs at scale 1, for 58,540 distinct triples. The
  processors run it as published. `gtfs.fossil` reads each point once, which defines the same
  triples; written as the join, fossil refuses the run at its 2 GiB pool (`run/over-budget`). The
  table says which side did which.

## Metrics and protocol

- **Time.** Wall time of the whole process, so JVM and Python start-up are in it, plus CPU time
  (user + system) over every process of the run.
- **Memory.** The peak resident set of the process tree, sampled every 20 ms, and the exact peak of
  the largest single process (`wait4`). The two differ for Morph-KGC, which forks workers. The JVM
  gets `-Xmx8g`. fossil holds itself to the 2 GiB pool of `crates/fossil-df/src/memory.rs` and
  refuses past it as `run/over-budget`; a refusal is a result and is reported as one.
- **Output.** The processors write N-Triples. fossil writes Apache GraphAr today, so its column is
  the graph a consumer receives from it, not a second encoding of the same file.
- **Repetitions.** `--repeat` runs per point (3 by default); the summary takes the median. A failure
  or a timeout (`--timeout`, 30 minutes by default) is recorded once, and that engine is not run
  again at that size.
- **Equivalence.** Checked up to `--verify-max` rows. fossil's graph is read back as RDF through the
  `mapping.ttl` it wrote (`rml::sql` on DuckDB, Statement R as `crates/fossil-df/tests/mapping.rs`
  holds it), then compared with each processor's N-Triples as sets.
- **Environment.** `env.json` names the machine and every version.

## What it cannot say

- **Small inputs measure start-up.** At 1,000 rows, JVM and Python start-up are most of what is
  measured. Conclusions come from 100,000 rows and up.
- **Join work is not symmetric.** fossil resolves an edge against the target's vertices, while the
  synthetic RML mappings use a plain template where that defines the same triples. The joins the
  benchmark asks of both sides are `shop` and GTFS's referencing object maps.
- **The output column compares formats, not efficiency.** It compares Parquet with N-Triples until
  fossil writes N-Triples itself.
- **Not tested yet:** `fossil-node`, which needs the wasm build, and runs larger than RAM. The
  driver only covers a host whose sources are served from memory, and the executor has nowhere to
  spill.

## Changes coming from fossil's design

The engine design closed on 20 October 2026 (`arquitectura/separar-motor.md` in the project files)
changes three things here:

- **The N-Triples sink (S1).** fossil is measured through it, the output column compares like with
  like, and equivalence is checked on fossil's own triples (⟦P⟧) instead of being read back from
  `mapping.ttl`.
- **`mapping.ttl` moves to `graphar-rdf` (G1).** `readback.py` then becomes the cross-check
  `graphar-rdf`(GraphAr(P)) = ⟦P⟧, which is the second paper's claim.
- **IRI-safe templates and source-scale decimals (S5).** Before them, fossil writes
  `…/user/user6@shop.example` where RMLMapper and Morph-KGC write `…/user6%40shop.example`, and
  `"102.7"` where all three keep `"102.70"`. SDM-RDFizer does not percent-encode either, so the
  processors also disagree among themselves; the per-engine equivalence table shows it.

## Divergences the first runs found

At 1,000 rows and at GTFS scale 1 (one repetition each), every engine writes the same number of
triples, and these are the differences between them:

| divergence | where | same value? | who changes |
|---|---|---|---|
| `…/user6@shop.example` against `…/user6%40shop.example` | `reviews`, `shop` | yes | fossil, through IRI-safe templates (decided) |
| `"526.0"` against `"526.00"` (`xsd:float`) | `shop` | yes | fossil, through S5 |
| `"false"` against `"0"` (`xsd:boolean`) | GTFS | yes | nobody: both are the same `xsd:boolean`; fossil writes the canonical form |
| `agency/1` against `agency/00000000000000000001` | GTFS: every all-digit identifier, in IRIs and literals (1,495 triples at scale 1) | **no** | fossil: its profiler types an all-digit column as an integer and the leading zeros are gone before the template sees them. A program has no way to keep the column as text |
| no percent-encoding at all | SDM-RDFizer, `reviews` and `shop` | yes | SDM-RDFizer, against R2RML's IRI-safe rule |

The fourth is the only one that changes the graph, and on GTFS it changes identifiers. It has to be
fixed in fossil before GTFS figures go into a paper.

Once the engine has spill (M2), a run larger than RAM is a `hello` or GTFS size past the machine's
memory, with `--timeout` raised to match. Comparing runs with and without the elimination of
blocking operators (§4.2) is a second `fossil` row with that analysis switched off, once there is a
switch.

## Running it

```sh
bash benches/setup.sh                                                   # into benches/.tools
cargo build --release --manifest-path benches/fossil-driver/Cargo.toml  # target/release/fossil-bench
python3 benches/run.py --tools benches/.tools
python3 benches/run.py --tools benches/.tools --workloads gtfs --sizes 1,5,10
python3 benches/summarise.py benches/results > benches/results/summary.md
```

`fossil-driver` is a package of its own, outside the workspace's `members`. It adds nothing to any
gate, and the workspace has no say in its build. It builds into the workspace's `target/`, from a
`Cargo.lock` copied from the root one.
