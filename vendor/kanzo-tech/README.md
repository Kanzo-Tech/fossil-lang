# vendor/kanzo-tech — two tarballs, and why they are in git

`@kanzo-tech/ui` is the design system fossil's app dresses in. **It has never been
published.** Its packages sit at `version: 0.0.0`, `release.yml` would publish them
through changesets and Trusted Publishing, and waiting for that is not the plan.

These two files are `pnpm pack` output, committed. That is a real cost and it is
the only option that survives someone else cloning this repository.

## What is here

| file | from | bytes |
|---|---|---|
| `kanzo-tech-theme-0.0.0.tgz` | `kanzo-ui/packages/theme` | 66 013 |
| `kanzo-tech-ui-0.0.0.tgz` | `kanzo-ui/packages/ui` | 556 934 |

Both packed from `kanzo-ui` at **`d4c4b15696cdbf7e79d933eda7c3d1d9111da20f`**
(`feat(ui): a tenant's default theme can be a side per side`, 2026-08-27), working
tree clean.

`refresh.sh` is what made them, and running it is how they change. It writes the
source SHA back into this table, so a stale blob is visible rather than assumed.

## Why the three obvious routes do not work

Each was tried against the actual manifests rather than assumed.

**A pnpm workspace spanning both repositories.** `pnpm-workspace.yaml` globs are
resolved under the workspace root; even where a `../kanzo-ui/packages/*` entry
resolves on this machine, a fresh clone has no sibling `kanzo-ui` and `pnpm
install` fails at resolution with nothing to point at. The failure is not even
about `@kanzo-tech/ui` — it is a glob that matched nothing.

**`pnpm link`.** Machine-local by construction. A clone gets an unresolvable
specifier and no hint that a second checkout was ever involved.

**`file:` or a git URL at `kanzo-ui/packages/ui`.** Two independent stoppers.
`dist/` is in kanzo-ui's `.gitignore` and there is no `prepare`, `prepack` or
`prepublishOnly` script, so nothing builds on install and a git URL fetches a
package whose every `exports` path is missing. And `@kanzo-tech/ui` depends on
`"@kanzo-tech/theme": "workspace:*"` — a protocol pnpm only resolves inside the
workspace that owns it, so the install fails even with `dist/` present.

## Why packing works where `file:` does not

`pnpm pack` — **not `npm pack`** — rewrites `workspace:*` into a concrete version
on the way into the tarball. kanzo-ui's own `scripts/smoke-install.mjs` says so at
lines 101-104: *"only pnpm rewrites the `workspace:*` dependency on
@kanzo-tech/theme into a real version. An npm-packed tarball cannot be
installed."*

What comes out declares `"@kanzo-tech/theme": "0.0.0"`, which is a version npm has
never heard of. The root `package.json` closes that with a `pnpm.overrides` entry
pointing `@kanzo-tech/theme` at the sibling tarball here. Both halves are required:
the override alone has nothing to override, the tarball alone resolves to a 404.

## Refreshing

```bash
vendor/kanzo-tech/refresh.sh                       # default sibling checkout
KANZO_UI=/path/to/kanzo-ui vendor/kanzo-tech/refresh.sh
```

It builds kanzo-ui first (`dist/` is gitignored there, so it may not exist), packs
both packages here, and rewrites the SHA in this file. Then `pnpm install`.

**Nothing in this repository can do that for you**, and that is the honest state
of a cross-repo dependency on an unpublished package: the tarballs are a snapshot,
they go stale silently, and the only signal that they have is someone reading the
SHA above against kanzo-ui's log. When `@kanzo-tech/ui` reaches npm, this directory
and the override both go, and the dependency becomes a version range like any
other.
