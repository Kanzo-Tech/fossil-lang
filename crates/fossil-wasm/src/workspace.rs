//! Workspace lifecycle — the `ty_wasm`-shaped multi-file API an editor host
//! uses. Mirrors Astral's `ty_wasm::Workspace`
//! exactly: a small
//! `FileHandle` newtype on the JS boundary plus an internal
//! `HashMap<FileHandle, SourceFile>` open-files map.
//!
//! # Why this is fan-out-safe
//!
//! The map keys are plain `u32` newtypes — they NEVER enter a Salsa key,
//! NEVER appear inside `Box<dyn Trait>`. `update_file` mutates the SAME
//! `SourceFile` Salsa input via the [`salsa::Setter`] (`set_text`); this is
//! the EXACT mechanism the LSP `didChange` path uses (the revision
//! bump is the cancellation trigger). No new tracked queries land
//! in the Workspace lifecycle path, so `MAX_PER_MAPPING_FAN_OUT` stays at 1
//! (`crates/fossil-hir/tests/invalidation_regression.rs` is what proves it).
//!
//! # Why `FileHandle` is a `u32` newtype (not the Salsa interned id)
//!
//! Salsa-interned ids are an implementation detail of the database — they
//! are not stable across JS calls (the JS shim cannot hold the `'db` lifetime
//! a `SourceFile` carries on the Rust side). A plain `u32` is the smallest
//! JS-friendly handle; the map below translates handle → `SourceFile` on
//! every operation. Counter overflow is panicked on (a browser tab
//! cannot realistically open 4 billion files; if it ever does, the panic
//! surfaces as a clear `console.error` via `console_error_panic_hook`).

use std::collections::HashMap;

use fossil_base::SourceFile;
use wasm_bindgen::prelude::*;

/// Opaque handle the JS side keys on. Plain `u32` newtype; NOT the Salsa
/// interned id (which doesn't cross the JS boundary).
///
/// `Copy + Clone + Eq + Hash` so it can be a `HashMap` key on the Rust
/// side; `#[wasm_bindgen]` so JS can pass it back across the boundary.
///
/// # Every exported method takes `&FileHandle`, and it has to
///
/// wasm-bindgen CONSUMES an exported struct passed by value: the generated glue
/// calls `__destroy_into_raw()` on the JS wrapper, which nulls its pointer. A
/// handle passed by value is therefore good for exactly one call, and the second
/// throws *"null pointer passed to rust"* — from inside a method that has nothing
/// wrong with it, naming no handle and no file.
///
/// `Copy` does not save it. The derive is a Rust-side property; nothing about it
/// reaches the JS boundary, where this is a class holding a pointer like any
/// other. The three methods a host calls repeatedly with one handle
/// (`update_file`, `close_file`, `diagnostics_for`) take `&FileHandle` so
/// wasm-bindgen borrows instead, and the handle stays live across the whole
/// lifetime `open_file`'s doc comment promises.
///
/// This was reachable from the FIRST loop an editor runs: `updateFile(h, text)` on
/// every check, with the handle `openFile` returned once.
#[wasm_bindgen]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct FileHandle(pub(crate) u32);

/// In-memory map of open files. Keys are [`FileHandle`]; values are the
/// Salsa [`SourceFile`] inputs that downstream queries (`def_map`,
/// `typecheck_mapping`) consume. Replacing the `SourceFile` for a given
/// handle on `update_file` is NOT how this works — we KEEP the `SourceFile`
/// and call `set_text` (Salsa `Setter`) so the revision bumps and
/// queries are invalidated incrementally.
// `pub(crate)` is the deliberate visibility (mirrors `WasmSystem` in
// `wasm_system.rs` — the clippy `redundant_pub_crate` nursery lint suggests
// `pub` since the parent module is private, but rustc's `unreachable_pub`
// then complains the other way; `pub(crate)` plus an allow is the wedge).
#[allow(clippy::redundant_pub_crate)]
#[derive(Debug, Default)]
pub(crate) struct OpenFiles {
    next: u32,
    files: HashMap<FileHandle, SourceFile>,
}

impl OpenFiles {
    pub(crate) fn insert(&mut self, file: SourceFile) -> FileHandle {
        let h = FileHandle(self.next);
        // Saturating would silently collide; panic is the honest signal.
        self.next = self
            .next
            .checked_add(1)
            .expect("FileHandle counter exhausted (u32::MAX files opened)");
        self.files.insert(h, file);
        h
    }

    pub(crate) fn get(&self, h: FileHandle) -> Option<SourceFile> {
        self.files.get(&h).copied()
    }

    pub(crate) fn remove(&mut self, h: FileHandle) -> Option<SourceFile> {
        self.files.remove(&h)
    }

    /// Iterate `(handle, file)` pairs. Used by `check()` to drain
    /// diagnostics across every open file, and by `diagnostics_for` to
    /// scope a drain to a single file.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (FileHandle, SourceFile)> + '_ {
        self.files.iter().map(|(&h, &f)| (h, f))
    }

    /// Path lookup for one handle — needed to populate the `uri` field on
    /// emitted diagnostics (a `SourceFile` carries its path as a Salsa-input
    /// string, but `check()` returns per-file rows and needs the URI in the
    /// outgoing structured shape).
    pub(crate) fn path_for(&self, h: FileHandle, db: &dyn fossil_base::Db) -> Option<String> {
        self.files.get(&h).map(|f| f.path(db).clone())
    }
}
