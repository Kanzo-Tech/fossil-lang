//! **The one rule that turns a reference a program writes into a locator a
//! reader can open**, and the anchor it needs to do it.
//!
//! # There were three of these, and they disagreed
//!
//! A `.fossil` program names files: `io.shex("shop.shex")`, `io.csv("data/items.csv")`,
//! `io.rdf("@warehouse/people.ttl")`. Until this module there were three
//! separate answers to "relative to what":
//!
//! | what resolved | against what | where |
//! |---|---|---|
//! | `io.shex("shop.shex")` | the program's directory | `fossil_engine::documents::registry_key`, mirroring `fossil_hir`'s own resolution (both are `fossil_hir::documents::registry_key` now) |
//! | provider sources, `@conn` | the program's directory, **falling back to the cwd** | `fossil_engine::resolve_ref` |
//! | `io.csv("data/items.csv")` under `run` | the process's cwd, and only that | the `DataFusion` executor, which was handed the written path verbatim |
//!
//! None of the three is theoretical. Inside ONE program `io.shex("x.shex")` and
//! `io.csv("data/x.csv")` resolved against different directories. `check`
//! pre-introspected against `path.parent()` while `run` executed against the
//! cwd, so the two commands disagreed about where the same file was. And a
//! program changed meaning when you `cd`: `fossil run
//! apps/docs/programs/hello/hello.fossil` from the repository root looked for
//! `<root>/data/people.csv`.
//!
//! # The rule
//!
//! **The program's directory wins.** A `.fossil` file is a document whose paths
//! were written by somebody looking at the folder it lives in. Resolving
//! against the cwd makes a program's meaning depend on where you invoked it
//! from, which is the opposite of what a compiler is for. The cwd is not
//! consulted here at all — not as a fallback, not as a last resort.
//!
//! Three references are NOT relative paths and never touch the anchoring, in
//! this order:
//!
//! 1. `@conn/path` — an alias for a host, expanded through the connection map
//!    into that connection's base URL.
//! 2. anything with a scheme (`s3://`, `az://`, `https://`) — already a locator.
//! 3. an absolute path — already a locator.
//!
//! # Why it is its own crate
//!
//! Because no two of its five readers share anything else: the checker
//! (`fossil-hir`, resolving a shape document), the host that registers those
//! documents (`fossil-engine`), the pre-compile introspection
//! (`fossil-introspect`), the executor (`fossil-df`), and the editor
//! (`fossil-lsp`). Anywhere higher and at least one of them would have to keep
//! a copy — which is exactly how three of them came to exist.
//!
//! It was a module of `fossil-base`, and that was the mistake this crate
//! undoes. rust-analyzer's `base-db` states the invariant plainly — it *"doesn't
//! know about file system and file paths"* — and it holds because its VFS is a
//! separate crate and the operating-system implementation a third. A substrate
//! that decides what a written path means is deciding something about a
//! program. This crate decides it; `fossil-base` no longer can.
//!
//! It depends on nothing, which is what lets `fossil-base` depend on **it**
//! instead of the other way round.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The directory a program's relative references are written against: the
/// directory the program itself lives in.
///
/// A program path with no parent (a bare `hello.fossil`, or an in-memory name
/// the browser interned) anchors to the empty path, and `Path::new("").join(x)`
/// is `x` — so a program with no directory resolves its references to
/// themselves, which is the only answer available and the one every host was
/// already giving.
#[must_use]
pub fn program_dir(program_path: &str) -> PathBuf {
    Path::new(program_path)
        .parent()
        .unwrap_or_else(|| Path::new(""))
        .to_path_buf()
}

