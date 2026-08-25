//! `textDocument/completion` — three sources.
//!
//! Merges three completion sources into a single `Vec<lsp_types::CompletionItem>`
//! (the `lsp-types`-direct shape is WASM-clean, so the browser host and the LSP
//! both consume it without a second conversion):
//!
//! 1. **stdlib functions**, narrowed to the cursor's RECEIVER and to its
//!    POSITION. After `str.` the list is what a string HAS, labelled `trim`;
//!    after a `:=` binding it is the relation verbs; on the left of a property
//!    line it is nothing, because `PropertyLhs := IDENT` and no catalogued row
//!    is an `IDENT`; anywhere else with no dot to the left it is the catalogue
//!    whole, spelled `str.trim`. See [`stdlib_completions`] for what the
//!    narrowing does not cover. There is no auto-import edit and no
//!    `(native-only)` tag:
//!    the first named a `use <ns>` line no program writes, and the second named
//!    `WasmClass::NativeUdfOnly`, a class that no longer exists.
//! 2. **shape properties** — the output contract of the mapping the cursor is
//!    in, offered in the ONE position a property key can be written: the left
//!    of a property line ([`Scope::PropertyKey`]). Each is labelled
//!    [`fossil_graph_schema::short_name`] of its predicate IRI — the `@rename`
//!    the program addressed to it, else the last segment — because that is
//!    what `PropertyLhs := IDENT` accepts and what the checker resolves a key
//!    against. It offered the raw `constraints[].predicate` IRI, wherever the
//!    cursor was inside a mapping whose shape resolved.
//! 3. **source fields** — the columns of the row the mapping reads, when the
//!    host has registered a descriptor for it AND the receiver is the name that
//!    row is addressed by. It fired on every `.` inside a mapping instead, so
//!    the columns were offered as the members of a string and of a name that
//!    denotes nothing.
//!
//! All three read ONE [`Scope`], resolved once per request: the rows a
//! catalogue has for a receiver, the columns a source row has and *whether a
//! key can be written here at all* are answers to the same question — where
//! the cursor is — and asking it three times is how they came to disagree.
//!
//! A source between the first and the second was **prefixes**: declared ones
//! from the cross-file index plus a well-known set offered as auto-importable.
//! It went whole with the `prefix` declaration; a vocabulary is not a form this
//! language has.
//!
//! # Domain + WASM boundary
//!
//! Returns `lsp_types::CompletionItem` directly; no stdio / JSON-RPC. The stdlib
//! source needs only the static catalog (`fossil_hir::stdlib::stdlib()`, no db); the
//! shape-property source calls `resolve_target_shape`, which reads the document
//! the program names as a Salsa INPUT — through `file_at` and the tracked
//! `shape_document`, which the HOST must have registered (see
//! [`crate::shape_documents`]). Both dependencies are file-keyed, not
//! per-mapping: ten mappings checked against one document share one decode, so
//! no new per-mapping key appears and the `body()` fan-out is unchanged
//! All type rendering routes through
//! [`fossil_hir::render_ty_kind`], so `TyKind::Unknown` never leaks into a
//! `detail` string: internal inference state must not reach the user.

use fossil_base::SourceFile;
use fossil_graph_schema::Primitive;
use fossil_hir::def_map::{MappingLoc, def_map};
use fossil_hir::item_tree::{ItemHeader, item_tree};
use fossil_hir::render_ty_kind;
use fossil_hir::shapes::resolve_target_shape;
use fossil_hir::stdlib::{FunctionRegistry, Receiver, ScalarTy};
use fossil_hir::ty::{Record, Ty, TyKind};
use fossil_syntax::{SyntaxKind, SyntaxToken};
use lsp_types::{CompletionItem, CompletionItemKind, CompletionItemTag};
use smol_str::SmolStr;

use crate::position::{node_at_position, token_at_position};

/// Compute completion items at an LSP position, merging the three sources.
///
/// `files` is the host's open-file set (for cross-file prefix resolution);
/// `file` is the file the cursor is in. No source is unconditional — this
/// sentence said the stdlib and prefix ones were, and there is no prefix source
/// left and no position at which the whole catalogue is the honest answer to a
/// dot. A program that names no output shape document contributes no
/// shape-property items; a cursor outside a key position contributes none
/// either.
///
/// `line` / `character` are UTF-16 LSP coordinates.
#[must_use]
pub fn completions(
    db: &dyn fossil_base::Db,
    // The open-file set. It fed `prefix_completions`, the one source that was
    // cross-file; nothing left here reads past `file`, and the parameter stays
    // because it is `fossil-lsp`'s call shape and the next cross-file source
    // (shape names from a `type` binding in a sibling file) wants it back.
    _files: &[SourceFile],
    file: SourceFile,
    line: u32,
    character: u32,
) -> Vec<CompletionItem> {
    let mut items = Vec::new();

    // ONE receiver question, asked once, read by the two sources that have a
    // receiver. It was asked by `stdlib_completions` alone and
    // `source_field_completions` did not ask at all — see [`Scope`].
    //
    // `stdlib()` and not `stdlib_default()`: the second is the CONSTRUCTOR, so
    // calling it here rebuilt the whole catalogue — every row, its signature and
    // its lowering — on every completion request, which is the keystroke path.
    // The `LazyLock` behind `stdlib()` exists for exactly this and was being
    // walked past. The two names differ by eight characters and neither is a
    // type error, which is why `is_static_catalogue_shared` below fails on the
    // ADDRESS rather than on the contents.
    let registry = fossil_hir::stdlib::stdlib();
    let scope = scope_at_cursor(db, file, line, character, registry);

    stdlib_completions(registry, &scope, &mut items);
    shape_property_completions(db, &scope, &mut items);
    source_field_completions(db, &scope, &mut items);

    items
}

