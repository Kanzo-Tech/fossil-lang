"""Run the benchmark: fossil against the RML processors, over the same inputs.

    python3 benches/run.py --tools <dir> [--workloads …] [--sizes …] [--engines …]

For every workload and size it lays out one directory holding the fossil program, its shape
document, the data and the RML mapping, and runs each engine there `--repeat` times. Per run:

- `wall_s`, `cpu_s` (user + system over every process of the run);
- `rss_mb`, the peak resident set of the largest single process (`wait4`), and
  `tree_rss_mb`, the peak of the sum over the process tree, sampled every 20 ms — they differ
  for Morph-KGC, which forks workers;
- `out_bytes`, what the engine wrote;
- for fossil, the timings it reports itself (`serve_ms`, `run_ms`, `write_ms`).

Then, once per workload and size up to `--verify-max`, it compares the triples: fossil's graph
read back as RDF (`readback.py`) against each processor's N-Triples, as sets of RDF terms (a
literal with no datatype is `xsd:string`) and as sets of values (IRIs percent-decoded, numbers
by number, booleans by truth), so a difference of spelling is told apart from a difference of graph.

`<out>/runs.csv`, `<out>/verify.csv` and `<out>/env.json` (machine and versions) are what
`summarise.py` reads. `README.md` says what each workload and engine is.
"""

import argparse
import csv
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import threading
import time
import urllib.parse
from pathlib import Path

BENCH = Path(__file__).resolve().parent
REPO = BENCH.parent
sys.path.insert(0, str(BENCH / "engines"))
from morph_dialect import rewrite  # noqa: E402

XSD = "http://www.w3.org/2001/XMLSchema#"
XSD_STRING = f"<{XSD}string>"
NUMERIC = re.compile(r'"([^"]*)"\^\^<' + re.escape(XSD) + r'(float|double|decimal|integer)> \.$')
IRI = re.compile(r"<([^>]*)>")
BOOLEAN = re.compile(r'"(0|1|false|true)"\^\^<' + re.escape(XSD) + r'boolean> \.$')


# ─────────────────────────────────────────────────────────────── workloads

class Synthetic:
    """A program of `docs/programs`, unchanged, over `synthetic/generate.py`'s data; `rows` is
    the size of its main source. The mapping is RML-Core (`http://w3id.org/rml/`)."""
    rml = "core"

    def __init__(self, name):
        self.name = name
        self.program = f"{name}.fossil"

    def lay_out(self, run, rows, a):
        for f in (REPO / "docs/programs" / self.name).iterdir():
            if f.suffix in (".fossil", ".shex"):
                shutil.copy(f, run / f.name)
        subprocess.run([sys.executable, str(BENCH / "workloads/synthetic/generate.py"),
                        self.name, str(rows), str(run), "--seed", str(a.seed)], check=True)
        return (BENCH / "workloads/synthetic/rml" / f"{self.name}.rml.ttl").read_text("utf-8")


class Gtfs:
    """GTFS-Madrid-Bench's CSV distribution at scale factor `rows`, from its own generator
    (`gtfs/generate.sh`, Docker); the mapping is the upstream one cut to what fossil's program
    defines (`gtfs/fragment.py`), in legacy RML (`http://semweb.mmlab.be/ns/rml#`)."""
    name = "gtfs"
    program = "gtfs.fossil"
    rml = "legacy"

    def lay_out(self, run, scale, a):
        here = BENCH / "workloads/gtfs"
        for f in ("gtfs.fossil", "gtfs.shex"):
            shutil.copy(here / f, run / f)
        subprocess.run(["bash", str(here / "generate.sh"), str(scale), str(run / "data")],
                       check=True)
        return subprocess.run([a.python, str(here / "fragment.py"), str(here / "upstream.rml.ttl")],
                              check=True, capture_output=True, text=True).stdout


WORKLOADS = {w.name: w for w in [Synthetic("hello"), Synthetic("reviews"), Synthetic("shop"),
                                 Synthetic("relationship"), Gtfs()]}


# ─────────────────────────────────────────────────────────────── engines

