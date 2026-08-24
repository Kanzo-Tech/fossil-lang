# The fossil corpus checker

Executable checks that a corpus on disk satisfies the conventions of the format. Run it against your
own output to prove it is readable.

```bash
node check.mjs /path/to/corpus     # check a corpus
node check.mjs --explain           # what each guard proves, and what it cannot
node self-test.mjs                 # prove the guards still fire
node fixture.mjs /tmp/corpus       # write a conforming corpus to look at
```

Exit `0` when every convention holds, `1` when one does not, `2` when the checker could not run.

## Requirements

`node` (≥ 20) and the [`duckdb`](https://duckdb.org/docs/installation) binary on `PATH`. That is
all. There is no `package.json` here, no install step, no build, and no dependency on fossil, on
Rust or on the repository this directory came from.

**This directory is meant to be copied.** If you write corpora and want the contract in executable
form, take it.

## What is here

| file | |
| --- | --- |
| `arithmetic.mjs` | the addressing and Morton arithmetic, as a second implementation. No imports. |
| `vectors.json` | the published test vectors. The deliverable — this is what gets copied. |
| `manifest.mjs` | the manifest, read by line scan rather than through the struct that wrote it |
| `inspect.mjs` | what is on disk, before any guard has an opinion about it |
| `guards.mjs` | the conventions, each with what it proves and what it cannot. No count here: `check.mjs` prints one and this line had already been wrong |
| `check.mjs` | the CLI |
| `duck.mjs` | the only thing between the guards and the corpus: `duckdb` over stdin |
| `fixture.mjs` | writes a conforming corpus in JavaScript, from the conventions alone |
| `self-test.mjs` | non-vacuity, plus one mutation per guard |

## Why a checker rather than a library

The corpus is documented, not typed. A shared type would put a compiler between the writer and the
reader — and a version, and a language. The corpus is already read from three languages that do not
know fossil exists, and the format's whole claim is that a fourth needs no permission.

The price is stated where it is paid: **there is no type, so a guard checks what it was asked to
check and nothing more.** Every guard therefore carries a `cannotProve` field, `--explain` prints
it, and a failure prints it beside the failure. Read that half.

Full documentation of the conventions: `apps/corpus/content/docs/`.