/// Where the cursor is — which is what decides both *which rows* are offered
/// and *what they are labelled*, taken in one place.
///
/// It began as the receiver question alone («what does the head to the left of
/// the dot name?») and every variant but one still answers it. All three
/// sources read it: the catalogue's rows, the source row's COLUMNS and the
/// shape's property KEYS are answers to one question, and each source that
/// asked it separately got it wrong. The second fired on every `.` inside a
/// mapping and offered the row's columns as the members of a string, of an
/// unknown name, and of a call's result; the third asked only «is the cursor
/// inside a mapping whose shape resolves», which is true of the header, of the
/// right of an `=`, and of the member position after a dot.
enum Scope<'db> {
    /// No dot: the cursor is not in a member position, so the catalogue is
    /// offered whole and spelled in full (`str.trim`). Nothing narrower is
    /// honest — there is no receiver to narrow by.
    Catalogue,
    /// The left-hand side of a property line inside a mapping body — the one
    /// position `PropertyLhs := IDENT` puts a property key, carrying the
    /// mapping whose target shape declares which keys those are.
    ///
    /// It is a POSITION and not a receiver, which is why the shape source could
    /// not be narrowed by the receiver question the other variants answer. The
    /// CST is what says so: the cursor's token is an `IDENT` whose parent is a
    /// `PROPERTY_LHS`. An `AT_ATTR` parented the same way is the identity line
    /// (`SubjectAssign := AT_ATTR ASSIGN Expression`, a different production
    /// sharing the node), and `@subject` is not a key.
    ///
    /// It is the one variant where the shape source speaks and the stdlib one
    /// is silent: the catalogue is dotted and a key is a bare name.
    PropertyKey(MappingLoc<'db>),
    /// A head the catalogue classifies as a type: `str` → `Scalar(String)`,
    /// `seq` → `Relation`. Also what a `:=` binding resolves to, and what a
    /// COLUMN resolves to once its type is read off the row.
    Members(Receiver),
    /// A namespace head (`io`, `parse`, `math`, `validate`, `core`, `anon`).
    /// [`Receiver::Namespace`] is ONE receiver shared by all six, so
    /// `members_of` would answer with every namespace's rows; the head is
    /// carried so `io.` offers `io.*` and not `math.abs`.
    Namespace(String),
    /// The head names the ROW the enclosing mapping's `from` clause bound, and
    /// the record is that row. Its members are the row's COLUMNS.
    ///
    /// It is ALSO a relation — the same binding is `users.where(…)` in a `:=`
    /// right-hand side and `users.name` in a mapping body — so the stdlib
    /// source treats this exactly as [`Receiver::Relation`] and offers the
    /// verbs beside the columns. Which of the two a body position can actually
    /// take is a receiver question this does not answer: only the columns are
    /// writable as a property value, and the CST says which side of the `:=`
    /// the cursor is on.
    Row(Record<'db>),
    /// A dot whose left half names nothing the catalogue or the file knows —
    /// `orders.`, a bare leading `.`, a call's result. Both sources stay quiet:
    /// nothing is known about the receiver, and a list is a claim.
    Nothing,
}

/// Source 1: the standard library, narrowed to the cursor's RECEIVER — and,
/// where there is no receiver to narrow by, to its POSITION.
///
/// It was not narrowed at all: every row of `stdlib_default` was pushed
/// wherever completion fired, so a `.` after a relation offered 51 items — 13
/// `str.*` and 3 `io.*` among them — in `HashMap` iteration order, which three
/// consecutive runs gave three different spellings of. `RegistryEntry` has
/// carried `recv` and `member` since the receiver replaced dispatch-by-string,
/// and nothing in this crate read either field.
///
/// **The position, which the receiver question cannot ask.** A property key is
/// `PropertyLhs := IDENT` (grammar.bnf) — a bare name — and every catalogued row
/// is dotted, so none of them is writable there and
/// [`Scope::PropertyKey`] offers none. That was the last position left
/// unnarrowed after the shape source was repaired: the two sources were
/// offering different unwritable spellings of the same wrong thing in the same
/// place, 51 rows and 2 raw IRIs. `tests/completion_property_key.rs` has both
/// numbers and
/// `every_position_that_is_not_a_key_keeps_its_catalogue` pins the six
/// positions the cut must not reach.
///
/// Two things follow from knowing the receiver, and both are here:
///
/// - the rows are the ones the value HAS (`FunctionRegistry::members_of`);
/// - they are labelled by their MEMBER (`trim`, not `str.trim`), because after
///   `x.` the member is what the user is typing. The `detail` still renders the
///   full dotted signature, so the type path is never hidden.
///
/// The order is by label, always. `FunctionRegistry` is a `HashMap` and an
/// unsorted list is a different list on every call — `xtask`'s reference
/// emitter sorts its groups for the same reason (`crates/xtask/src/reference.rs`,
/// `by_head`).
///
/// **Now covered:** a column. `users.name.` reads `name`'s type off the row
/// [`fossil_hir::infer::source_row_inferred`] built and offers the members of
/// THAT — the 13 `str.*` for a String, nothing for an Integer, because the
/// catalogue has no row whose receiver is any scalar but String.
///
/// **Still not covered:** a receiver that is an EXPRESSION.
/// `users.name.trim().` has a `)` to the left of its dot, and typing it needs
/// an id the body arena does not mint — one entry per property VALUE, so a
/// sub-expression has none (`crate::hover` records the same limit for the same
/// reason). It answers [`Scope::Nothing`], which is silence and not a guess.
fn stdlib_completions(
    registry: &FunctionRegistry,
    scope: &Scope<'_>,
    items: &mut Vec<CompletionItem>,
) {
    // `(label, entry)` — the label differs between the two shapes, so it is
    // decided while selecting rather than guessed afterwards from the name.
    let mut rows: Vec<(String, &fossil_hir::stdlib::RegistryEntry)> = match scope {
        // A key position is `PropertyLhs := IDENT` and no catalogued row is one:
        // `str.trim` is no more writable there than the raw predicate IRI the
        // shape source used to offer. Both sources were making the same
        // mistake, and this arm shared `Scope::Catalogue`'s while the shape
        // source was repaired alone — 51 items, dotted, offered where none of
        // them parses.
        Scope::Nothing | Scope::PropertyKey(_) => Vec::new(),
        Scope::Catalogue => registry.iter().map(|e| (e.name.to_string(), e)).collect(),
        // A row binding is a relation as well as a row — see [`Scope::Row`].
        Scope::Row(_) => registry
            .members_of(Receiver::Relation)
            .map(|e| (e.member.to_string(), e))
            .collect(),
        Scope::Members(recv) => registry
            .members_of(*recv)
            .map(|e| (e.member.to_string(), e))
            .collect(),
        Scope::Namespace(head) => {
            let prefix = format!("{head}.");
            registry
                .iter()
                .filter(|e| e.recv == Receiver::Namespace && e.name.starts_with(&prefix))
                .map(|e| (e.member.to_string(), e))
                .collect()
        }
    };
    rows.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.name.cmp(&b.1.name)));

    for (label, entry) in rows {
        // The receiver is the dotted prefix (`str` in `str.trim`).
        let namespace = entry.name.split('.').next().unwrap_or(&entry.name);

        // A `WasmClass::NativeUdfOnly` entry was tagged DEPRECATED here with a
        // `(native-only)` detail suffix, so the browser playground could gray
        // it out. There is no such entry and no such class: ruling 15 of
        // `SURFACE-PLAN.md` deleted the `Udf` lowering, and with it the only
        // reason a catalogued function could fail to run in a browser. Every
        // row runs everywhere, so nothing is grayed.
        let detail = render_sig(namespace, entry);
        let tags: Option<Vec<CompletionItemTag>> = None;

        // A gleam-lsp auto-import edit sat here: a top-of-file `use <ns>`
        // insertion, offered when the file's `PrefixIndex` said the namespace
        // was not declared. There is no `use` production — `use` is an ordinary
        // identifier and there is no module system for a name to come from —
        // and no prefix table to ask, and `io` / `str` / `clean` are resolved
        // by the checker against a catalogue rather than imported at all — so
        // the edit named a line no program writes.
        let additional_text_edits = None;

        items.push(CompletionItem {
            label,
            kind: Some(CompletionItemKind::FUNCTION),
            detail: Some(detail),
            tags,
            additional_text_edits,
            ..Default::default()
        });
    }
}