def engines(w, a):
    """name → (command, what it writes), run in the workload's directory."""
    tools = a.tools
    return {
        "fossil": ([str(a.fossil), ".", w.program, "out/fossil"], "out/fossil"),
        "fossil-node": (["node", str(BENCH / "engines/fossil-node.mjs"), ".", w.program,
                         "out/fossil-node"], "out/fossil-node"),
        "rmlmapper": (["java", f"-Xmx{a.xmx}", "-jar", str(tools / a.rmlmapper),
                       "-m", "mapping.rml.ttl", "-s", "nquads", "-o", "out/rmlmapper.nt"],
                      "out/rmlmapper.nt"),
        "morph-kgc": ([a.python, "-m", "morph_kgc", "morph.ini"], "out/morph-kgc.nt"),
        "morph-kgc-1p": ([a.python, "-m", "morph_kgc", "morph-1p.ini"], "out/morph-kgc-1p.nt"),
        "sdm-rdfizer": ([a.python, "-m", "rdfizer", "-c", "sdm.ini"], "out/sdm-rdfizer.nt"),
    }


def configure(run, w, mapping):
    """The mapping, and each Python processor's configuration, beside the data."""
    (run / "mapping.rml.ttl").write_text(mapping, encoding="utf-8")
    (run / "morph.rml.ttl").write_text(rewrite(mapping), encoding="utf-8")
    for name, extra in (("morph-kgc", ""), ("morph-kgc-1p", "number_of_processes=1\n")):
        ini = "morph" if not extra else "morph-1p"
        (run / f"{ini}.ini").write_text(
            f"[CONFIGURATION]\noutput_file=out/{name}.nt\noutput_format=N-TRIPLES\n{extra}"
            f"\n[DataSource]\nmappings=morph.rml.ttl\n", encoding="utf-8")
    (run / "sdm.ini").write_text(
        "[default]\nmain_directory: .\n\n[datasets]\nnumber_of_datasets: 1\n"
        "output_folder: out\nall_in_one_file: no\nremove_duplicate: yes\nenrichment: yes\n"
        "name: sdm\nordered: no\nmapping_partitions: no\noutput_format: n-triples\n"
        f"new_formulation: {'yes' if w.rml == 'core' else 'no'}\n\n"
        "[dataset1]\nname: sdm-rdfizer\nmapping: mapping.rml.ttl\n", encoding="utf-8")


def lay_out(w, n, a):
    run = a.work / w.name / str(n)
    if (run / ".ready").exists():
        return run
    if run.exists():
        shutil.rmtree(run)
    run.mkdir(parents=True)
    configure(run, w, w.lay_out(run, n, a))
    (run / ".ready").touch()
    return run


# ─────────────────────────────────────────────────────────────── measuring

def tree_rss_kb(root):
    """The summed VmRSS of `root` and every descendant, from /proc."""
    children = {}
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        try:
            with open(f"/proc/{entry}/stat") as f:
                stat = f.read()
            ppid = int(stat[stat.rindex(")") + 2:].split()[1])
            children.setdefault(ppid, []).append(int(entry))
        except (OSError, ValueError):
            pass
    total, stack = 0, [root]
    while stack:
        pid = stack.pop()
        stack.extend(children.get(pid, []))
        try:
            with open(f"/proc/{pid}/status") as f:
                for line in f:
                    if line.startswith("VmRSS:"):
                        total += int(line.split()[1])
        except OSError:
            pass
    return total


def measure(cmd, cwd, timeout):
    """Run `cmd`, answering wall and CPU time, peak RSS, exit status and output."""
    started = time.perf_counter()
    proc = subprocess.Popen(cmd, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            text=True, start_new_session=True)
    peak, done = [0], threading.Event()

    def sample():
        while not done.wait(0.02):
            peak[0] = max(peak[0], tree_rss_kb(proc.pid))

    threading.Thread(target=sample, daemon=True).start()
    out, err = [], []
    readers = [threading.Thread(target=lambda s=s, b=b: b.append(s.read()), daemon=True)
               for s, b in ((proc.stdout, out), (proc.stderr, err))]
    for r in readers:
        r.start()
    timer = threading.Timer(timeout, lambda: os.killpg(proc.pid, 9))
    timer.start()
    _, status, usage = os.wait4(proc.pid, 0)
    elapsed = time.perf_counter() - started
    timer.cancel()
    done.set()
    for r in readers:
        r.join()
    return {
        "wall_s": round(elapsed, 3),
        "cpu_s": round(usage.ru_utime + usage.ru_stime, 3),
        "rss_mb": round(usage.ru_maxrss / 1024, 1),
        "tree_rss_mb": round(max(peak[0], usage.ru_maxrss) / 1024, 1),
        "exit": os.waitstatus_to_exitcode(status),
        "timed_out": elapsed >= timeout,
        "stdout": "".join(out),
        "stderr": "".join(err)[-4000:],
    }