/// Everything a written reference needs to become a locator: the directory of
/// the program that wrote it, and what its `@conn` aliases name.
///
/// The two travel **together**, in one `Copy` value, and that is the point.
/// They were separate parameters, and half the code paths carried only the
/// connection map — which is not an omission a reader can see, because a path
/// resolved against nothing still *looks* like a path and still opens a file
/// whenever the cwd happens to be right. Carrying them as one value is what
/// makes "resolved without an anchor" fail to compile instead of failing on
/// somebody else's machine.
#[derive(Debug, Clone, Copy)]
pub struct SourceAnchor<'a> {
    program_dir: &'a Path,
    connections: &'a HashMap<String, String>,
}

impl<'a> SourceAnchor<'a> {
    /// An anchor for a program in `program_dir` whose `@conn` aliases resolve
    /// through `connections` (name → base URL).
    #[must_use]
    pub const fn new(program_dir: &'a Path, connections: &'a HashMap<String, String>) -> Self {
        Self {
            program_dir,
            connections,
        }
    }

    /// An anchor for a host that has no connection map — the checker, which
    /// resolves shape documents and is never given credentials.
    ///
    /// An unknown alias passes through verbatim ([`Self::locator`]), so a
    /// program naming `@warehouse/shapes/x.shex` reads here as the literal
    /// `@warehouse/…`, and the "not found" the reader then reports names what
    /// the program wrote. That is the same answer an unknown alias gets from a
    /// host that DOES have a map, which is why there is no third behaviour.
    #[must_use]
    pub fn beside(program_dir: &'a Path) -> Self {
        static NO_CONNECTIONS: OnceLock<HashMap<String, String>> = OnceLock::new();
        Self {
            program_dir,
            connections: NO_CONNECTIONS.get_or_init(HashMap::new),
        }
    }

    /// The directory this anchor resolves against.
    #[must_use]
    pub const fn program_dir(&self) -> &Path {
        self.program_dir
    }

    /// The `@conn` name → base-URL map this anchor expands aliases through.
    #[must_use]
    pub const fn connections(&self) -> &HashMap<String, String> {
        self.connections
    }

    /// **The rule.** Turn `raw` — a reference exactly as the program wrote it —
    /// into a locator a reader can open.
    ///
    /// `@name/path` expands to that connection's base URL; a reference that
    /// already carries a scheme or is absolute is returned untouched; anything
    /// else is a relative path and is joined onto the program's directory. The
    /// process working directory is never consulted.
    #[must_use]
    pub fn locator(&self, raw: &str) -> String {
        let expanded = self.expand_alias(raw);
        // An `@alias` that expanded to nothing is not a relative path and must
        // not be anchored: joining it produces `/programs/shop/@missing/x.csv`,
        // a path naming a directory the program never mentioned, and the "not
        // found" that follows sends the reader looking for the wrong mistake.
        // Left as itself, the message names what the program wrote — which is
        // the connection that was not supplied.
        if expanded.starts_with('@') {
            return expanded;
        }
        // A scheme or an absolute path is already a locator. `contains("://")`
        // and not a list of schemes: `s3`, `az`, `gs`, `http(s)` and whatever
        // the object store learns next are all the same answer, and a list is a
        // place for one of them to be forgotten.
        if expanded.contains("://") || Path::new(&expanded).is_absolute() {
            return expanded;
        }
        self.program_dir
            .join(&expanded)
            .to_string_lossy()
            .into_owned()
    }

