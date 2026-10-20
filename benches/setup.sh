#!/usr/bin/env bash
# The engines the benchmark compares fossil with, pinned, into $1 (default: benches/.tools).
#
#   bash benches/setup.sh [dir]
#
# Needs Java 17+, Python 3.10+, unixODBC (SDM-RDFizer imports pyodbc whether or not a database
# is read) and, for GTFS-Madrid-Bench, Docker. fossil itself is built from this checkout:
#   cargo build --release --manifest-path benches/fossil-driver/Cargo.toml
# and, for the Node host, `pnpm --filter @fossil-lang/executor build`.
set -euo pipefail
dir=${1:-$(dirname "$0")/.tools}
mkdir -p "$dir"
cd "$dir"

jar=rmlmapper-8.1.0-r380-all.jar
if [ ! -f "$jar" ]; then
  curl -fsSLO "https://repo1.maven.org/maven2/be/ugent/rml/rmlmapper/8.1.0/$jar"
  echo "c7db38929fbbb9056584964095cf0053905d00a5  $jar" | sha1sum -c -
fi

if [ ! -d venv ]; then
  python3 -m venv venv
fi
venv/bin/pip install -q morph-kgc==2.10.0 rdfizer==4.7.5.15.4.1
