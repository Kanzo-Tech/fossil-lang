# Email draft: rudof WASM compatibility — to José Emilio Labra Gayo

**Recipient:** José Emilio Labra Gayo <labra@uniovi.es>
**Alt contact:** rudof GitHub (https://github.com/rudof-project/rudof) or his home page (https://labra.weso.es/)
**Sent date:** <YYYY-MM-DD or "DRAFT — not yet sent">
**Reply received:** <YYYY-MM-DD or "awaiting" or "no reply">

---

**Subject:** `Fossil compiler — rudof crates on wasm32-unknown-unknown (spike PASSED, opening collab)`

**Body:**

```
Hola José Emilio,

I'm Angel Iglesias (Kanzo). I'm starting Fossil — a typed compiler for RDF
graph construction targeting both native CLI and a public web playground
(à la RDFShape, in the same lineage your team has discussed). Apache-2.0
OSS, ~16-week aspirational ship for v0.1.

Fossil's output-descriptor pillar is ShEx-as-target-type, with rudof as the
real dependency (no fork, no reimplementation). The single highest-risk
external decision in my Phase 0 was whether the rudof crates compile to
wasm32-unknown-unknown — the playground depends on it.

**Good news first:** I just ran the spike against rudof commit d024a53b
(workspace v0.3.1). All four crates I plan to depend on — shex_ast,
shex_validation, rudof_iri, prefixmap — compile cleanly to
wasm32-unknown-unknown with no feature flag changes. No tokio, no reqwest
in the build path. The `getrandom = { features = ["wasm_js"] }` pin in
your workspace Cargo.toml clearly did its job. Thank you.

Two questions to open the collab properly:

1. Are there known rudof sub-crates that DON'T compile to wasm32 today
   (so I can avoid them by design)? I checked the four above; happy to
   know if `srdf`, `shacl`, etc. have known issues.

2. If a future rudof release breaks WASM compat, what's your team's
   preferred channel for me to flag it? GitHub issue, email, the WESO
   Slack/Discord if there is one? I'd rather catch regressions on your
   side before pinning to last-known-good.

Bonus context: Fossil cites the Min Oo & Hartig operator algebra (ESWC
2025) as foundation, which puts it in the same intellectual neighborhood
as your work on shape semantics. I'd love to share the project's planning
docs (PROJECT.md, REQUIREMENTS.md, ROADMAP.md) if useful — happy to set
up a 30-minute call when v0.1 ships, ahead of any KGCW or W3C
kg-construct mailing list announcement.

Gracias por adelantado,
Angel
angel.iglesias@kanzo.tech
github.com/<org> (org pending — see .planning/STATE.md open todos)
```

---

## Notes for the user

This email is a **draft**. Claude cannot send email; the user must send manually
from their preferred client (suggested: standard email or via GitHub if Labra
Gayo prefers issue-based contact).

The original draft (per RESEARCH.md) was framed as "asking if rudof works for WASM."
This revised version reflects the spike result: rudof works. The email now opens
the relationship from a position of "spike passed, here's what I found, opening
collab" rather than "please help me figure this out." Better posture; same goal.

When sent:
1. Update `Sent date:` field above to today's YYYY-MM-DD.
2. Note any subject-line tweaks the user makes.
3. Replace `github.com/<org>` placeholder if the org name is locked by send time.

When a reply is received:
1. Update `Reply received:` field above to YYYY-MM-DD.
2. If the reply changes any future-relevant fact (known-broken sub-crate,
   WASM regression on an unreleased branch), open a new commit updating
   `decisions/rudof-wasm.md` with a "Reply from Labra Gayo (DATE)" section.
3. If the reply leads to a collab PR, link it from `decisions/rudof-wasm.md`
   action items.

## Why this email matters

Per RESEARCH.md: "A reply may save 3 weeks; the spike runs in parallel."

Even though the spike passed (path (a) Clean WASM), the email opens a
relationship that pays off in Phase 3 (rudof integration), Phase 9 (launch
needs WESO-group amplification on the W3C kg-construct mailing list), and
forward (regression-detection alliance with upstream).