def size_of(path):
    if not path.exists():
        return ""
    if path.is_file():
        return path.stat().st_size
    return sum(p.stat().st_size for p in path.rglob("*") if p.is_file())


# ─────────────────────────────────────────────────────────────── comparing

def triples(path):
    """The set of N-Triples lines in `path`: one space before the final dot, a simple literal
    as `xsd:string`."""
    out = set()
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = re.sub(r"\s*\.\s*$", " .", line.strip())
            if line == " ." or not line:
                continue
            if line.endswith('" .'):
                line = line[:-2] + "^^" + XSD_STRING + " ."
            out.add(line)
    return out


def values(lines):
    """`lines` compared as values: IRIs percent-decoded, numbers by their number, booleans
    by their truth (`"0"` and `"false"` are one `xsd:boolean`)."""
    def one(line):
        line = IRI.sub(lambda m: "<" + urllib.parse.unquote(m.group(1)) + ">", line)
        m = NUMERIC.search(line)
        if m:
            line = line[:m.start()] + f"{float(m.group(1))!r}^^{m.group(2)} ."
        m = BOOLEAN.search(line)
        if m:
            line = line[:m.start()] + f"{m.group(1) in ('1', 'true')}^^boolean ."
        return line
    return {one(line) for line in lines}


def verify(run, w, n, a, keep, vlog):
    """Compare every N-Triples file in `keep` with fossil's."""
    ran = measure([str(a.fossil), ".", w.program, "out/fossil", "--triples-sql", "verify/fossil.sql"],
                  run, a.timeout)
    if ran["exit"] != 0:
        return
    subprocess.run([a.python, str(BENCH / "readback.py"), "verify/fossil.sql", "verify/fossil.nt"],
                   cwd=run, check=True)
    mine = triples(keep / "fossil.nt")
    vlog.writerow(dict(workload=w.name, rows=n, engine="fossil", triples=len(mine)))
    for theirs_path in sorted(keep.glob("*.nt")):
        if theirs_path.stem == "fossil":
            continue
        theirs = triples(theirs_path)
        only_f, only_e = mine - theirs, theirs - mine
        vlog.writerow(dict(
            workload=w.name, rows=n, engine=theirs_path.stem, triples=len(theirs),
            same_terms=not only_f and not only_e, same_values=values(mine) == values(theirs),
            only_fossil=len(only_f), only_engine=len(only_e),
            example=" | ".join(sorted(only_f)[:1] + sorted(only_e)[:1])))


# ─────────────────────────────────────────────────────────────── the run

