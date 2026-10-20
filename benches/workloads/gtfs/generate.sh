#!/usr/bin/env bash
# GTFS-Madrid-Bench's CSV files at scale factor $1, into $2, from the benchmark's own generator.
#
# The generator is VIG over the Madrid metro feed, shipped as the Docker image
# `oegdataintegration/gtfs-bench`; its interactive front end (`composer/app.py`) is skipped and
# its `generation/generate.sh` called directly, which is what the front end runs. VIG's output is
# deterministic in the scale factor.
set -euo pipefail
scale=${1:?scale factor}
out=${2:?output directory}
image=oegdataintegration/gtfs-bench@sha256:c080b0a46cfcba46b9730cc3369a409240d8937b7c91f6d7ecca3136bc694772
mkdir -p "$out"
docker run --rm -v "$(cd "$out" && pwd)":/out --entrypoint bash "$image" -c "
  service mysql start > /dev/null 2>&1
  cd generation && ./generate.sh $scale > /dev/null && cp resources/csvs/*.csv /out/
  chown -R $(id -u):$(id -g) /out"
