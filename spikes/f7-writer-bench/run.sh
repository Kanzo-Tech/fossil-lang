#!/usr/bin/env bash
# F7's missing measurement: 5M rows written both ways, footers/requests/bytes/pages counted.
#
#   ./run.sh [rows] [draw|full] [outdir]
#
# Release only — 5M rows in a debug build measures the absence of inlining.
# Needs the `duckdb` CLI on PATH for the DuckDB half; without it the arrow-rs
# half still runs and the report says the DuckDB rows are absent.
set -euo pipefail
cd "$(dirname "$0")"

ROWS="${1:-5000000}"
SCHEMA="${2:-draw}"
DIR="${3:-/tmp/f7-writer-bench}"

cargo run --release -- --rows "$ROWS" --schema "$SCHEMA" --dir "$DIR"