/// Which rows the cursor's position selects.
///
/// The key position is decided first and structurally — the CST says whether
/// the cursor is on a `PROPERTY_LHS` — because it is the one variant that is
/// not a receiver, and because a key has no dot to reason from.
///
/// After that, the head is the token before the dot, and there are two ways to
/// be after one: the cursor is ON the dot (completion fired the moment `.` was
/// typed), or it is inside the partial member that follows it (`users.wh|`).
/// Both are handled, because an editor that re-requests on every keystroke
/// produces the second on the very next character.
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Salsa-handle lifetime contract
fn scope_at_cursor<'db>(
    db: &'db dyn fossil_base::Db,
    file: SourceFile,
    line: u32,
    character: u32,
    registry: &FunctionRegistry,
) -> Scope<'db> {
    let Some(token) = token_at_position(db, file, line, character) else {
        return Scope::Catalogue;
    };
    if at_property_key(&token)
        && let Some(mapping) = enclosing_mapping_loc(db, file, line, character)
    {
        return Scope::PropertyKey(mapping);
    }
    let dot = if token.kind() == SyntaxKind::DOT {
        token
    } else {
        match prev_meaningful(&token) {
            Some(t) if t.kind() == SyntaxKind::DOT => t,
            // Not in a member position at all.
            _ => return Scope::Catalogue,
        }
    };
    let Some(head_token) = prev_meaningful(&dot) else {
        return Scope::Nothing;
    };
    if head_token.kind() != SyntaxKind::IDENT {
        // A leading `.name` is a RETIRED form — `parser/expr.rs` refuses it by
        // name (`retired::LEADING_DOT`) because the row has a name and every
        // reference is qualified. A `)` is the other way to get here, and it is
        // the call-result receiver nothing types yet. Neither has a receiver to
        // the left of the dot, so neither source has anything to say.
        return Scope::Nothing;
    }
    let head = head_token.text().to_string();

    // The catalogue classifies its own heads, and `receiver_of` is the ONE
    // place that classification lives (`fossil-hir/src/stdlib.rs`). Asking it
    // about an arbitrary identifier would answer `Namespace` for `orders`, so
    // the head has to be catalogued FIRST.
    if registry.is_catalogued_head(&head) {
        let recv = fossil_hir::stdlib::receiver_of(&head);
        return if recv == Receiver::Namespace {
            Scope::Namespace(head)
        } else {
            Scope::Members(recv)
        };
    }

    // The row the enclosing mapping's `from` clause bound, and the two things
    // the head can be against it. This is the INFERENCE the previous commit
    // left open: `users.name.` is a member of the type of the COLUMN `name`,
    // and the type is on the row the host's descriptor produced.
    if let Some((binding, record)) = enclosing_row(db, file, line, character) {
        if head == binding {
            return Scope::Row(record);
        }
        // `users.name.` — the head is a column of the row, so the receiver is
        // that column's TYPE. `qualifier` is what keeps this from firing on a
        // bare `name.`: a column is only reachable through the binding, and an
        // unqualified reference is not a form the language has.
        if qualifier(&head_token).is_some_and(|q| q == binding)
            && let Some(field) = record.fields(db).iter().find(|f| f.name == head)
            && let Some(recv) = receiver_of_ty(db, field.ty)
        {
            return Scope::Members(recv);
        }
    }

    // A `:=` binding is a relation, so its members are the verbs. The symbol
    // index is the file's own table of them and is a plain CST walk — no Salsa
    // key is added here.
    let indexed_source = crate::SymbolIndex::build(db, file)
        .of_kind(crate::SymbolKind::Source)
        .any(|e| e.name == head);
    if indexed_source {
        return Scope::Members(Receiver::Relation);
    }
    Scope::Nothing
}

