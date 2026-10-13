//! `textDocument/definition` — where a name a program writes is declared.
//!
//! [`crate::defs::name_at`] says what the name under the cursor names, and the
//! answer is where that is declared: a top-level name's
//! [`Definition::declaration`](fossil_hir::def_map::Definition::declaration),
//! and a property key's field in the type its mapping targets. Both read the
//! resolved type's own file ([`fossil_hir::shapes::ResolvedShape::file`]), so
//! a type declared in the program and one imported from a document answer the
//! same way and this module never asks which it has.
//!
//! The program is one file and the documents it names; there is no import of
//! another program, so nothing here takes a set of open files.
//!
//! # Precision
//!
//! A declaration carries its span: a name the program binds, a type's name
//! and each field's, or the decoder's own spans in a document. Where a decoder
//! has none (`ShExJ`, SHACL), the answer is the right file with a `0..0` range
//! — the top of the document, which takes the reader to the definition's file
//! rather than nowhere. `0..0` is the precision flag, and every consumer
//! already reads it.
//!
//! # Domain boundary
//!
//! Returns Fossil-domain [`NavigationTarget`]s (`{ file, range: Range<u32> }`),
//! never `lsp_types::Location`: the byte→UTF-16 translation is the host's,
//! through [`crate::position::position`], so this stays transport-free and
//! wasm-clean. A property key resolves by its text against the checker's
//! `predicates` table, so nothing here indexes into a body.

use std::ops::Range;

use fossil_base::SourceFile;
use fossil_hir::check::typecheck_mapping;
use fossil_hir::def_map::MappingLoc;
use fossil_hir::shapes::resolve_target_shape;

use crate::defs::{NameAt, name_at};
use crate::position::token_at_position;

/// A goto-def navigation target: the owning file + the byte range.
///
/// The host translates `range` to a UTF-16 `lsp_types::Range` and pairs it
/// with the file URI to build a `lsp_types::Location`.
///
/// **`range == 0..0` means "this file, position unknown"** — see the module
/// docs. It is produced only for a declaration in a document whose decoder
/// kept no span for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavigationTarget {
    /// The file that contains the definition. May be the `.fossil` program or
    /// a shape document it names.
    pub file: SourceFile,
    /// Byte range of the definition site into that file's text.
    pub range: Range<u32>,
}

/// Resolve the definition of the identifier at an LSP position in `file`.
///
/// Two kinds of name resolve, as [`crate::defs::name_at`] classifies them:
///
/// 1. **a name the program binds** — a source, a type, a mapping — wherever
///    it is written, the binding included: its declaration, which for an
///    imported type is the shape in the document, and the binding line's name
///    when that document is not registered;
/// 2. **a property key** → the field or predicate that declares it, in the
///    type the mapping targets.
///
/// At most one target, since one name has one declaration; a `Vec` because
/// "nothing under the cursor" is an empty answer, as LSP's is. Empty also for
/// a name the program does not bind — a misspelt type resolves to nothing, not
/// to the header it is written in.
///
/// `line` / `character` are UTF-16 LSP coordinates (resolved via the
/// [`crate::position::LineIndex`]); never byte offsets.
#[must_use]
pub fn goto_definition(
    db: &dyn fossil_base::Db,
    file: SourceFile,
    line: u32,
    character: u32,
) -> Vec<NavigationTarget> {
    let Some(token) = token_at_position(db, file, line, character) else {
        return Vec::new();
    };
    let target = match name_at(db, file, &token) {
        Some(NameAt::Definition(def)) => {
            let at = def.declaration(db, file);
            Some(NavigationTarget {
                file: at.file,
                range: at.span.map_or(0..0, Into::into),
            })
        }
        Some(NameAt::Key { mapping, name }) => key_target(db, mapping, &name),
        None => None,
    };
    target.into_iter().collect()
}

/// The field or predicate a property key names, in the type the mapping
/// targets.
///
/// The type is the mapping's own target, [`resolve_target_shape`], and the key
/// resolves through the checker's `predicates` table — the one hover and
/// `fossil-mir` read — so a key lands where the checker says it is. `None` when
/// the type does not resolve: the binding line says nothing about which
/// predicates a document declares, so there is no fallback for a key.
fn key_target(
    db: &dyn fossil_base::Db,
    mapping: MappingLoc<'_>,
    key: &str,
) -> Option<NavigationTarget> {
    let shape = resolve_target_shape(db, mapping)?;
    let home = shape.file?;
    let iri = typecheck_mapping(db, mapping)
        .checked(db)
        .ok()?
        .predicates(db)
        .iter()
        .find(|p| p.name.as_str() == key)?
        .iri
        .clone();
    let span = shape.constraint_for(&iri).and_then(|c| c.span);
    Some(NavigationTarget {
        file: home,
        range: span.map_or(0..0, Into::into),
    })
}
