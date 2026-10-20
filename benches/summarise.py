"""Tables from `run.py`'s results: the median of the repetitions, per workload.

    python3 benches/summarise.py benches/results > benches/results/summary.md
"""

import csv
import json
import statistics
import sys
from collections import defaultdict
from pathlib import Path

ENGINES = ["fossil", "fossil-node", "rmlmapper", "morph-kgc", "morph-kgc-1p", "sdm-rdfizer"]


def mb(b):
    return f"{int(b) / 2**20:,.2f}" if b not in ("", None) else "–"


def main():
    out = Path(sys.argv[1])
    env = json.loads((out / "env.json").read_text())
    runs = list(csv.DictReader(open(out / "runs.csv")))
    checks = list(csv.DictReader(open(out / "verify.csv")))

    by = defaultdict(list)
    for r in runs:
        by[(r["workload"], int(r["rows"]), r["engine"])].append(r)

    print("# Results\n")
    print(f"{env['date']} · {env['machine']} · {env['cpus']} CPUs · {env['memory_gb']} GiB · "
          f"fossil `{env['fossil']}` · {env['rmlmapper']} on {env['java']} (`-Xmx{env['java_xmx']}`) · "
          f"Morph-KGC {env['morph_kgc']} and SDM-RDFizer {env.get('sdm_rdfizer', '–')} on "
          f"{env['python']} · Node {env.get('node', '–')} · median of {env['repeat']} · "
          f"timeout {env['timeout_s']:.0f} s\n")
    print("Wall time is the whole process, start-up included. Memory is the peak resident set "
          "of the process tree. Output is N-Triples for the RML processors and the GraphAr "
          "directory for fossil.\n")

    for w in dict.fromkeys(r["workload"] for r in runs):
        print(f"## {w}\n")
        print("| rows | input MB | engine | wall s | CPU s | peak MB | output MB | vs fossil |")
        print("|---:|---:|---|---:|---:|---:|---:|---:|")
        for n in sorted({int(r["rows"]) for r in runs if r["workload"] == w}):
            base = None
            for e in ENGINES:
                rs = by.get((w, n, e))
                if not rs:
                    continue
                ok = [r for r in rs if r["exit"] == "0"]
                if not ok:
                    why = "timeout" if rs[0]["timed_out"] == "True" else f"exit {rs[0]['exit']}"
                    print(f"| {n:,} | {mb(rs[0]['input_bytes'])} | {e} | {why} | | "
                          f"{float(rs[0]['tree_rss_mb']):,.0f} | | |")
                    continue
                med = lambda k: statistics.median(float(r[k]) for r in ok)
                wall = med("wall_s")
                if e == "fossil":
                    base = wall
                ratio = f"{wall / base:,.1f}×" if base and e != "fossil" else ""
                print(f"| {n:,} | {mb(ok[0]['input_bytes'])} | {e} | {wall:,.2f} | "
                      f"{med('cpu_s'):,.2f} | {med('tree_rss_mb'):,.0f} | {mb(ok[0]['out_bytes'])} | "
                      f"{ratio} |")
        print()

    print("## Same triples\n")
    print("fossil's graph read back through the `mapping.ttl` it wrote, against each processor's "
          "N-Triples, as sets: of RDF terms, and of values (IRIs percent-decoded, numbers by "
          "number). The counts and the example are of terms.\n")
    print("| workload | rows | engine | triples | same terms | same values | only fossil | "
          "only engine | example |")
    print("|---|---:|---|---:|---|---|---:|---:|---|")
    for c in checks:
        example = c["example"].replace("|", "\\|")
        print(f"| {c['workload']} | {int(c['rows']):,} | {c['engine']} | {int(c['triples']):,} | "
              f"{c['same_terms']} | {c['same_values']} | {c['only_fossil']} | {c['only_engine']} | "
              f"{example} |")


if __name__ == "__main__":
    main()