/// Is the cursor's token the name on the left of a property line?
///
/// `PropertyLhs := IDENT` (`grammar.bnf`) — a bare name, and the only place one
/// goes. The parser gives that name a `PROPERTY_LHS` node of its own, so this
/// is a parent check and not a search: no scan for an `=` to the right, which
/// is the condition that would also accept the header's `:` line and would
/// answer wrongly for a key not yet followed by one.
///
/// Two positions a key COULD occupy answer `false`, and both are the CST
/// declining to say rather than this function narrowing:
///
/// - a still-blank body line, whose indent the parser attaches to the PREVIOUS
///   property's expression — indistinguishable from a value continued onto a
///   second line;
/// - the column immediately before a key's first character, which is the same
///   whitespace token.
///
/// The first keystroke closes both. An `AT_ATTR` under the same node is
/// `@subject`, a different production (`SubjectAssign`) sharing the node, and
/// no property key can be written where it already stands.
fn at_property_key(token: &SyntaxToken) -> bool {
    token.kind() == SyntaxKind::IDENT
        && token
            .parent()
            .is_some_and(|p| p.kind() == SyntaxKind::PROPERTY_LHS)
}

/// The token before `t`, skipping whitespace and comments.
fn prev_meaningful(t: &SyntaxToken) -> Option<SyntaxToken> {
    let mut cur = t.prev_token();
    while let Some(tok) = cur {
        if !matches!(tok.kind(), SyntaxKind::WHITESPACE | SyntaxKind::COMMENT) {
            return Some(tok);
        }
        cur = tok.prev_token();
    }
    None
}