def versions(a):
    def first(cmd):
        try:
            r = subprocess.run(cmd, capture_output=True, text=True, timeout=60)
            lines = (r.stdout + r.stderr).strip().splitlines()
            return next((l for l in lines if "JAVA_TOOL_OPTIONS" not in l), "")
        except OSError as e:
            return str(e)

    def pip(name):
        return first([a.python, "-c", f"import importlib.metadata as m; print(m.version('{name}'))"])

    return {
        "date": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        "machine": platform.platform(),
        "cpus": os.cpu_count(),
        "memory_gb": round(os.sysconf("SC_PAGE_SIZE") * os.sysconf("SC_PHYS_PAGES") / 2**30, 1),
        "fossil": first(["git", "-C", str(REPO), "rev-parse", "--short", "HEAD"]),
        "node": first(["node", "--version"]),
        "rmlmapper": a.rmlmapper,
        "java": first(["java", "-version"]),
        "java_xmx": a.xmx,
        "morph_kgc": pip("morph-kgc"),
        "sdm_rdfizer": pip("rdfizer"),
        "python": first([a.python, "--version"]),
        "repeat": a.repeat,
        "timeout_s": a.timeout,
        "seed": a.seed,
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--workloads", default="hello,reviews,shop,relationship")
    ap.add_argument("--sizes", default="1000,10000,100000,1000000",
                    help="rows of the main source; for gtfs, its scale factors")
    ap.add_argument("--engines", default="fossil,rmlmapper,morph-kgc,morph-kgc-1p,sdm-rdfizer")
    ap.add_argument("--repeat", type=int, default=3)
    ap.add_argument("--timeout", type=float, default=1800)
    ap.add_argument("--verify-max", type=int, default=100000)
    ap.add_argument("--seed", type=int, default=20261019)
    ap.add_argument("--work", type=Path, default=BENCH / ".work")
    ap.add_argument("--out", type=Path, default=BENCH / "results")
    ap.add_argument("--tools", type=Path, required=True,
                    help="where `setup.sh` put the RMLMapper jar and the Python environment")
    ap.add_argument("--fossil", type=Path, default=REPO / "target/release/fossil-bench")
    ap.add_argument("--rmlmapper", default="rmlmapper-8.1.0-r380-all.jar")
    ap.add_argument("--xmx", default="8g")
    a = ap.parse_args()
    a.tools = a.tools.resolve()
    a.fossil = a.fossil.resolve()
    a.python = str(a.tools / "venv/bin/python")
    a.work = a.work.resolve()

    a.out.mkdir(parents=True, exist_ok=True)
    (a.out / "env.json").write_text(json.dumps(versions(a), indent=2) + "\n")
    fields = ["workload", "rows", "input_bytes", "engine", "rep", "exit", "timed_out", "wall_s",
              "cpu_s", "rss_mb", "tree_rss_mb", "out_bytes", "serve_ms", "run_ms", "write_ms",
              "wasm_memory_bytes"]
    runs = open(a.out / "runs.csv", "w", newline="")
    log = csv.DictWriter(runs, fields, extrasaction="ignore")
    log.writeheader()
    checks = open(a.out / "verify.csv", "w", newline="")
    vlog = csv.DictWriter(checks, ["workload", "rows", "engine", "triples", "same_terms",
                                   "same_values", "only_fossil", "only_engine", "example"])
    vlog.writeheader()
    wanted = a.engines.split(",")

    for w in (WORKLOADS[name] for name in a.workloads.split(",")):
        for n in map(int, a.sizes.split(",")):
            run = lay_out(w, n, a)
            input_bytes = size_of(run / "data")
            keep = run / "verify"
            shutil.rmtree(keep, ignore_errors=True)
            keep.mkdir()
            failed = set()
            for rep in range(1, a.repeat + 1):
                for engine, (cmd, out) in engines(w, a).items():
                    if engine not in wanted or engine in failed:
                        continue
                    shutil.rmtree(run / "out", ignore_errors=True)
                    (run / "out").mkdir()
                    r = measure(cmd, run, a.timeout)
                    r.update(workload=w.name, rows=n, input_bytes=input_bytes, engine=engine,
                             rep=rep, out_bytes=size_of(run / out))
                    if engine.startswith("fossil") and r["exit"] == 0:
                        r.update(json.loads(r["stdout"].strip().splitlines()[-1]))
                    log.writerow(r)
                    runs.flush()
                    status = "ok" if r["exit"] == 0 else (
                        "TIMEOUT" if r["timed_out"] else f"FAILED ({r['exit']})")
                    print(f"{w.name:13} {n:>8} {engine:13} rep {rep}: {r['wall_s']:8.2f}s "
                          f"{r['tree_rss_mb']:8.1f} MB  {status}", flush=True)
                    if r["exit"] != 0:
                        # A failure or a timeout is a result, and repeating it buys nothing.
                        failed.add(engine)
                        (a.out / f"{w.name}-{n}-{engine}.stderr").write_text(r["stderr"])
                    elif rep == a.repeat and n <= a.verify_max and out.endswith(".nt"):
                        shutil.copy(run / out, keep / f"{engine}.nt")
            if n <= a.verify_max and "fossil" in wanted:
                verify(run, w, n, a, keep, vlog)
                checks.flush()
            shutil.rmtree(keep, ignore_errors=True)
            shutil.rmtree(run / "out", ignore_errors=True)


if __name__ == "__main__":
    main()
