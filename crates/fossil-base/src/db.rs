//! Salsa `Db` trait + `FossilDb` implementation.
//!
//! Per ADR-0003 the trait is intentionally **thin**: just `system()` + `files()`.
//! Descriptors, registry, and host capabilities flow through `&dyn System`
//! rather than as separate composing traits. This is the verified pattern from
//! `ruff_db` (and `ty_wasm` composes `Workspace { db, system }` over it).

use std::sync::Arc;

use crate::files::Files;
use crate::system::System;

#[salsa::db]
pub trait Db: salsa::Database {
    fn system(&self) -> &dyn System;
    fn files(&self) -> &Files;
}

#[salsa::db]
#[derive(Clone)]
pub struct FossilDb {
    storage: salsa::Storage<Self>,
    system: Arc<dyn System>,
    files: Files,
}

impl std::fmt::Debug for FossilDb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FossilDb")
            .field("system", &self.system)
            .finish_non_exhaustive()
    }
}

impl FossilDb {
    #[must_use]
    pub fn new(system: Arc<dyn System>) -> Self {
        Self {
            storage: salsa::Storage::default(),
            system,
            files: Files::default(),
        }
    }
}

#[salsa::db]
impl salsa::Database for FossilDb {}

#[salsa::db]
impl Db for FossilDb {
    fn system(&self) -> &dyn System {
        &*self.system
    }

    fn files(&self) -> &Files {
        &self.files
    }
}