/// The identifier `head` is itself a member of — the `users` of `users.name.`.
///
/// `None` when `head` stands on its own, which is every unqualified name.
fn qualifier(head: &SyntaxToken) -> Option<String> {
    let dot = prev_meaningful(head).filter(|t| t.kind() == SyntaxKind::DOT)?;
    let owner = prev_meaningful(&dot).filter(|t| t.kind() == SyntaxKind::IDENT)?;
    Some(owner.text().to_string())
}

/// The row the cursor's enclosing mapping reads, under the name its body
/// addresses it by — `("users", {name: String, age: Integer})`.
///
/// The name comes from [`item_tree`], the SIGNATURE-only query, so it survives
/// every keystroke inside the body the user is typing in; the row comes from
/// [`fossil_hir::infer::source_row_inferred`], the side-effect-free sibling of
/// the checker's `resolve_source_scope`. That choice is load-bearing and not a
/// preference: `resolve_source_scope` can reach `delay_span_bug`, and a
/// `salsa` accumulator OUTSIDE a tracked function **panics** by construction
/// (`salsa::accumulator`: *«cannot accumulate values outside of an active
/// tracked function»*). Completion is not inside one.
///
/// `None` when the mapping's `from` is not a plain `IDENT`, when the host
/// registered no descriptor for the URI the binding names, or when the cursor
/// is outside any mapping — three different reasons to say nothing, and the
/// editor says nothing for all three rather than guessing column names.
///
/// **The one name, and not the scope.** A body that draws `from Adults`, where
/// `Adults := Users.where(…)`, addresses its columns as `Users` — see
/// `fossil_hir::ty::Rows`. Only the checker's `Rows` knows that, and only a
/// join makes a second name addressable. Neither is reachable from here without
/// the panic above, so a derived binding and a join alias resolve no row and
/// offer no columns, which is what they did before this and is measured in
/// `tests/completion_receiver_inference.rs`.
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Salsa-handle lifetime contract
fn enclosing_row<'db>(
    db: &'db dyn fossil_base::Db,
    file: SourceFile,
    line: u32,
    character: u32,
) -> Option<(SmolStr, Record<'db>)> {
    let mapping = enclosing_mapping_loc(db, file, line, character)?;
    // Filter-then-nth, the contract `MappingLoc::index` is numbered under: the
    // item tree carries source definitions too, and counting them in would name
    // a different mapping's binding.
    let binding = item_tree(db, file)
        .items(db)
        .iter()
        .filter_map(|item| match item {
            ItemHeader::Mapping(header) => Some(header),
            ItemHeader::SourceDef(_) => None,
        })
        .nth(mapping.index(db))?
        .source_binding
        .clone()?;
    let row = fossil_hir::infer::source_row_inferred(db, mapping)?;
    match row.kind(db) {
        TyKind::Record(record) => Some((binding, *record)),
        _ => None,
    }
}

/// The catalogue receiver a value of this type has, if the catalogue classifies
/// it at all.
///
/// The inverse of [`ScalarTy::to_ty`], and written as an exhaustive match over
/// [`Primitive`] so that a primitive added to the lattice is a compile error
/// here rather than a member list that silently goes empty.
///
/// Three primitives answer `None` and that is the catalogue's own shape:
/// `receiver_of` classifies exactly two heads as types (`str` → `Scalar(String)`,
/// `seq` → `Relation`), so `members_of` for any other scalar is EMPTY anyway.
/// An Integer column offers nothing after its dot, and nothing is the honest
/// answer until the catalogue grows a row that hangs off one.
fn receiver_of_ty<'db>(db: &'db dyn fossil_base::Db, ty: Ty<'db>) -> Option<Receiver> {
    match ty.kind(db) {
        TyKind::Primitive(primitive) => scalar_of(*primitive).map(Receiver::Scalar),
        _ => None,
    }
}

/// The `'db`-free [`ScalarTy`] tag for a lattice primitive, or `None` for one
/// no signature can name.
const fn scalar_of(primitive: Primitive) -> Option<ScalarTy> {
    Some(match primitive {
        Primitive::String => ScalarTy::String,
        Primitive::Integer => ScalarTy::Integer,
        Primitive::Float => ScalarTy::Float,
        Primitive::Bool => ScalarTy::Bool,
        Primitive::Date => ScalarTy::Date,
        Primitive::DateTime => ScalarTy::DateTime,
        // `ScalarTy` names no `Time`, `gYear` or `anyURI`, and `SeqString` —
        // the one tag with no primitive — is a `TyKind::Seq`, not a
        // `TyKind::Primitive`, so it never reaches this table.
        Primitive::Time | Primitive::GYear | Primitive::AnyUri => return None,
    })
}