    /// `@name/path` → `{connections[name]}/path`; anything else verbatim.
    ///
    /// An unknown alias is left alone rather than diagnosed — the reader that
    /// fails to open `@warehouse/x.csv` says so naming what the program wrote,
    /// which is a better message than one this function could build.
    fn expand_alias(&self, raw: &str) -> String {
        match raw.strip_prefix('@').and_then(|r| r.split_once('/')) {
            Some((name, path)) => self.connections.get(name).map_or_else(
                || raw.to_string(),
                |base| format!("{}/{path}", base.trim_end_matches('/')),
            ),
            None => raw.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conns(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(n, u)| ((*n).to_string(), (*u).to_string()))
            .collect()
    }

    #[test]
    fn a_relative_path_anchors_to_the_program_and_not_the_cwd() {
        let c = conns(&[]);
        let anchor = SourceAnchor::new(Path::new("apps/docs/programs/hello"), &c);
        assert_eq!(
            anchor.locator("data/people.csv"),
            "apps/docs/programs/hello/data/people.csv"
        );
    }

    /// The whole bug in one assertion: the answer does not mention, and cannot
    /// depend on, where the process happens to be standing.
    #[test]
    fn the_answer_does_not_move_when_the_process_does() {
        let c = conns(&[]);
        let anchor = SourceAnchor::new(Path::new("/srv/programs/shop"), &c);
        let first = anchor.locator("data/items.csv");
        assert_eq!(first, "/srv/programs/shop/data/items.csv");
        assert!(
            !first.contains(
                &std::env::current_dir()
                    .expect("cwd")
                    .to_string_lossy()
                    .into_owned()
            )
        );
    }

    #[test]
    fn a_program_with_no_directory_resolves_a_reference_to_itself() {
        assert_eq!(program_dir("hello.fossil"), Path::new(""));
        let c = conns(&[]);
        assert_eq!(
            SourceAnchor::new(&program_dir("hello.fossil"), &c).locator("users.csv"),
            "users.csv"
        );
    }

    #[test]
    fn program_dir_is_the_directory_the_program_lives_in() {
        assert_eq!(
            program_dir("apps/docs/programs/hello/hello.fossil"),
            Path::new("apps/docs/programs/hello")
        );
    }

    /// The three that are not relative paths, and which the anchoring must
    /// leave alone — a cloud URL is not a filename that happens to have colons.
    #[test]
    fn a_url_or_an_absolute_path_is_already_a_locator() {
        let c = conns(&[]);
        let anchor = SourceAnchor::new(Path::new("/programs/shop"), &c);
        assert_eq!(anchor.locator("s3://bucket/x.csv"), "s3://bucket/x.csv");
        assert_eq!(anchor.locator("az://acct/x.csv"), "az://acct/x.csv");
        assert_eq!(
            anchor.locator("https://example.org/x.csv"),
            "https://example.org/x.csv"
        );
        assert_eq!(anchor.locator("/data/x.csv"), "/data/x.csv");
    }

    #[test]
    fn an_alias_expands_to_its_connection_and_is_not_anchored() {
        let c = conns(&[("sales", "s3://bucket/prefix")]);
        let anchor = SourceAnchor::new(Path::new("/programs/shop"), &c);
        assert_eq!(
            anchor.locator("@sales/2024/orders.csv"),
            "s3://bucket/prefix/2024/orders.csv"
        );
    }

    #[test]
    fn collapses_slashes_at_the_join() {
        let c = conns(&[("sales", "s3://bucket/prefix/")]);
        assert_eq!(
            SourceAnchor::new(Path::new("/p"), &c).locator("@sales/x.csv"),
            "s3://bucket/prefix/x.csv"
        );
    }

    /// An unknown alias survives as itself, in both anchors, so the reader's
    /// "not found" names what the program wrote.
    #[test]
    fn an_unknown_alias_passes_through_verbatim() {
        let c = conns(&[("sales", "s3://bucket")]);
        assert_eq!(
            SourceAnchor::new(Path::new("/p"), &c).locator("@missing/x.csv"),
            "@missing/x.csv"
        );
        assert_eq!(
            SourceAnchor::beside(Path::new("/p")).locator("@missing/x.csv"),
            "@missing/x.csv"
        );
    }

    /// A relative path resolves the same whether or not the file is there. The
    /// rule this replaced consulted the filesystem — `if joined.exists()` —
    /// and fell back to the cwd when it did not, which made a missing file
    /// report a path the program never named.
    #[test]
    fn a_missing_file_resolves_where_the_program_said_it_would() {
        let c = conns(&[]);
        assert_eq!(
            SourceAnchor::new(Path::new("/nowhere/at/all"), &c).locator("data/x.csv"),
            "/nowhere/at/all/data/x.csv"
        );
    }
}
