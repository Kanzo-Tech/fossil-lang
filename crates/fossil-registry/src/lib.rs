//! `fossil-registry` — stdlib function registry.
//!
//! Phase 1: 4 hardcoded entries (`io.csv`, `core.iri`, `core.triple`, `core.literal`)
//! sufficient for the `hello.fossil` walking-skeleton demo.
//!
//! Phase 1 deliberately does NOT invoke registry lookup from the parser/HIR for
//! `hello.fossil`: `io.csv` is recognised syntactically and `core.iri` /
//! `core.literal` are implicit (template literal in IRI position → IRI;
//! field ref returning String → Literal automatic). The registry exists so
//! Phase 5's STDL-* tasks have a slot to populate.
//!
//! Phase 5 (STDL-01..07) extends `RegistryEntry` with signature + lowering
//! hints (Plan / Inline / Builtin / UDF) and grows the namespace catalog.

use smol_str::SmolStr;
use std::collections::HashMap;

/// Registry of stdlib functions available to a Fossil program.
///
/// Phase 1 only exposes [`Self::phase1_default`] (4 hardcoded entries) and
/// [`Self::lookup`] (name → entry). Phase 5 expands the surface with mutator
/// methods + cross-namespace iteration.
#[derive(Debug, Clone)]
pub struct FunctionRegistry {
    entries: HashMap<SmolStr, RegistryEntry>,
}

/// A single stdlib function entry. Phase 1 carries only the fully-qualified
/// name + a coarse [`RegistryKind`] tag. Phase 5 adds signature, lowering kind
/// (Plan/Inline/Builtin/UDF), and codegen hints.
#[derive(Debug, Clone)]
pub struct RegistryEntry {
    /// Fully-qualified dotted name (e.g. `"io.csv"`, `"core.iri"`).
    pub name: SmolStr,
    /// Coarse classification used by Phase 1 lowering. Phase 5 supersedes
    /// with a richer signature/kind pair.
    pub kind: RegistryKind,
}

/// Coarse classification of stdlib functions for Phase 1 lowering.
///
/// Phase 5 (STDL-01..07) replaces this with a `LoweringKind`
/// (Plan / Inline / Builtin / UDF) carrying full signature info.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryKind {
    /// `io.csv(path)` → `SourceOp(path, Csv)`.
    SourceConstructor,
    /// `core.iri(s)` → identity at codegen (`s` is already a `String → Iri` cast).
    CoreIri,
    /// `core.triple(s, p, o)` → composite `TripleEmit`.
    CoreTriple,
    /// `core.literal(v)` → identity at codegen.
    CoreLiteral,
}

impl FunctionRegistry {
    /// Construct the Phase 1 default registry with 4 hardcoded entries.
    ///
    /// Entries: `io.csv`, `core.iri`, `core.triple`, `core.literal`.
    /// Phase 5 STDL-01..07 expands the catalog.
    #[must_use]
    pub fn phase1_default() -> Self {
        let mut entries = HashMap::new();
        entries.insert(
            "io.csv".into(),
            RegistryEntry {
                name: "io.csv".into(),
                kind: RegistryKind::SourceConstructor,
            },
        );
        entries.insert(
            "core.iri".into(),
            RegistryEntry {
                name: "core.iri".into(),
                kind: RegistryKind::CoreIri,
            },
        );
        entries.insert(
            "core.triple".into(),
            RegistryEntry {
                name: "core.triple".into(),
                kind: RegistryKind::CoreTriple,
            },
        );
        entries.insert(
            "core.literal".into(),
            RegistryEntry {
                name: "core.literal".into(),
                kind: RegistryKind::CoreLiteral,
            },
        );
        Self { entries }
    }

    /// Lookup a function entry by fully-qualified dotted name.
    /// Returns `None` if the name is unknown.
    #[must_use]
    pub fn lookup(&self, name: &str) -> Option<&RegistryEntry> {
        self.entries.get(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase1_default_has_four_entries() {
        let r = FunctionRegistry::phase1_default();
        assert!(r.lookup("io.csv").is_some());
        assert!(r.lookup("core.iri").is_some());
        assert!(r.lookup("core.triple").is_some());
        assert!(r.lookup("core.literal").is_some());
        assert!(r.lookup("nonexistent").is_none());
    }

    #[test]
    fn phase1_default_entries_carry_correct_kinds() {
        let r = FunctionRegistry::phase1_default();
        assert_eq!(
            r.lookup("io.csv").unwrap().kind,
            RegistryKind::SourceConstructor
        );
        assert_eq!(r.lookup("core.iri").unwrap().kind, RegistryKind::CoreIri);
        assert_eq!(
            r.lookup("core.triple").unwrap().kind,
            RegistryKind::CoreTriple
        );
        assert_eq!(
            r.lookup("core.literal").unwrap().kind,
            RegistryKind::CoreLiteral
        );
    }
}