/// Source 2: the mapping's output contract — its target shape's predicates,
/// under the names a program can WRITE, in the one position it can write them.
///
/// It had two defects and they are independent.
///
/// **The label** was `constraint.predicate`, the raw predicate IRI, so every
/// item it offered wrote a line the parser refuses: `PropertyLhs := IDENT`, and
/// `https://shop.example/voc#email` is not one. The name is
/// [`ResolvedShape::short_names`] — [`fossil_graph_schema::short_name`] over
/// the `@rename` table off the `type` binding, else the IRI's last segment.
/// That is the same table [`fossil_hir::check`] resolves a written key against
/// and the same one `OutputShapes::to_graph_schema` emits the column under, so
/// what is offered, what compiles and what ships are one name. [`local_name`]
/// would have been the plausible wrong answer: it agrees on every predicate no
/// `@rename` names, so it fails only on the one program the rename exists for,
/// and it fails by offering a key the checker rejects.
///
/// [`local_name`]: fossil_graph_schema::local_name
/// [`ResolvedShape::short_names`]: fossil_hir::shapes::ResolvedShape::short_names
///
/// **The position** was «the cursor is inside a mapping whose shape resolves»,
/// which is also true of the header, of the right-hand side of an `=`, and of
/// the member position after a dot. It is [`Scope::PropertyKey`] and nothing
/// else.
///
/// A predicate dropped for a short-name COLLISION is not offered: `short_names`
/// keeps the first and reports the rest, and a name that resolves to two IRIs
/// is exactly the key the checker refuses. The repair — `@rename` — is what
/// puts it back, under the name the program chose.
fn shape_property_completions(
    db: &dyn fossil_base::Db,
    scope: &Scope<'_>,
    items: &mut Vec<CompletionItem>,
) {
    let Scope::PropertyKey(mapping) = scope else {
        return;
    };
    // A document that is missing, undecodable or does not declare this shape is
    // a [`fossil_hir::shapes::TargetShapeError`], and `typecheck_mapping` is
    // where it becomes a diagnostic the user reads. A completion list is not a
    // place to report it: the honest answer here is to offer no shape
    // properties, which is what a program with no output contract gets too.
    let Ok(Some(shape)) = resolve_target_shape(db, *mapping) else {
        return;
    };
    let renames = shape_iri_of(db, *mapping).map_or_else(Vec::new, |iri| {
        def_map(db, mapping.file(db)).renames_for_shape(db, &iri)
    });
    let (table, _collisions) = shape.short_names(&renames);
    for (name, predicate) in table {
        let ty_str = shape
            .constraint_for(&predicate)
            .and_then(|c| c.value_ty)
            .map_or_else(|| "Iri".to_string(), |ty| render_ty_kind(db, ty.kind(db)));
        items.push(CompletionItem {
            // The IRI is what the key MEANS, so moving it out of the label must
            // not lose it — it goes to the `detail`, bracketed, where the
            // stdlib source keeps `str` for the label `trim`.
            label: name.to_string(),
            kind: Some(CompletionItemKind::FIELD),
            detail: Some(format!("shape property : {ty_str}  [{predicate}]")),
            ..Default::default()
        });
    }
}

/// The fully-resolved shape IRI a mapping targets — the key a `@rename` is
/// addressed by — or `None` when its header did not lower.
///
/// Mirrors `fossil_hir::check`'s own `shape_iri_of`, which is private to the
/// checker. It adds no Salsa fan-out: `resolve_target_shape` has already
/// evaluated `lower_to_hir` for this file on the line above.
fn shape_iri_of(db: &dyn fossil_base::Db, mapping: MappingLoc<'_>) -> Option<SmolStr> {
    fossil_hir::lower::lower_to_hir(db, mapping.file(db))
        .mapping(db, mapping.index(db))
        .map(|m| m.shape_iri.clone())
        .filter(|iri| !iri.is_empty())
}

/// Source 3: the COLUMNS of the row the receiver names.
///
/// It had no receiver. The trigger was `at_field_ref_context` — «the cursor is
/// on a `DOT` and somewhere under a `MAPPING`» — so every dot in a body got the
/// row's columns whatever stood to its left, and the three that are not the row
/// all got the same wrong two items: `str.` got them beside the 13 correct
/// string members, `orders.` and `users.name.` got them and nothing else, and a
/// leading `.` — a form `parser/expr.rs` refuses by name — got them too.
///
/// The receiver is [`Scope::Row`] and nothing else: the head has to be the name
/// the mapping's `from` clause bound (see [`enclosing_row`] for the one name it
/// can be, and for the two it cannot). The fields + their inferred types are
/// [`fossil_hir::infer::source_row_inferred`]'s — the same forward-typed record
/// the checker reads. No descriptor registered means no [`Scope::Row`] at all,
/// so the editor is quiet rather than guessing field names.
fn source_field_completions(
    db: &dyn fossil_base::Db,
    scope: &Scope<'_>,
    items: &mut Vec<CompletionItem>,
) {
    let Scope::Row(record) = scope else {
        return;
    };
    for field in record.fields(db) {
        items.push(CompletionItem {
            label: field.name.to_string(),
            kind: Some(CompletionItemKind::FIELD),
            detail: Some(format!(
                "source field : {}",
                render_ty_kind(db, field.ty.kind(db))
            )),
            ..Default::default()
        });
    }
}

