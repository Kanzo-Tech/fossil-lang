//! File registry + Salsa source-file input.
//!
//! Phase 1 keeps `Files` as an empty placeholder — `SourceFile` is the
//! only entry point for consumers. Phase 2+ adds an interned-path registry
//! once multi-file resolution is needed.

#[salsa::input(debug)]
pub struct SourceFile {
    #[returns(ref)]
    pub text: String,
    #[returns(ref)]
    pub path: String,
}

#[derive(Debug, Default, Clone)]
pub struct Files {
    // Phase 1: empty placeholder. Phase 2 adds an interned-path registry
    // (e.g., HashMap<PathBuf, SourceFile>) so def_map can resolve file imports.
}
