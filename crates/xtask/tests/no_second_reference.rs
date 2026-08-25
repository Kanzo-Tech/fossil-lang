//! The repository root holds no prose but its own front door.
//!
//! This repository has deleted a second reference twice. First `decisions/`, sixty-three numbered
//! records that produced 159 dead citations and eighteen of twenty `ADR-0050` references resolving
//! to the wrong record. Then `SURFACE-PLAN.md`, 1,443 lines, which diagnosed itself on its own line
//! 5 — *un plan se convierte en segunda referencia el día que el trabajo aterriza y el plan sigue
//! describiéndolo. Ha pasado* — and was cited 89 times from the code by the time it went.
//!
//! Both were deleted by hand, and nothing stopped a third. The failure mode does not announce
//! itself: a plan is genuinely useful on the day it is written, and it becomes a second reference
//! silently, on the day the work lands and the document keeps describing it. By then it is load
//! bearing enough that deleting it is a project.
//!
//! So the rule is a test rather than a habit: documentation lives in `apps/docs`, and the root
//! carries only what a reader meets before the site — the front door, the contributor's guide, the
//! agent instructions, the licence.
//!
//! WHAT THIS DOES NOT PROVE. It checks a filename, not a genre. A second reference filed as
//! `apps/docs/content/docs/design/plan.mdx` passes here and is the same mistake — what stops that
//! one is `design/discarded`'s admission rule, which is prose. And it says nothing about the four
//! permitted files themselves: `CONTRIBUTING.md` could grow a design chapter and this would not
//! notice. It closes the one door that has been walked through twice.

use std::path::{Path, PathBuf};
use std::process::Command;

/// What a reader meets before the site, and nothing else.
const PERMITTED: &[&str] = &["README.md", "CLAUDE.md", "CONTRIBUTING.md"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("xtask sits two levels below the root")
        .to_path_buf()
}

/// Untracked and ignored files are somebody's local scratch, not a reference anyone else can read.
fn is_ignored(root: &Path, name: &str) -> bool {
    Command::new("git")
        .args(["check-ignore", "-q", name])
        .current_dir(root)
        .status()
        .is_ok_and(|s| s.success())
}

#[test]
fn the_root_holds_no_prose_but_its_own_front_door() {
    let root = repo_root();

    let mut strays: Vec<String> = std::fs::read_dir(&root)
        .expect("the repository root is readable")
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            let is_prose = name.ends_with(".md") && !name.starts_with("LICENSE");
            let allowed = PERMITTED.contains(&name.as_str());
            (is_prose && !allowed && !is_ignored(&root, &name)).then_some(name)
        })
        .collect();
    strays.sort();

    assert!(
        strays.is_empty(),
        "the repository root has grown prose that is not its front door: {strays:?}\n\
         Documentation belongs in apps/docs. A plan belongs in the commit that executes it.\n\
         This repository has deleted a second reference from the root twice; the second time it \
         had been cited 89 times from the code and the deletion took a day.",
    );
}

/// A guard over an empty set is a guard that has stopped guarding, and this one is one `read_dir`
/// away from that: a wrong root, a permissions error swallowed, and the assertion above passes over
/// nothing forever. So it is checked that the directory being read is the one with the workspace in
/// it, and that the permitted files are actually there rather than merely permitted.
#[test]
fn the_guard_is_reading_the_repository_root() {
    let root = repo_root();

    assert!(
        root.join("Cargo.toml").is_file() && root.join("grammar.bnf").is_file(),
        "{} is not the workspace root, so the check above is reading the wrong directory",
        root.display(),
    );

    for name in PERMITTED {
        assert!(
            root.join(name).is_file(),
            "{name} is permitted at the root and is not there — either it moved and this list is \
             stale, or the root is wrong",
        );
    }
}
