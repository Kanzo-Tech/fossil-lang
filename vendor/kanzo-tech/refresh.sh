#!/usr/bin/env bash
#
# Re-pack @kanzo-tech/{theme,ui} from a kanzo-ui checkout into this directory.
#
# `pnpm pack`, NEVER `npm pack`: only pnpm rewrites the `workspace:*` dependency
# @kanzo-tech/ui declares on @kanzo-tech/theme into a concrete version. An
# npm-packed tarball carries `workspace:*` verbatim and cannot be installed
# anywhere. kanzo-ui's own scripts/smoke-install.mjs says the same thing.
#
# `pnpm build` first because kanzo-ui gitignores dist/ and has no prepare,
# prepack or prepublishOnly script — a fresh clone of it has nothing to pack,
# and `pnpm pack` will happily produce a tarball whose every exports path is
# missing rather than telling you.
set -euo pipefail

KANZO_UI="${KANZO_UI:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)/kanzo-ui}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [ ! -f "$KANZO_UI/pnpm-workspace.yaml" ]; then
  echo "no kanzo-ui checkout at $KANZO_UI" >&2
  echo "set KANZO_UI=/path/to/kanzo-ui and run this again" >&2
  exit 1
fi

echo "==> building $KANZO_UI (topological: ui needs theme)"
(cd "$KANZO_UI" && pnpm install --frozen-lockfile && pnpm -r --filter "./packages/*" build)

for pkg in theme ui; do
  echo "==> packing @kanzo-tech/$pkg"
  (cd "$KANZO_UI/packages/$pkg" && pnpm pack --pack-destination "$HERE")
done

SHA="$(cd "$KANZO_UI" && git rev-parse HEAD)"
SUBJECT="$(cd "$KANZO_UI" && git log -1 --format=%s)"
DATE="$(cd "$KANZO_UI" && git log -1 --format=%cs)"
DIRTY=""
if [ -n "$(cd "$KANZO_UI" && git status --porcelain)" ]; then
  DIRTY=", **working tree DIRTY — this snapshot is not reproducible from that SHA**"
fi

THEME_BYTES="$(wc -c <"$HERE/kanzo-tech-theme-0.0.0.tgz" | tr -d ' ')"
UI_BYTES="$(wc -c <"$HERE/kanzo-tech-ui-0.0.0.tgz" | tr -d ' ')"

# Rewrite the provenance block in README.md. The sizes and the SHA are the only
# things anyone can check a stale blob against, so they are generated, not typed.
python3 - "$HERE/kanzo-tech-theme-0.0.0.tgz" <<PY
import re, sys
p = "$HERE/README.md"
s = open(p).read()
s = re.sub(
    r"\| \`kanzo-tech-theme-0\.0\.0\.tgz\` \| \`kanzo-ui/packages/theme\` \| [0-9 ]+ \|",
    "| \`kanzo-tech-theme-0.0.0.tgz\` | \`kanzo-ui/packages/theme\` | $THEME_BYTES |",
    s,
)
s = re.sub(
    r"\| \`kanzo-tech-ui-0\.0\.0\.tgz\` \| \`kanzo-ui/packages/ui\` \| [0-9 ]+ \|",
    "| \`kanzo-tech-ui-0.0.0.tgz\` | \`kanzo-ui/packages/ui\` | $UI_BYTES |",
    s,
)
s = re.sub(
    r"Both packed from \`kanzo-ui\` at \*\*\`[0-9a-f]{40}\`\*\*\n\(.*?\)[^\n]*\n",
    "Both packed from \`kanzo-ui\` at **\`$SHA\`**\n(\`$SUBJECT\`, $DATE)$DIRTY.\n",
    s,
    flags=re.S,
)
open(p, "w").write(s)
PY

echo
echo "==> packed from kanzo-ui $SHA"
echo "    now run: pnpm install"
