#!/bin/sh
# ./run.sh <dir-del-corpus> [k]   — ver la cabecera de lca-vs-csr.sql
set -eu
corpus=${1:?uso: ./run.sh <dir-del-corpus> [k]}
k=${2:-20000}
here=$(cd "$(dirname "$0")" && pwd)
printf "SET VARIABLE corpus = '%s';\nSET VARIABLE k = %s;\n.read %s/lca-vs-csr.sql\n" \
  "$corpus" "$k" "$here" | duckdb