/// Resolve the cursor's enclosing mapping to its [`def_map`] `MappingLoc`
/// (mirrors `hover::resolve_hover_target`'s filter-then-nth contract,
/// without the property-level resolution).
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the Salsa-handle lifetime contract
fn enclosing_mapping_loc<'db>(
    db: &'db dyn fossil_base::Db,
    file: SourceFile,
    line: u32,
    character: u32,
) -> Option<fossil_hir::def_map::MappingLoc<'db>> {
    let node = node_at_position(db, file, line, character)?;
    let mut current = Some(node);
    let mut mapping_node = None;
    while let Some(n) = current {
        if n.kind() == SyntaxKind::MAPPING {
            mapping_node = Some(n);
            break;
        }
        current = n.parent();
    }
    let mapping_node = mapping_node?;
    let cst = fossil_syntax::parse(db, file);
    let mapping_index = cst
        .root(db)
        .syntax()
        .children()
        .filter(|c| c.kind() == SyntaxKind::MAPPING)
        .position(|c| c == mapping_node)?;
    def_map(db, file)
        .mappings(db)
        .iter()
        .find(|m| m.index(db) == mapping_index)
        .copied()
}

/// Render a stdlib entry's signature as a `detail` string from its `'db`-free
/// [`fossil_hir::stdlib::SigSpec`].
fn render_sig(namespace: &str, entry: &fossil_hir::stdlib::RegistryEntry) -> String {
    let params = entry
        .sig
        .params
        .iter()
        .map(|p| sig_name(p.ty))
        .collect::<Vec<_>>()
        .join(", ");
    let ret = sig_name(entry.sig.ret);
    format!("{}({params}) -> {ret}  [{namespace}]", entry.name)
}

/// Human-readable name for a signature position. Mirrors `render_ty_kind`'s
/// surface names.
///
/// A verb of the algebra shows `Rows` and `Predicate` — what completion offers
/// for `User.` is `where(Rows, Predicate) -> Rows`, and until the catalogue
/// could say that it offered `where(String) -> String`.
fn sig_name(t: fossil_hir::stdlib::SigTy) -> String {
    use fossil_hir::stdlib::SigTy;
    match t {
        SigTy::Scalar(s) => scalar_name(s),
        SigTy::Rows => "Rows".to_string(),
        SigTy::Predicate => "Predicate".to_string(),
        // A position that NAMES rather than evaluates, and completion says so:
        // `select(Rows, Column)` tells a reader to write `User.id` and not an
        // expression, which is the one thing the offer could not convey while
        // the catalogue said every verb took a bare relation.
        SigTy::Column => "Column".to_string(),
        SigTy::Binding => "Binding".to_string(),
    }
}

/// Human-readable name for a `'db`-free [`fossil_hir::stdlib::ScalarTy`].
fn scalar_name(s: fossil_hir::stdlib::ScalarTy) -> String {
    use fossil_hir::stdlib::ScalarTy as S;
    match s {
        S::String => "String",
        S::Integer => "Integer",
        S::Float => "Float",
        S::Bool => "Bool",
        S::Date => "Date",
        S::DateTime => "DateTime",
        S::SeqString => "Seq<String>",
    }
    .to_string()
}

// `import_edit` and `prefix_import_edit` lived here, plus the `top_of_file`
// zero-width range both inserted at. One wrote `use <namespace>`, the other
// `prefix <p>: <iri>`; neither line is a production the language has.

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use fossil_base::test_support::NativeSystem;
    use fossil_base::{Catalogue, Files, System};
    use std::sync::Arc;

    // A minimal host db stand-in, so these unit tests exercise the stdlib +
    // prefix sources (which are unconditional) without a filesystem fixture.
    // The shape-property source needs a program naming a real document on
    // disk, and is exercised end-to-end in `tests/completion.rs`.
    #[salsa::db]
    #[derive(Clone)]
    struct HostDb {
        storage: salsa::Storage<Self>,
        system: Arc<dyn System>,
        files: Files,
        catalogue: Catalogue,
    }

    impl std::fmt::Debug for HostDb {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("HostDb").finish_non_exhaustive()
        }
    }

    #[salsa::db]
    impl salsa::Database for HostDb {}

    #[salsa::db]
    impl fossil_base::Db for HostDb {
        fn system(&self) -> &dyn System {
            &*self.system
        }
        fn files(&self) -> &Files {
            &self.files
        }

        fn catalogue(&self) -> &Catalogue {
            &self.catalogue
        }
    }

    fn db() -> HostDb {
        HostDb {
            storage: salsa::Storage::default(),
            system: Arc::new(NativeSystem::default()),
            files: Files::default(),
            catalogue: Catalogue::default(),
        }
    }

    fn file(db: &HostDb, src: &str) -> SourceFile {
        SourceFile::new(db, src.to_string(), "c.fossil".to_string())
    }

    /// The stdlib source is BARE: no auto-import edit and no tag on any row it
    /// offers, and no `prefix:` completion at all.
    ///
    /// It said "unconditional — every catalogued entry is offered wherever the
    /// cursor is", and that stopped being true twice: `5be2de6` gave completion
    /// a [`Scope`], and `88091c0` gave the row binding one. The catalogue is
    /// offered WHOLE only in [`Scope::Catalogue`] — no dot to the left of the
    /// cursor — which is the position this fixture puts it in. After a dot the
    /// rows are the receiver's, and after an unknown one there are none.
    ///
    /// Four tests stood here and all four asserted the opposite. Two wanted an
    /// `additional_text_edits` inserting `use clean` — there is no `use`
    /// production and no module system for a name to come from. One wanted a
    /// `DEPRECATED` tag and a `(native-only)` detail on `clean.slug`, from a
    /// `WasmClass::NativeUdfOnly` that no longer exists: every catalogued row
    /// runs everywhere now, so nothing is grayed. Two more wanted `ex:` and
    /// `xsd:` offered as prefix completions.
    #[test]
    fn stdlib_entries_are_offered_bare() {
        let db = db();
        let f = file(&db, "Users : Person from u\n    name = u.name\n");
        let items = completions(&db, &[f], f, 0, 0);
        let trim = items
            .iter()
            .find(|i| i.label == "str.trim")
            .expect("str.trim must be offered");
        assert_eq!(trim.kind, Some(CompletionItemKind::FUNCTION));
        assert!(
            trim.additional_text_edits.is_none(),
            "there is no import line to add; got {:?}",
            trim.additional_text_edits,
        );
        assert!(
            trim.tags.is_none(),
            "no entry is native-only any more; got {:?}",
            trim.tags,
        );
        assert!(
            !items.iter().any(|i| i.label.ends_with(':')),
            "a `prefix:` completion is a form this language does not have",
        );
    }

    /// Source 4: with a host-registered `InferredDescriptor`, the ROW BINDING's
    /// dot offers the source's columns as Field completions.
    ///
    /// The fixture was `prefix ex: <…>` + `ex:name = .name` and every one of
    /// those three is a form the parser refuses by name: `retired::PREFIX_DECL`,
    /// `retired::CURIE`, `retired::LEADING_DOT`. It passed because the trigger
    /// was «a `DOT` under a `MAPPING`», which a refused leading dot still is —
    /// so the test proved the columns were offered where the language cannot
    /// write anything at all. `u.` is the position they belong to.
    #[test]
    fn offers_source_fields_after_the_row_binding_s_dot() {
        use fossil_descriptors_input::{InferredColumn, InferredDescriptor};
        let db = db();
        db.system
            .descriptors()
            .expect("the test host keeps a descriptor table")
            .insert(InferredDescriptor {
                // Keyed by the URI the binding names, not by `u` —
                // so the program below has to declare the binding for the
                // completion to find anything.
                uri: "u.csv".into(),
                columns: vec![
                    InferredColumn {
                        name: "name".into(),
                        primitive: Primitive::String,
                    },
                    InferredColumn {
                        name: "age".into(),
                        primitive: Primitive::Integer,
                    },
                ],
                freshness_token: String::new(),
            });
        let src = "u := io.csv(\"u.csv\")\nUser : Person from u\n    name = u.\n";
        let f = file(&db, src);
        // The cursor an editor puts one character past the `.` it fired on.
        let items = completions(&db, &[f], f, 2, 13);
        let fields: Vec<&str> = items
            .iter()
            .filter(|i| i.kind == Some(CompletionItemKind::FIELD))
            .map(|i| i.label.as_str())
            .collect();
        assert_eq!(
            fields,
            vec!["name", "age"],
            "`u` is the row: its members are the columns, in descriptor order",
        );
    }

    /// Without a registered descriptor the source-field source stays quiet — no
    /// guessing field names (the editor degrades, not invents).
    #[test]
    fn no_source_fields_without_descriptor() {
        let db = db();
        let src = "u := io.csv(\"u.csv\")\nUser : Person from u\n    name = u.\n";
        let f = file(&db, src);
        let items = completions(&db, &[f], f, 2, 13);
        assert!(
            !items.iter().any(|i| i.label == "name" || i.label == "age"),
            "no source fields should be offered without a descriptor",
        );
    }

    /// No `TyKind::Unknown` leaks into any `detail` string.
    #[test]
    fn no_unknown_leaks_into_detail() {
        let db = db();
        let f = file(&db, "User : ex:Person from u\n");
        let items = completions(&db, &[f], f, 0, 0);
        for i in &items {
            let d = i.detail.as_deref().unwrap_or("");
            assert!(
                !d.contains("Unknown") && !d.contains("InferenceId"),
                "completion detail leaked internal type state: {d:?}",
            );
        }
    }
}
