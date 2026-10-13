//! [`DefMap`] — per-file name resolution table.
//!
//! Holds the source bindings (`User := io.csv("...")`), the TYPES — imported
//! (`type { Person } := io.shex("...")`) and declared (`@iri("…") type Person`)
//! in one table — and the mapping list (one entry per `Name : Shape from
//! source` block). Populated by the [`def_map`] Salsa query,
//! which consumes the CST from [`fossil_syntax::parse`].
//!
//! # There is no prefix table, and its absence is the point
//!
//! A shape name, a property key, an expression and an interpolation hole are
//! all bare names or full IRIs in strings, so there is nothing to expand. A
//! mapping's SHAPE resolves through [`DefMap::lookup_type`]: the header names
//! one of the bare names a `type { … } := …` binding introduced, and that
//! binding knows the shape IRI. The resolution is a lookup against a document
//! the program names, not a text substitution the program spelled out — and
//! naming one is MANDATORY, because a bare name has nowhere else to come from.
//! It is why a shape name can be MISSPELT and reported rather than expanded
//! into an IRI that happens to denote nothing.
//!
//! Mapping and source locations are interned with a `'db` lifetime so
//! downstream queries can be keyed per-item (rust-analyzer's per-item Salsa
//! fan-out): one mapping's edit does not re-run another's queries.
//!
//! # Storage choice
//!
//! The source and type tables are insertion-ordered `Vec`s rather than the
//! originally-planned `IndexMap<K, V>`. The workspace
//! `indexmap = "2"` pin does not implement [`std::hash::Hash`] on `IndexMap`,
//! and Salsa tracked-struct fields require Hash for memoisation. Insertion
//! order is preserved either way; callers that need O(1) lookup can build a
//! `HashMap` at the call site (or use the [`DefMap::lookup_source`] helper
//! added below).

use fossil_base::SourceFile;
use fossil_graph_schema::Span;
use fossil_syntax::{SyntaxKind, SyntaxNode};
use smol_str::SmolStr;

/// The `MAPPING` nodes under `root`, in written order: the index space
/// [`MappingLoc::index`] counts in. A `type` or `:=` binding above a mapping
/// is not counted, and every walk from an index to a node, or back, goes
/// through here so that no two of them can count differently.
pub fn mapping_nodes(root: &SyntaxNode) -> impl Iterator<Item = SyntaxNode> {
    root.children().filter(|n| n.kind() == SyntaxKind::MAPPING)
}

/// The [`MappingLoc`] of `node`, a `MAPPING` node of `file`'s tree.
#[must_use]
pub fn mapping_of<'db>(
    db: &'db dyn fossil_base::Db,
    file: SourceFile,
    node: &SyntaxNode,
) -> Option<MappingLoc<'db>> {
    let root = fossil_syntax::parse(db, file).root(db).syntax();
    let index = mapping_nodes(&root).position(|n| &n == node)?;
    def_map(db, file).mappings(db).get(index).copied()
}

#[salsa::interned(debug)]
pub struct MappingLoc<'db> {
    #[returns(copy)]
    pub file: SourceFile,
    #[returns(copy)]
    pub index: usize,
}

#[salsa::interned(debug)]
pub struct SourceLoc<'db> {
    #[returns(copy)]
    pub file: SourceFile,
    #[returns(copy)]
    pub index: usize,
}

/// Why a destructuring member bound no shape.
///
/// Four causes, separated because the reader cannot act on a message that
/// names the wrong thing: collapsed into one `None`, an unreadable file would
/// report that the member's NAME "matches no shape in the schema".
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::SalsaValue)]
pub enum ShapeBindError {
    /// The constructor carries no `schema = …`, so there is no document.
    NoSchema,
    /// The document is named and could not be read as one — nothing is
    /// registered at the path, the provider refused it, or its decoder rejected
    /// it. The problem is `crate::shapes::decoded_document`'s, kept typed.
    Document(fossil_graph_schema::Problem),
    /// The member names a local name no shape of the document has. `declared`
    /// is the local names it does have, in declaration order — the list the
    /// author picks from.
    NotInDocument {
        shape: SmolStr,
        declared: Vec<SmolStr>,
    },
    /// Two shapes of the document share the member's local name. Both IRIs,
    /// because the repair — declaring the type — has to quote one exactly.
    Ambiguous {
        shape: SmolStr,
        first: SmolStr,
        second: SmolStr,
    },
    /// A declared type with no `@iri`. The parser has said so at the
    /// declaration; this is what a use of the name is told.
    NoIri,
}

impl ShapeBindError {
    /// The problem a use of `name` reports — `document` is the one the binding
    /// names, when it names one. The one mapping from cause to code, shared by
    /// the type side ([`crate::lower`]) and the source side ([`crate::infer`]).
    #[must_use]
    pub fn problem(&self, name: &str, document: Option<&str>) -> fossil_graph_schema::Problem {
        use fossil_graph_schema::Problem;
        let document = document.unwrap_or_default().to_string();
        match self {
            Self::NoSchema => Problem::NoDocument {
                binding: Some(name.to_string()),
            },
            Self::Document(problem) => problem.clone(),
            Self::NotInDocument { shape, declared } => Problem::NotInDocument {
                name: shape.to_string(),
                document,
                declared: declared.iter().map(ToString::to_string).collect(),
            },
            Self::Ambiguous {
                shape,
                first,
                second,
            } => Problem::AmbiguousImport {
                name: shape.to_string(),
                document,
                first: first.to_string(),
                second: second.to_string(),
            },
            Self::NoIri => Problem::MissingIri {
                name: name.to_string(),
            },
        }
    }
}

/// What a source binding's right-hand side says: which constructor, over which
/// document, with which reader option.
///
/// One value and not three lookups, because the three are only meaningful
/// together — a `delimiter` belongs to the row `constructor` names, and a URI
/// read without the constructor is the defect `check_provider` exists to stop.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SourceCall {
    /// The dotted constructor — `io.csv`. See [`SourceEntry::constructor`].
    pub constructor: Option<SmolStr>,
    /// The first positional string — the source URI. See [`SourceEntry::uri`].
    pub uri: Option<SmolStr>,
    /// The reader option the row declares, if the header wrote one. See
    /// [`SourceEntry::delimiter`].
    pub delimiter: Option<SmolStr>,
}

/// One entry in the source-binding table (`users := io.csv(...)`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::SalsaValue)]
pub struct SourceEntry<'db> {
    pub name: SmolStr,
    pub loc: SourceLoc<'db>,
    /// The DOCUMENT named by the `schema =` argument — the path inside the
    /// provider call: `{ A, B } := io.rdf("g.ttl", schema = io.shex("x.shex"))`
    /// gives `x.shex`.
    ///
    /// [`crate::infer::resolve_source_scope`] reads it to type the source's rows
    /// forward. It is a SIGNATURE-only datum:
    /// it is read from the `SOURCE_DEF` header tokens, NOT from any mapping
    /// body, so it does NOT widen the per-mapping `body()` fan-out. The
    /// `def_map` query is file-keyed and structurally stable across
    /// body-only edits (verified by `tests/invalidation_regression.rs`).
    pub schema_arg: Option<SmolStr>,
    /// The PROVIDER that argument names — `io.shex`, `io.shacl`.
    ///
    /// `schema =` takes a provider call, not a bare path, so there is one rule
    /// — where there is a document, there is a row that names it — and `None`
    /// here means the program wrote a bare string, which
    /// `crate::lower::check_provider` reports with a span.
    pub schema_provider: Option<SmolStr>,
    /// For a destructuring RDF source member (`{ IfcBeam, ... } := io.rdf(...,
    /// schema = io.shex("x.shex"))`), the shape IRI this member's local-name resolves to
    /// in the schema (`IfcBeam` ↔ `http://ifcowl.../IfcBeam`), resolved at
    /// COMPILE TIME against the document. Subject selection is always by `rdf:type ==
    /// shape_iri`. `None` for plain single-binding native sources (csv/json/…).
    pub shape_iri: Option<SmolStr>,
    /// Set iff this is a destructuring member and [`Self::shape_iri`] is `None`
    /// — why it is `None`. `None` here for every non-member binding.
    pub shape_error: Option<ShapeBindError>,
    /// Dotted name of the source constructor (`io.csv` / `io.json` /
    /// `io.parquet`), if a call-shaped RHS could be parsed. The
    /// constructor name selects the source FORMAT downstream
    /// (`fossil-mir::lower` maps it to `SourceFormat`). Like [`Self::schema_arg`]
    /// this is a SIGNATURE-only `SOURCE_DEF`-header datum.
    pub constructor: Option<SmolStr>,
    /// The first POSITIONAL string argument of the constructor — the source
    /// URI (`io.csv("examples/users.csv")` → `examples/users.csv`). Resolved by
    /// `fossil-mir::lower` into `Op::Source.uri`. Signature-only.
    pub uri: Option<SmolStr>,
    /// The READER OPTION the constructor's row declares — `io.csv("u.csv",
    /// delimiter = "|")` → `|`.
    ///
    /// **`None` means absent and never a default.** Every reader downstream
    /// receives `Option` and supplies its own answer for `None`
    /// (`read_csv_auto` sniffs, `DataFusion` assumes a comma), which is what
    /// keeps `catalogue.bnf` from having to state a default the two engines do
    /// not share.
    ///
    /// **Which rows accept one, and what it is called, are the CATALOGUE's** —
    /// `reader_option_of` reads them off the row's signature rather than
    /// spelling `delimiter` here. A `delimiter =` written on `io.json`, whose
    /// row declares no such position, is therefore not read at all rather than
    /// read and dropped somewhere further down.
    ///
    /// Signature-only, like [`Self::uri`]: it comes off the `SOURCE_DEF` header
    /// tokens and widens no per-mapping fan-out.
    pub delimiter: Option<SmolStr>,
    /// The whole `User := io.csv("users.csv")` item, FILE-ABSOLUTE — where to
    /// point when a diagnostic is about the row this binding introduced rather
    /// than about the line that read it.
    ///
    /// `User.nmae` is blamed at the property, and the second half of the
    /// sentence — which fields `User` actually has — belongs at the binding
    /// that answers it. That is a label with its own `SpanFrame`
    /// ([`fossil_base::SpanLabel`]), riding inside a mapping-relative
    /// diagnostic.
    ///
    /// **It is recorded HERE, and not looked up when the diagnostic is
    /// emitted.** A per-mapping query that reached for `parse(db, file)` to
    /// find it would tie every mapping's type-check to the whole-file CST and
    /// break the fan-out barrier `crate::body::mapping_cst_node` exists to
    /// keep (`tests/invalidation_regression.rs`). `def_map` is file-keyed,
    /// already walking these nodes, and structurally stable across body-only
    /// edits — the same argument [`Self::schema_arg`] makes.
    pub span: Span,
    /// The bound NAME alone — `User` in `User := io.csv("users.csv")`, and each
    /// member's own name in `{ A, B } := …`. It is where an editor lands on the
    /// definition: [`Self::span`] is the whole binding, which in a destructuring
    /// is shared by every member and so cannot tell `A` from `B`.
    pub name_span: Span,
    /// The call right of `:=` — `io.csv("u.csv")` in `users := io.csv("u.csv")`
    /// — where a problem with the source itself, rather than with its name, is
    /// underlined. The whole binding when the right-hand side did not parse.
    pub call_span: Span,
}

/// One type name the program has — imported (`type { Place as City } :=
/// io.shex("s.shex")`) or declared (`@iri("…") type City`).
///
/// The value side of the language binds sources; this binds TYPES. One
/// catalogue (`io.*`) and ONE binder: `:=` binds a NAME, and the left-hand side
/// says what kind of name it is (grammar.bnf, DEFINE). Types had their own `=`
/// for a while and lost it: `=` assigns a value, and a type binding assigns
/// nothing.
///
/// A declared type is a row of the same table, so a mapping header, an edge and
/// a field's `Ref<…>` resolve both kinds by one lookup and never learn which
/// they got (`/docs/design/declared`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::SalsaValue)]
pub struct TypeEntry {
    /// The name the program uses: the declared name, or an import's — the
    /// shape's local name, or the one after `as`.
    pub name: SmolStr,
    /// The shape IRI this name binds: the declaration's `@iri`, or the one shape
    /// of the document whose local name the import wrote.
    pub shape_iri: Option<SmolStr>,
    /// Why it bound nothing, when it bound nothing. Never a bare `None`.
    pub shape_error: Option<ShapeBindError>,
    /// `true` for a type the program declares; its shape is
    /// [`crate::lower::HirFile::declared`]'s, and it names no document.
    pub declared: bool,
    /// The document the constructor names — `io.shex("personas.shex")`. `None`
    /// for a declared type, which is its own document.
    pub document: Option<SmolStr>,
    /// The constructor — `io.shex`, `io.shacl`. It is the NAME that selects the
    /// registry row; dispatching on the document's extension instead would make
    /// `io.shex("x.ttl")` and `io.shacl("x.ttl")` the same program.
    pub constructor: Option<SmolStr>,
    /// The whole `type { Person } := io.shex("shape.shex")` item, or the whole
    /// declaration, file-absolute and trivia-trimmed — [`SourceEntry::span`]'s
    /// twin, and the same [`item_span`].
    ///
    /// A diagnostic about the SHAPE rather than about one property belongs
    /// here: two predicates that collide make the shape unwritable, and the
    /// binding is where the reader chose it.
    pub span: Span,
    /// The name alone, where the program writes it — `City` in
    /// `type { Place as City } := …`, `Person` in `type Person`.
    /// [`SourceEntry::name_span`]'s twin.
    pub name_span: Span,
}

/// A mapping's name and where it is written — `Users` in
/// `Users : Person from Adults`.
///
/// Beside [`DefMap::mappings`] rather than in it: a mapping is keyed by its
/// [`MappingLoc`] everywhere a query reads it, and a header that has no name yet
/// is still a mapping with a body to check. This table holds only the headers
/// that name one.
///
/// It holds no position: an edit to one mapping's body moves every mapping
/// after it, and a position here would change the def map on that edit and
/// re-lower every sibling. Where a mapping is written is read off the CST when
/// it is asked for ([`MappingName::spans`]), as rust-analyzer keeps its item
/// tree apart from its source map.
#[derive(Debug, Clone, PartialEq, Eq, Hash, salsa::SalsaValue)]
pub struct MappingName<'db> {
    /// The name the header writes.
    pub name: SmolStr,
    /// The mapping it names.
    pub loc: MappingLoc<'db>,
}

impl MappingName<'_> {
    /// The whole mapping, header and body, file-absolute and trivia-trimmed,
    /// and its name token alone.
    #[must_use]
    pub fn spans(&self, db: &dyn fossil_base::Db) -> Option<(Span, Span)> {
        let file = self.loc.file(db);
        let root = fossil_syntax::parse(db, file).root(db).syntax();
        let item = mapping_nodes(&root).nth(self.loc.index(db))?;
        let header = item
            .children()
            .find(|c| c.kind() == SyntaxKind::MAPPING_HEADER)?;
        Some((item_span(&item), token_span(&first_ident(&header)?)))
    }
}

/// Which table a written name is looked up in.
///
/// The grammar decides it by position and never by spelling: a header's shape
/// and a field's type name a TYPE; a binding's own name, a `from` and the head
/// of an expression name a VALUE; and a mapping's name is written once, in its
/// header, and read nowhere else. One name may be a value and a type at once —
/// `{ Person } := io.rdf(…)` beside `type { Person } := …` — and the position
/// is what says which is meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Namespace {
    /// A `:=` source binding — a read or a derivation.
    Value,
    /// A type, imported or declared.
    Type,
    /// A mapping header's name.
    Mapping,
}

/// A name the program binds at the top level, as [`DefMap::resolve`] answers
/// it: the one resolver every editor feature asks.
///
/// Each variant borrows the row of the table it came from, so a reader that
/// needs more than the name and its place reads the row; the methods here are
/// what every kind has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Definition<'db> {
    /// A `:=` binding.
    Source(&'db SourceEntry<'db>),
    /// A type the program imports or declares.
    Type(&'db TypeEntry),
    /// A mapping.
    Mapping(&'db MappingName<'db>),
}

impl<'db> Definition<'db> {
    /// The name the program binds.
    #[must_use]
    pub const fn name(self) -> &'db SmolStr {
        match self {
            Self::Source(e) => &e.name,
            Self::Type(e) => &e.name,
            Self::Mapping(e) => &e.name,
        }
    }

    /// The table it was found in.
    #[must_use]
    pub const fn namespace(self) -> Namespace {
        match self {
            Self::Source(_) => Namespace::Value,
            Self::Type(_) => Namespace::Type,
            Self::Mapping(_) => Namespace::Mapping,
        }
    }

    /// The whole item that binds it, file-absolute.
    #[must_use]
    pub fn span(self, db: &dyn fossil_base::Db) -> Span {
        match self {
            Self::Source(e) => e.span,
            Self::Type(e) => e.span,
            Self::Mapping(e) => e.spans(db).map_or(Span::new(0, 0), |(item, _)| item),
        }
    }

    /// The name alone, where the program writes it.
    #[must_use]
    pub fn name_span(self, db: &dyn fossil_base::Db) -> Span {
        match self {
            Self::Source(e) => e.name_span,
            Self::Type(e) => e.name_span,
            Self::Mapping(e) => e.spans(db).map_or(Span::new(0, 0), |(_, name)| name),
        }
    }

    /// Where the definition is DECLARED: the program's own name for everything
    /// the program declares, and the document's declaration for an imported
    /// type.
    ///
    /// An imported type is declared in the document it names, and the
    /// resolved type knows which file that is
    /// ([`crate::shapes::ResolvedShape::file`]), so this asks it rather than
    /// branching on the kind of type. When it cannot be resolved — the
    /// document is not registered, or does not declare the name — the binding
    /// that introduced the name is the declaration there is.
    #[must_use]
    pub fn declaration(self, db: &'db dyn fossil_base::Db, file: SourceFile) -> Declaration {
        let local = Declaration {
            file,
            span: Some(self.name_span(db)),
        };
        let Self::Type(entry) = self else {
            return local;
        };
        crate::shapes::resolve_type(db, file, entry)
            .and_then(|shape| {
                Some(Declaration {
                    file: shape.file?,
                    span: shape.span,
                })
            })
            .unwrap_or(local)
    }
}

/// Where a [`Definition`] is declared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Declaration {
    /// The file that holds it — the program, or a document it names.
    pub file: SourceFile,
    /// The declaration's span in that file. `None` when the file is known and
    /// the decoder kept no span for what is in it (`ShExJ`, SHACL).
    pub span: Option<Span>,
}

#[salsa::tracked(debug)]
pub struct DefMap<'db> {
    #[returns(ref)]
    pub sources: Vec<SourceEntry<'db>>,
    #[returns(ref)]
    pub mappings: Vec<MappingLoc<'db>>,
    #[returns(ref)]
    pub types: Vec<TypeEntry>,
    /// Every mapping whose header names one, in written order. A field of its
    /// own so the queries that read [`Self::mappings`] do not depend on it.
    #[returns(ref)]
    pub mapping_names: Vec<MappingName<'db>>,
}

impl<'db> DefMap<'db> {
    /// The definition `name` has in `namespace`, or `None` when the program
    /// binds no such name there.
    ///
    /// The first binding in written order wins, as it does for every lookup
    /// below: a name bound twice is reported where it is bound, and an editor
    /// that answered with both would be guessing which the checker read.
    #[must_use]
    pub fn resolve(
        self,
        db: &'db dyn fossil_base::Db,
        namespace: Namespace,
        name: &str,
    ) -> Option<Definition<'db>> {
        match namespace {
            Namespace::Value => self
                .sources(db)
                .iter()
                .find(|e| e.name.as_str() == name)
                .map(Definition::Source),
            Namespace::Type => self
                .types(db)
                .iter()
                .find(|e| e.name.as_str() == name)
                .map(Definition::Type),
            Namespace::Mapping => self
                .mapping_names(db)
                .iter()
                .find(|e| e.name.as_str() == name)
                .map(Definition::Mapping),
        }
    }

    /// Every top-level definition, in the order the file writes them.
    #[must_use]
    pub fn definitions(self, db: &'db dyn fossil_base::Db) -> Vec<Definition<'db>> {
        let mut all: Vec<Definition<'db>> = self
            .sources(db)
            .iter()
            .map(Definition::Source)
            .chain(self.types(db).iter().map(Definition::Type))
            .chain(self.mapping_names(db).iter().map(Definition::Mapping))
            .collect();
        all.sort_by_key(|d| (d.span(db).start, d.name_span(db).start));
        all
    }

    /// Look up the [`SourceLoc`] bound to a source name (e.g. `users`).
    #[must_use]
    pub fn lookup_source(self, db: &'db dyn fossil_base::Db, name: &str) -> Option<SourceLoc<'db>> {
        self.sources(db)
            .iter()
            .find(|e| e.name.as_str() == name)
            .map(|e| e.loc)
    }

    /// Where a source name was bound — [`SourceEntry::span`], file-absolute.
    #[must_use]
    pub fn lookup_source_span(self, db: &'db dyn fossil_base::Db, name: &str) -> Option<Span> {
        self.sources(db)
            .iter()
            .find(|e| e.name.as_str() == name)
            .map(|e| e.span)
    }

    /// Where a TYPE name was bound — [`TypeEntry::span`], file-absolute.
    #[must_use]
    pub fn lookup_type_span(self, db: &'db dyn fossil_base::Db, name: &str) -> Option<Span> {
        self.types(db)
            .iter()
            .find(|e| e.name.as_str() == name)
            .map(|e| e.span)
    }

    /// The `(provider, document)` pair a source's `schema =` argument names —
    /// `schema = io.shex("x.shex")`.
    ///
    /// A pair, because the provider is what selects the row that reads the document, and handing
    /// them out separately is how a caller comes to read one and forget the
    /// other. That is not hypothetical here — `crate::infer` did exactly that
    /// and passed `None` for the provider, which turned every destructuring RDF
    /// source into "the document is named by no provider".
    #[must_use]
    pub fn lookup_source_schema_binding(
        self,
        db: &'db dyn fossil_base::Db,
        name: &str,
    ) -> Option<(Option<SmolStr>, SmolStr)> {
        self.sources(db)
            .iter()
            .find(|e| e.name.as_str() == name)
            .and_then(|e| e.schema_arg.clone().map(|d| (e.schema_provider.clone(), d)))
    }

    /// Look up the resolved shape IRI bound to a destructuring RDF source member
    /// (`{ IfcBeam, ... } := io.rdf(..., schema = io.shex("x.shex"))` → `IfcBeam`'s shape
    /// IRI). `None` for native single-binding sources. Used by the input typing
    /// ([`crate::infer::resolve_source_scope`]) to resolve the member's
    /// compile-time row type from the declared shape.
    #[must_use]
    pub fn lookup_source_shape_iri(
        self,
        db: &'db dyn fossil_base::Db,
        name: &str,
    ) -> Option<SmolStr> {
        self.sources(db)
            .iter()
            .find(|e| e.name.as_str() == name)
            .and_then(|e| e.shape_iri.clone())
    }

    /// Why a destructuring member bound no shape. `None` when it bound one, or
    /// when the binding never named a shape at all.
    #[must_use]
    pub fn lookup_source_shape_error(
        self,
        db: &'db dyn fossil_base::Db,
        name: &str,
    ) -> Option<ShapeBindError> {
        self.sources(db)
            .iter()
            .find(|e| e.name.as_str() == name)
            .and_then(|e| e.shape_error.clone())
    }

    /// Look up the constructor call bound to a source name (e.g. `users` →
    /// `io.csv("examples/users.csv")`). `fossil-mir::lower` reads it to resolve
    /// `Op::Source`'s format + URI from the binding. Any component is `None`
    /// when the `SOURCE_DEF` RHS is not a recognisable `io.*("...")` call.
    ///
    /// One [`SourceCall`], for the reason
    /// [`Self::lookup_source_schema_binding`] is a pair rather than two
    /// lookups: the delimiter is meaningless without the constructor that
    /// declares it, and a caller handed them separately is a caller who can
    /// read one and forget the other.
    #[must_use]
    pub fn lookup_source_call(
        self,
        db: &'db dyn fossil_base::Db,
        name: &str,
    ) -> Option<SourceCall> {
        self.sources(db)
            .iter()
            .find(|e| e.name.as_str() == name)
            .map(|e| SourceCall {
                constructor: e.constructor.clone(),
                uri: e.uri.clone(),
                delimiter: e.delimiter.clone(),
            })
    }

    /// The shape IRI a type name binds, declared or imported.
    #[must_use]
    pub fn lookup_type(self, db: &'db dyn fossil_base::Db, name: &str) -> Option<SmolStr> {
        self.types(db)
            .iter()
            .find(|e| e.name.as_str() == name)
            .and_then(|e| e.shape_iri.clone())
    }

    /// Why a type name bound nothing. `None` when it bound a shape.
    #[must_use]
    pub fn lookup_type_error(
        self,
        db: &'db dyn fossil_base::Db,
        name: &str,
    ) -> Option<ShapeBindError> {
        self.types(db)
            .iter()
            .find(|e| e.name.as_str() == name)
            .and_then(|e| e.shape_error.clone())
    }

    /// The entry that introduced `shape_iri` — what a mapping targeting that
    /// shape is checked against: the declaration, or the document of the import.
    ///
    /// **By TYPE, not by file.** This was a `find_map` over the type bindings
    /// returning the FIRST document any of them named. With one document per
    /// program the two answers coincide; with two they do not, and
    /// `docs/programs/multi-document` — `Persona` from `es.shex`, `Human` from
    /// `en.shex`, disjoint — resolved its second mapping against the first
    /// document and reported *«`es.shex` declares no shape
    /// `https://council.example/voc#Person`»*, which is true and is not the
    /// mistake.
    #[must_use]
    pub fn type_of_shape(
        self,
        db: &'db dyn fossil_base::Db,
        shape_iri: &str,
    ) -> Option<&'db TypeEntry> {
        self.types(db)
            .iter()
            .find(|e| e.shape_iri.as_deref() == Some(shape_iri))
    }
}

#[salsa::tracked(returns(copy))]
#[allow(clippy::elidable_lifetime_names)] // explicit 'db documents the locked query surface
pub fn def_map<'db>(db: &'db dyn fossil_base::Db, file: SourceFile) -> DefMap<'db> {
    let cst = fossil_syntax::parse(db, file);
    let mut sources: Vec<SourceEntry<'db>> = Vec::new();
    let mut mappings: Vec<MappingLoc<'db>> = Vec::new();
    let mut types: Vec<TypeEntry> = Vec::new();
    let mut mapping_names: Vec<MappingName<'db>> = Vec::new();

    // Per-kind dense indices: `mapping_idx` counts what [`mapping_nodes`]
    // yields, and SOURCE_DEF follows the same scheme.
    let mut mapping_idx = 0usize;
    let mut source_idx = 0usize;
    for item in cst.root(db).syntax().children() {
        use fossil_syntax::SyntaxKind;
        match item.kind() {
            SyntaxKind::SOURCE_DEF => {
                if let Some(name) = parse_source_name(&item) {
                    let schema = parse_schema_arg(&item);
                    let (constructor, uri) = parse_source_call(&item);
                    let delimiter =
                        parse_reader_option(&item, constructor.as_ref()).and_then(|o| o.value);
                    sources.push(SourceEntry {
                        name,
                        loc: SourceLoc::new(db, file, source_idx),
                        schema_arg: schema.as_ref().and_then(|s| s.document.clone()),
                        schema_provider: schema.as_ref().and_then(|s| s.provider.clone()),
                        shape_iri: None,
                        // A single binding names no shape, so it cannot fail to
                        // bind one. Absence here is not an error.
                        shape_error: None,
                        constructor,
                        uri,
                        delimiter,
                        span: item_span(&item),
                        name_span: first_ident_span(&item).unwrap_or_else(|| item_span(&item)),
                        call_span: call_span(&item),
                    });
                    source_idx += 1;
                }
            }
            SyntaxKind::TYPE_DEF => {
                // `type { Place as City } := io.shex("places.shex")`. Binds
                // TYPES, not sources, so it produces no `SourceEntry` and takes
                // no slot in `source_idx` — nothing downstream may read a type
                // name as a source. The document is the constructor's first
                // positional string, exactly as a source's URI is.
                let members = parse_brace_members(&item);
                let name_spans = brace_member_name_spans(&item);
                let (ctor, document) = parse_source_call(&item);
                let bound = resolve_member_shape_iris(
                    db,
                    file,
                    ctor.as_deref(),
                    document.as_deref(),
                    &members,
                );
                for (((_, name), (shape_iri, shape_error)), name_span) in
                    members.into_iter().zip(bound).zip(name_spans)
                {
                    types.push(TypeEntry {
                        name,
                        shape_iri,
                        shape_error,
                        declared: false,
                        document: document.clone(),
                        constructor: ctor.clone(),
                        // Every name a `type { A, B }` line binds shares the
                        // line's span, because they share the binding — the
                        // same answer the destructuring source gives.
                        span: item_span(&item),
                        name_span,
                    });
                }
            }
            SyntaxKind::TYPE_DECL => {
                // `@iri("https://schema.org/City") type City`. The IRI is the
                // one written, and the fields are `crate::declared`'s: this
                // table holds what a NAME resolves to, which is all a header,
                // an edge or a `Ref<…>` asks of it.
                let Some(decl) = fossil_syntax::ast::TypeDecl::cast(item.clone()) else {
                    continue;
                };
                let Some(name) = decl.name() else {
                    continue;
                };
                let name_span = decl
                    .name_token()
                    .map_or_else(|| item_span(&item), |t| token_span(&t));
                let shape_iri = decl.iri_attr().and_then(|a| a.iri());
                types.push(TypeEntry {
                    name,
                    shape_error: shape_iri.is_none().then_some(ShapeBindError::NoIri),
                    shape_iri,
                    declared: true,
                    document: None,
                    constructor: None,
                    span: item_span(&item),
                    name_span,
                });
            }
            SyntaxKind::MULTI_SOURCE_DEF => {
                // `{ A, B, ... } := io.rdf(uri, schema = io.shex("x.shex"))`. Each member
                // becomes its own `SourceEntry` sharing the one source's URI +
                // constructor + schema; the member's local-name resolves to a
                // shape IRI in the schema (COMPILE TIME). Each member then lowers
                // to its own `Op::Source` and `from <member>` resolves uniformly
                // via the existing per-source lookups.
                let members = parse_brace_members(&item);
                let name_spans = brace_member_name_spans(&item);
                let schema = parse_schema_arg(&item);
                let schema_arg = schema.as_ref().and_then(|s| s.document.clone());
                let schema_provider = schema.as_ref().and_then(|s| s.provider.clone());
                let (constructor, uri) = parse_source_call(&item);
                let delimiter =
                    parse_reader_option(&item, constructor.as_ref()).and_then(|o| o.value);
                // The PROVIDER the argument names, which is what selects the row
                // that reads the document. `None` is a program that wrote a bare
                // string, and it is an error rather than a fallback to the
                // extension — `crate::lower::check_provider` reports it.
                let shape_iris = resolve_member_shape_iris(
                    db,
                    file,
                    schema_provider.as_deref(),
                    schema_arg.as_deref(),
                    &members,
                );
                for (((_, member), (shape_iri, shape_error)), name_span) in
                    members.into_iter().zip(shape_iris).zip(name_spans)
                {
                    sources.push(SourceEntry {
                        name: member,
                        loc: SourceLoc::new(db, file, source_idx),
                        schema_arg: schema_arg.clone(),
                        schema_provider: schema_provider.clone(),
                        shape_iri,
                        shape_error,
                        constructor: constructor.clone(),
                        uri: uri.clone(),
                        delimiter: delimiter.clone(),
                        // Every member of `{ A, B } := io.rdf(…)` shares the
                        // binding's span, because they share the binding; the
                        // member's own name is `name_span`.
                        span: item_span(&item),
                        name_span,
                        call_span: call_span(&item),
                    });
                    source_idx += 1;
                }
            }
            SyntaxKind::MAPPING => {
                let loc = MappingLoc::new(db, file, mapping_idx);
                mappings.push(loc);
                if let Some(header) =
                    fossil_syntax::ast::Mapping::cast(item.clone()).and_then(|m| m.header())
                    && let Some(token) = first_ident(header.syntax())
                {
                    mapping_names.push(MappingName {
                        name: SmolStr::from(token.text()),
                        loc,
                    });
                }
                mapping_idx += 1;
            }
            _ => {}
        }
    }

    DefMap::new(db, sources, mappings, types, mapping_names)
}

/// The first `IDENT` directly under `node` — the bound name of a `SOURCE_DEF`
/// and of a `MAPPING_HEADER`, which both start with it.
fn first_ident(node: &SyntaxNode) -> Option<fossil_syntax::SyntaxToken> {
    node.children_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .find(|t| t.kind() == SyntaxKind::IDENT)
}

fn first_ident_span(node: &SyntaxNode) -> Option<Span> {
    first_ident(node).map(|t| token_span(&t))
}

fn token_span(token: &fossil_syntax::SyntaxToken) -> Span {
    let r = token.text_range();
    Span::new(u32::from(r.start()), u32::from(r.end()))
}

/// The name span of each member [`parse_brace_members`] returns, in the same
/// order and under the same filter, so the two zip.
fn brace_member_name_spans(node: &SyntaxNode) -> Vec<Span> {
    node.children()
        .filter_map(fossil_syntax::ast::ImportMember::cast)
        .filter(|m| m.shape_name().is_some())
        .filter_map(|m| m.name_token())
        .map(|t| token_span(&t))
        .collect()
}

/// Extract the bound name from a `SOURCE_DEF` node (the `users` in
/// `users := io.csv("...")`). The first IDENT child token is the binding name;
/// IDENTs nested inside the call expression belong to the callee.
/// The file-absolute [`Span`] of a top-level item, **trivia trimmed**.
///
/// `def_map` walks `cst.root(db)`, which is the whole file, so `text_range()`
/// here is already absolute — unlike the per-mapping subtrees rowan resets to
/// zero (`crate::spans`'s «offset semantics» section).
///
/// The trim is not cosmetic. A node's range carries the trailing newline, the
/// blank line after it and any comment in between, so an untrimmed span ends on
/// a LATER line and miette draws the label as a multi-line block — `╭─▶` down
/// the gutter, with the caret under an empty line. Measured twice: on
/// the native CLI's `broken_field` golden, and on `compound-key` with its join
/// condition broken, where the `// #endregion on` comment and the blank line
/// after it both ended up underlined.
///
/// Shared with [`crate::lower::lower_pipe_expr`] for that second one:
/// `HirSourcePipe::span` is «the whole `name := …` item» by the same
/// definition, and two implementations of one trim are two answers.
pub(crate) fn item_span(node: &fossil_syntax::SyntaxNode) -> Span {
    use fossil_syntax::SyntaxKind;

    // By TOKEN and not by trimming the text, because a comment is trivia and is
    // not whitespace. `Joined := LineRow.join(…)` followed by `// #endregion on`
    // carries that comment inside its own range, so trimming characters left it
    // underlined — `compound-key`, measured.
    let mut tokens = node
        .descendants_with_tokens()
        .filter_map(fossil_syntax::SyntaxElement::into_token)
        .filter(|t| {
            !matches!(
                t.kind(),
                SyntaxKind::WHITESPACE | SyntaxKind::NEWLINE | SyntaxKind::COMMENT
            )
        })
        .map(|t| t.text_range());
    let Some(first) = tokens.next() else {
        // Nothing but trivia. The node's own range is the only answer left, and
        // a caller that underlines it is no worse off than before.
        let range = node.text_range();
        return Span::new(u32::from(range.start()), u32::from(range.end()));
    };
    let last = tokens.last().unwrap_or(first);
    Span::new(u32::from(first.start()), u32::from(last.end()))
}

fn parse_source_name(node: &fossil_syntax::SyntaxNode) -> Option<SmolStr> {
    first_ident(node).map(|t| SmolStr::from(t.text()))
}

/// The `schema = <provider call>` argument of a source binding.
///
/// `{ A, B } := io.rdf("g.ttl", schema = io.shex("x.shex"))` →
/// `provider: "io.shex"`, `document: "x.shex"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SchemaArg {
    /// The dotted callee — `io.shex`, `io.shacl`. `None` when the value is not
    /// a call at all, which is the bare-string spelling and an error.
    pub provider: Option<SmolStr>,
    /// The document path the call names.
    pub document: Option<SmolStr>,
    /// The VALUE's own span, so the diagnostic underlines what was written and
    /// not the whole binding. This struct exists for that: the checking side
    /// (`crate::lower`) reads the argument through here and nowhere else.
    pub span: fossil_graph_schema::Span,
}

/// Extract the `schema =` argument from a `SOURCE_DEF` / `MULTI_SOURCE_DEF`
/// node's call expression, if it has one.
///
/// This reads ONLY the header tokens (the call expression on the right of
/// `:=`), never any mapping body — so it stays signatures-only and does not
/// widen the per-mapping `body()` fan-out, which is what keeps
/// [`crate::infer::resolve_source_scope`] out of `parse(file)`.
///
/// # It takes a provider call, not a bare string
///
/// A bare `schema = "x.shex"` names a document and no provider, so the decoder
/// would come from the document's own EXTENSION — a dispatch that does not go
/// by name. The value is read by the same scan as any other call
/// (`io` `.` `shex` `(` `"…"` `)`), so a document is never named without the
/// row that reads it. A value that is not a call yields `provider: None`, which
/// is reported with the span below rather than quietly falling back.
///
pub(crate) fn parse_schema_arg(node: &SyntaxNode) -> Option<SchemaArg> {
    let value = Call::of_binding(node)?.named("schema")?.value?;
    let (provider, document) = Call::of(&value).map_or_else(
        || (None, string_literal(&value)),
        |call| (call.callee(), call.positional_string()),
    );
    Some(SchemaArg {
        provider,
        document,
        span: span_of(value.text_range()),
    })
}

/// A call as the parser built it — `callee(arg, …, name = value, …)`, a
/// `POSTFIX_EXPR` holding an `ARG_LIST`.
struct Call {
    node: SyntaxNode,
}

/// One `name = value` of a [`Call`].
struct NamedArg {
    name: fossil_syntax::SyntaxToken,
    value: Option<SyntaxNode>,
}

impl Call {
    /// `expr` as a call, if it is one.
    fn of(expr: &SyntaxNode) -> Option<Self> {
        let node = unwrap_expr(expr.clone())?;
        node.children()
            .any(|c| c.kind() == SyntaxKind::ARG_LIST)
            .then_some(Self { node })
    }

    /// The call on the right of a binding's `:=`.
    fn of_binding(binding: &SyntaxNode) -> Option<Self> {
        Self::of(&binding_rhs(binding)?)
    }

    /// The dotted callee — `io.csv`.
    fn callee(&self) -> Option<SmolStr> {
        let text: String = self
            .node
            .first_child()?
            .descendants_with_tokens()
            .filter_map(fossil_syntax::SyntaxElement::into_token)
            .filter(|t| matches!(t.kind(), SyntaxKind::IDENT | SyntaxKind::DOT))
            .map(|t| t.text().to_owned())
            .collect();
        (!text.is_empty()).then(|| SmolStr::from(text))
    }

    fn args(&self) -> impl Iterator<Item = SyntaxNode> {
        self.node
            .children()
            .find(|c| c.kind() == SyntaxKind::ARG_LIST)
            .into_iter()
            .flat_map(|list| list.children())
    }

    /// The first positional argument, when it is a string literal.
    fn positional_string(&self) -> Option<SmolStr> {
        let arg = self.args().find(|a| a.kind() == SyntaxKind::ARG)?;
        string_literal(&arg.first_child()?)
    }

    /// Every `name = value`, in written order.
    fn named_args(&self) -> impl Iterator<Item = NamedArg> {
        self.args()
            .filter(|a| a.kind() == SyntaxKind::NAMED_ARG)
            .filter_map(|a| {
                Some(NamedArg {
                    name: a
                        .children_with_tokens()
                        .filter_map(fossil_syntax::SyntaxElement::into_token)
                        .find(|t| t.kind() == SyntaxKind::IDENT)?,
                    value: a.children().next(),
                })
            })
    }

    fn named(&self, name: &str) -> Option<NamedArg> {
        self.named_args().find(|a| a.name.text() == name)
    }
}

/// The expression on the right of a binding's `:=`, past its `EXPR` wrappers.
/// [`SourceEntry::call_span`].
fn call_span(binding: &SyntaxNode) -> Span {
    binding_rhs(binding).map_or_else(|| item_span(binding), |rhs| item_span(&rhs))
}

fn binding_rhs(binding: &SyntaxNode) -> Option<SyntaxNode> {
    unwrap_expr(binding.children().find(|c| c.kind() == SyntaxKind::EXPR)?)
}

/// `node` past any `EXPR` wrapper — the node that says what the expression is.
fn unwrap_expr(node: SyntaxNode) -> Option<SyntaxNode> {
    std::iter::successors(Some(node), |n| {
        (n.kind() == SyntaxKind::EXPR)
            .then(|| n.first_child())
            .flatten()
    })
    .find(|n| n.kind() != SyntaxKind::EXPR)
}

/// The contents of a string literal, unquoted; `None` for anything else.
fn string_literal(expr: &SyntaxNode) -> Option<SmolStr> {
    let expr = unwrap_expr(expr.clone())?;
    if expr.kind() != SyntaxKind::LITERAL_EXPR {
        return None;
    }
    let token = expr
        .first_token()
        .filter(|t| t.kind() == SyntaxKind::STRING)?;
    Some(SmolStr::from(
        token.text().trim_start_matches('"').trim_end_matches('"'),
    ))
}

fn span_of(range: rowan::TextRange) -> Span {
    Span::new(range.start().into(), range.end().into())
}

/// Extract the source constructor's dotted callee name and first positional
/// string argument from a `SOURCE_DEF` node:
/// `users := io.csv("examples/users.csv")` → `(Some("io.csv"),
/// Some("examples/users.csv"))`.
///
/// Like [`parse_schema_arg`] this reads ONLY the call on the right of `:=`,
/// never any mapping body, so it is signatures-only and does NOT widen the
/// per-mapping `body()` fan-out (`tests/invalidation_regression.rs`).
///
/// A right-hand side that is not a call — `type { Person } := "person.shex"` —
/// gives no callee and its string as the document, which is what lets the
/// refusal name the document no provider reads.
pub(crate) fn parse_source_call(node: &SyntaxNode) -> (Option<SmolStr>, Option<SmolStr>) {
    let Some(rhs) = binding_rhs(node) else {
        return (None, None);
    };
    Call::of(&rhs).map_or_else(
        || (None, string_literal(&rhs)),
        |call| (call.callee(), call.positional_string()),
    )
}

/// The name of the READER OPTION `constructor`'s catalogue row declares, if it
/// declares one.
///
/// **The name is the catalogue's and is not spelled here.** `catalogue.bnf`
/// writes `io.csv`'s second position as `delimiter = String?` — named, because
/// the call site writes its name, and optional, because absent is a legal
/// program — and this reads that word back off the registry the file generates.
/// Spelling `"delimiter"` in this module would be the same row stated twice, in
/// the one place nothing would compare them.
///
/// `None` for every row that declares no optional position, which is every row
/// but `io.csv`. That is what keeps `io.json("x", delimiter = "|")` from being
/// read at all: a row that has no such parameter has no such argument, and the
/// alternative — reading it and dropping it later — is a silently ignored
/// argument, which is worse than an unread one.
pub(crate) fn reader_option_of(constructor: &str) -> Option<&'static SmolStr> {
    crate::stdlib::stdlib()
        .lookup(constructor)?
        .sig
        .params
        .iter()
        .find(|p| p.named && p.arity == crate::stdlib::Arity::Optional)
        .map(|p| &p.name)
}

/// Every name that IS a reader option of some `io.` row — the set a header's
/// named argument is measured against when the row it was written on declares
/// none of them.
///
/// It exists for one diagnostic: `io.json("x", delimiter = "|")` writes a
/// position `io.json` does not have, and the alternative to naming it is
/// silence. The set is the catalogue's, so a row that grows an option joins it
/// with no edit here.
pub(crate) fn reader_option_names() -> Vec<(&'static SmolStr, &'static SmolStr)> {
    crate::stdlib::stdlib()
        .iter()
        .filter(|e| e.name.starts_with(fossil_base::CONSTRUCTOR_PREFIX))
        .filter_map(|e| {
            e.sig
                .params
                .iter()
                .find(|p| p.named && p.arity == crate::stdlib::Arity::Optional)
                .map(|p| (&p.name, &e.name))
        })
        .collect()
}

/// One reader option written in a source header: what it was called, the string
/// it was given, and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReaderOptionArg {
    /// The name the row declares — `delimiter`.
    pub name: &'static SmolStr,
    /// The literal's contents, or `None` when the value is not a string
    /// literal. `crate::lower::check_reader_option` is what reports the second.
    pub value: Option<SmolStr>,
    /// The value's extent, for the diagnostic. The NAME's extent when there is
    /// no value token at all, so the caret lands on what the author wrote.
    pub span: Span,
}

/// Read the reader option a `SOURCE_DEF` / `MULTI_SOURCE_DEF` header wrote,
/// under the name the CONSTRUCTOR's row declares.
///
/// `None` means the row declares no option, or the header wrote none. A row
/// that declares one and a header that wrote it with a value that is not a
/// string literal gives `Some` with `value: None`, which is a thing to report
/// rather than a thing to skip.
///
/// Signature-only: it reads the header and never a mapping body, so it does
/// not widen the per-mapping `body()` fan-out.
pub(crate) fn parse_reader_option(
    node: &SyntaxNode,
    constructor: Option<&SmolStr>,
) -> Option<ReaderOptionArg> {
    let name = reader_option_of(constructor?)?;
    let arg = Call::of_binding(node)?.named(name)?;
    Some(match arg.value {
        Some(value) => ReaderOptionArg {
            name,
            value: string_literal(&value),
            span: span_of(value.text_range()),
        },
        None => ReaderOptionArg {
            name,
            value: None,
            span: span_of(arg.name.text_range()),
        },
    })
}

/// Every `name = value` the binding's call writes: the name, and where it is.
/// What `crate::lower` measures against the catalogue's reader options.
pub(crate) fn named_arg_names(node: &SyntaxNode) -> Vec<(SmolStr, Span)> {
    Call::of_binding(node).map_or_else(Vec::new, |call| {
        call.named_args()
            .map(|a| (SmolStr::from(a.name.text()), span_of(a.name.text_range())))
            .collect()
    })
}

/// The members a destructuring binding lists, one per `IMPORT_MEMBER`: the
/// shape's local name in the document, and the name the program uses — the one
/// after `as` when there is one, else the same.
///
/// Serves `MULTI_SOURCE_DEF` (`{ A, B } := …`) and `TYPE_DEF`
/// (`type { Place as City } := …`) alike, because both hold the same member
/// list (grammar.bnf, `ImportMember`). A member whose name did not parse
/// introduces nothing.
fn parse_brace_members(node: &fossil_syntax::SyntaxNode) -> Vec<(SmolStr, SmolStr)> {
    node.children()
        .filter_map(fossil_syntax::ast::ImportMember::cast)
        .filter_map(|m| Some((m.shape_name()?, m.name()?)))
        .collect()
}

/// Resolve each destructuring member to its shape IRI in the document, at
/// COMPILE TIME, by the shape's LOCAL NAME — the last segment of its IRI, the
/// rule a property's name follows ([`fossil_graph_schema::local_name`]). A
/// member that binds nothing carries WHY (the caller — lowering or inference —
/// emits the diagnostic; this signatures-only query stays diagnostic-free).
///
/// It bound by POSITION, the Nth name to the Nth declared shape, and
/// reordering a document rebound every program that read it
/// (`/docs/design/declared`). Two shapes whose local names coincide are an
/// error when one of them is imported and nothing otherwise: a document may
/// hold shapes a program never names.
///
/// `constructor` is the provider the binding names (`io.shex`, `io.shacl`), or
/// `None` only when the program wrote a bare path, which is an error rather
/// than a fallback — see [`crate::shapes::decoded_document`].
fn resolve_member_shape_iris(
    db: &dyn fossil_base::Db,
    file: fossil_base::SourceFile,
    constructor: Option<&str>,
    schema_arg: Option<&str>,
    members: &[(SmolStr, SmolStr)],
) -> Vec<(Option<SmolStr>, Option<ShapeBindError>)> {
    let all = |e: &ShapeBindError| {
        members
            .iter()
            .map(|_| (None, Some(e.clone())))
            .collect::<Vec<_>>()
    };
    let Some(schema_path) = schema_arg else {
        return all(&ShapeBindError::NoSchema);
    };
    let document = match crate::shapes::decoded_document(db, file, constructor, schema_path) {
        Ok(d) => d,
        Err(problem) => return all(&ShapeBindError::Document(problem)),
    };

    members
        .iter()
        .map(|(shape, _)| {
            let mut named = document
                .shapes()
                .filter(|s| fossil_graph_schema::local_name(&s.iri) == shape.as_str());
            match (named.next(), named.next()) {
                (Some(only), None) => (Some(SmolStr::from(only.iri.as_str())), None),
                (Some(first), Some(second)) => (
                    None,
                    Some(ShapeBindError::Ambiguous {
                        shape: shape.clone(),
                        first: SmolStr::from(first.iri.as_str()),
                        second: SmolStr::from(second.iri.as_str()),
                    }),
                ),
                (None, _) => {
                    let mut declared: Vec<SmolStr> = Vec::new();
                    for s in document.shapes() {
                        let local = SmolStr::from(fossil_graph_schema::local_name(&s.iri));
                        if !declared.contains(&local) {
                            declared.push(local);
                        }
                    }
                    (
                        None,
                        Some(ShapeBindError::NotInDocument {
                            shape: shape.clone(),
                            declared,
                        }),
                    )
                }
            }
        })
        .collect()
}

// Path resolution is `crate::documents::registry_key`, beside the loop the
// HOSTS call to register the same documents — the key and the lookup are one
// function, which is the only arrangement in which they cannot disagree.

#[cfg(test)]
mod tests {
    use super::*;

    use fossil_base::test_support::{db_with_document, new_db};

    /// Two shapes, in this order, with nothing else in them — alphabetical order
    /// reversed, so a list that came back sorted would show.
    const TWO_SHAPES: &str = "\
shape https://example.org/Zeta
shape https://example.org/Alpha
";

    const HELLO_FOSSIL: &str = "\
type { Person } := io.shex(\"personas.shex\")

User := io.csv(\"examples/users.csv\")

Users : Person from User
    @subject = \"https://example.org/user/{User.id}\"
    name = User.name
";

    fn db_with_hello() -> (fossil_base::FossilDb, fossil_base::SourceFile) {
        let db = new_db();
        let file = fossil_base::SourceFile::new(
            &db,
            HELLO_FOSSIL.to_string(),
            "examples/hello.fossil".to_string(),
        );
        (db, file)
    }

    /// A binding's span is the binding, and stops at its last character.
    ///
    /// Sliced out of the source rather than compared to two numbers, because
    /// the numbers are the thing under test. `HELLO_FOSSIL` puts a blank line
    /// after this binding on purpose: rowan's `text_range()` carries the
    /// trailing trivia, so an untrimmed span ends on line 4 and miette renders
    /// the label as a multi-line block down the gutter, with the caret under
    /// nothing. The native CLI's `broken_field` golden showed exactly that.
    #[test]
    fn a_source_binding_span_is_the_binding_and_no_trivia() {
        let (db, file) = db_with_hello();
        let span = def_map(&db, file)
            .lookup_source_span(&db, "User")
            .expect("`User` is bound");
        let text = &HELLO_FOSSIL[span.start as usize..span.end as usize];
        assert_eq!(text, "User := io.csv(\"examples/users.csv\")");
    }

    /// And a COMMENT is trivia too, which trimming characters cannot see.
    ///
    /// The corpus writes these: `// #region` / `// #endregion` mark the runs
    /// `docs/` transcludes, so the binding a documentation page shows is
    /// exactly the binding with a comment glued to its range — on
    /// `compound-key` with its join condition broken, the report would
    /// underline `// #endregion on` and the blank line after it.
    #[test]
    fn a_trailing_comment_is_trivia_and_stays_out_of_the_span() {
        const COMMENTED: &str = "\
type { Person } := io.shex(\"personas.shex\")

// #region binding
User := io.csv(\"examples/users.csv\")
// #endregion binding

Users : Person from User
    @subject = \"https://example.org/user/{User.id}\"
    name = User.name
";
        let db = new_db();
        let file = fossil_base::SourceFile::new(
            &db,
            COMMENTED.to_string(),
            "examples/commented.fossil".to_string(),
        );
        let span = def_map(&db, file)
            .lookup_source_span(&db, "User")
            .expect("`User` is bound");
        assert_eq!(
            &COMMENTED[span.start as usize..span.end as usize],
            "User := io.csv(\"examples/users.csv\")"
        );
    }

    #[test]
    fn def_map_for_hello_fossil_has_one_type_one_source_one_mapping() {
        let (db, file) = db_with_hello();
        let dm = def_map(&db, file);
        // The TYPE table is the thing a shape name resolves against.
        assert_eq!(dm.types(&db).len(), 1);
        assert_eq!(dm.types(&db)[0].name, "Person");
        assert_eq!(dm.types(&db)[0].document.as_deref(), Some("personas.shex"));
        assert_eq!(dm.sources(&db).len(), 1);
        assert!(dm.lookup_source(&db, "User").is_some());
        assert_eq!(dm.mappings(&db).len(), 1);
    }

    #[test]
    fn def_map_resolves_source_constructor_and_uri() {
        let (db, file) = db_with_hello();
        let dm = def_map(&db, file);
        let call = dm.lookup_source_call(&db, "User").expect("User is bound");
        assert_eq!(call.constructor.as_deref(), Some("io.csv"));
        assert_eq!(call.uri.as_deref(), Some("examples/users.csv"));
        assert_eq!(call.delimiter, None);
    }

    #[test]
    fn def_map_resolves_json_and_parquet_constructors() {
        let src = "\
a := io.json(\"a.json\")
b := io.parquet(\"b.parquet\")
";
        let db = new_db();
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        let dm = def_map(&db, file);
        assert_eq!(
            dm.lookup_source_call(&db, "a"),
            Some(SourceCall {
                constructor: Some("io.json".into()),
                uri: Some("a.json".into()),
                delimiter: None,
            })
        );
        assert_eq!(
            dm.lookup_source_call(&db, "b"),
            Some(SourceCall {
                constructor: Some("io.parquet".into()),
                uri: Some("b.parquet".into()),
                delimiter: None,
            })
        );
    }

    #[test]
    fn def_map_skips_named_arg_string_for_uri() {
        // The POSITIONAL URI is resolved, the `schema = "..."` named-arg string
        // is NOT mistaken for the URI.
        let src = "u := io.csv(\"u.csv\", schema = io.shex(\"u.shex\"))\n";
        let db = new_db();
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        let dm = def_map(&db, file);
        let call = dm.lookup_source_call(&db, "u").unwrap();
        assert_eq!(call.constructor.as_deref(), Some("io.csv"));
        assert_eq!(call.uri.as_deref(), Some("u.csv"));
        assert_eq!(
            dm.lookup_source_schema_binding(&db, "u")
                .map(|(_, d)| d)
                .as_deref(),
            Some("u.shex")
        );
    }

    /// **The delimiter reaches the `DefMap`, and only on the row that declares
    /// one.**
    ///
    /// The second half is the part worth a test: `io.json`'s row has no
    /// optional named position, so `delimiter =` written there is not read. An
    /// argument that is read and then dropped somewhere downstream is the
    /// silent defect this whole thread exists to avoid; an argument that is
    /// never read is one `check_reader_option` can report at the node.
    #[test]
    fn the_delimiter_is_read_for_the_row_that_declares_one() {
        let src = "u := io.csv(\"u.csv\", delimiter = \"|\")\n\
                   j := io.json(\"j.json\", delimiter = \"|\")\n\
                   p := io.csv(\"p.csv\")\n";
        let db = new_db();
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        let dm = def_map(&db, file);

        let u = dm.lookup_source_call(&db, "u").expect("`u` is bound");
        assert_eq!(
            u.uri.as_deref(),
            Some("u.csv"),
            "the URI is still positional"
        );
        assert_eq!(u.delimiter.as_deref(), Some("|"));

        assert_eq!(
            dm.lookup_source_call(&db, "j").unwrap().delimiter,
            None,
            "`io.json`'s catalogue row declares no reader option, so it has no such argument"
        );
        assert_eq!(
            dm.lookup_source_call(&db, "p").unwrap().delimiter,
            None,
            "absent is absent — there is no default in this layer or any other"
        );
    }

    /// The name the header is read for is the name `catalogue.bnf` declares.
    ///
    /// [`reader_option_of`] derives it from the registry, so this cannot fail
    /// by drift — what it holds is that the derivation FINDS something, which a
    /// `find` returning `None` would not. Without it, renaming the parameter in
    /// the `.bnf` would make every `delimiter =` silently unread and every test
    /// above would still have to be edited to notice.
    #[test]
    fn the_option_read_is_the_one_the_catalogue_declares() {
        assert_eq!(
            reader_option_of("io.csv").map(SmolStr::as_str),
            Some("delimiter"),
            "`io.csv`'s row must declare exactly one optional named position"
        );
        assert_eq!(reader_option_of("io.json"), None);
        assert_eq!(reader_option_of("io.parquet"), None);
        assert_eq!(reader_option_of("io.rdf"), None);
    }

    /// **`schema =` names a provider.** The argument carries the row that reads
    /// the document, so no document in the language arrives without one — and
    /// the source's own positional URI is still the source's, not the argument's.
    #[test]
    fn the_schema_argument_carries_its_provider() {
        let src = "{ Zeta } := io.rdf(\"g.ttl\", schema = io.shex(\"two_shapes.shex\"))\n";
        let (db, file) = db_with_document(src, "two_shapes.shex", TWO_SHAPES);
        let dm = def_map(&db, file);
        let entry = &dm.sources(&db)[0];
        assert_eq!(entry.constructor.as_deref(), Some("io.rdf"));
        assert_eq!(entry.uri.as_deref(), Some("g.ttl"));
        assert_eq!(entry.schema_provider.as_deref(), Some("io.shex"));
        assert_eq!(entry.schema_arg.as_deref(), Some("two_shapes.shex"));
        assert!(dm.lookup_source_shape_iri(&db, "Zeta").is_some());
    }

    /// A bare path still PARSES — the parser recovers it rather than dropping
    /// the argument — and comes back with no provider, which is what
    /// `crate::lower::check_schema_arg` reports with the argument's own span.
    /// Reading it as "no argument" would be the silent drop again.
    #[test]
    fn a_bare_schema_path_parses_and_names_no_provider() {
        let src = "{ a } := io.rdf(\"g.ttl\", schema = \"two_shapes.shex\")\n";
        let (db, file) = db_with_document(src, "two_shapes.shex", TWO_SHAPES);
        let dm = def_map(&db, file);
        let entry = &dm.sources(&db)[0];
        assert_eq!(entry.schema_arg.as_deref(), Some("two_shapes.shex"));
        assert_eq!(entry.schema_provider, None);
        // The span is the bare path and nothing else — not the outer call's
        // closing paren, which is the token immediately after it.
        let arg = parse_schema_arg(
            &fossil_syntax::parse(&db, file)
                .root(&db)
                .syntax()
                .children()
                .next()
                .expect("the binding"),
        )
        .expect("a `schema =` argument");
        assert_eq!(
            &src[arg.span.start as usize..arg.span.end as usize],
            "\"two_shapes.shex\""
        );
        assert!(
            matches!(
                dm.lookup_source_shape_error(&db, "a"),
                Some(ShapeBindError::Document(
                    fossil_graph_schema::Problem::BareDocumentPath { .. }
                ))
            ),
            "no provider named ⇒ no row ⇒ no shapes, and it says which"
        );
    }

    /// The document declares `Zeta` then `Alpha`, and the binding names them
    /// the other way round. Under positional binding `Alpha` took `Zeta`'s IRI;
    /// by name each member takes the shape it names, and `as` renames it in the
    /// program without changing which shape it is.
    #[test]
    fn a_destructuring_member_binds_by_local_name_and_as_renames_it() {
        let src = "{ Alpha as primero, Zeta } := io.rdf(\"g.ttl\", schema = \
                   io.shex(\"two_shapes.shex\"))\n";
        let (db, file) = db_with_document(src, "two_shapes.shex", TWO_SHAPES);
        let dm = def_map(&db, file);

        assert_eq!(
            dm.lookup_source_shape_iri(&db, "primero").as_deref(),
            Some("https://example.org/Alpha"),
            "the name before `as` selects the shape; the one after names it"
        );
        assert_eq!(
            dm.lookup_source_shape_iri(&db, "Zeta").as_deref(),
            Some("https://example.org/Zeta")
        );
        assert!(
            dm.lookup_source(&db, "Alpha").is_none(),
            "`as` renames: the shape's own name is not bound as well"
        );
        assert_eq!(dm.lookup_source_shape_error(&db, "primero"), None);
    }

    /// A member naming no shape the document declares carries the ones it
    /// does, in the document's order.
    #[test]
    fn a_member_the_document_does_not_declare_carries_the_ones_it_does() {
        let src = "{ Omega } := io.rdf(\"g.ttl\", schema = io.shex(\"two_shapes.shex\"))\n";
        let (db, file) = db_with_document(src, "two_shapes.shex", TWO_SHAPES);
        let dm = def_map(&db, file);

        assert_eq!(
            dm.lookup_source_shape_error(&db, "Omega"),
            Some(ShapeBindError::NotInDocument {
                shape: "Omega".into(),
                declared: vec!["Zeta".into(), "Alpha".into()],
            })
        );
    }

    /// Two shapes whose local names coincide are an error for the member that
    /// names them, with both IRIs — and nothing for a binding that names
    /// neither.
    #[test]
    fn two_shapes_sharing_a_local_name_refuse_the_import_that_names_it() {
        const TWO_PLACES: &str = "\
shape https://a.example/Place
shape https://b.example/Place
shape https://a.example/Tag
";
        let src = "type { Place, Tag } := io.shex(\"places.shex\")\n";
        let (db, file) = db_with_document(src, "places.shex", TWO_PLACES);
        let dm = def_map(&db, file);

        assert_eq!(
            dm.lookup_type_error(&db, "Place"),
            Some(ShapeBindError::Ambiguous {
                shape: "Place".into(),
                first: "https://a.example/Place".into(),
                second: "https://b.example/Place".into(),
            })
        );
        assert_eq!(
            dm.lookup_type(&db, "Tag").as_deref(),
            Some("https://a.example/Tag"),
            "a member that names an unambiguous shape binds it"
        );
    }

    /// A document that is not there is not a misspelt name: the cause is
    /// reported, not the member's name.
    #[test]
    fn a_document_that_cannot_be_read_says_so_instead_of_blaming_the_name() {
        let src = "{ a } := io.rdf(\"g.ttl\", schema = io.shex(\"does_not_exist.shex\"))\n";
        let (db, file) = db_with_document(src, "two_shapes.shex", TWO_SHAPES);
        let dm = def_map(&db, file);

        assert!(matches!(
            dm.lookup_source_shape_error(&db, "a"),
            Some(ShapeBindError::Document(
                fossil_graph_schema::Problem::NotRegistered { .. }
            ))
        ));
    }

    /// A document that IS there and cannot be read as a shape document is a
    /// different cause from one that is not there at all — the decoder's own
    /// reason travels with it.
    #[test]
    fn a_document_the_decoder_rejected_is_unparseable_and_carries_the_reason() {
        let src = "{ a } := io.rdf(\"g.ttl\", schema = io.shex(\"broken.shex\"))\n";
        let (db, file) = db_with_document(src, "broken.shex", "!malformed no shapes here\n");
        let dm = def_map(&db, file);

        let Some(ShapeBindError::Document(fossil_graph_schema::Problem::Unparseable {
            document,
            reason: Some(reason),
        })) = dm.lookup_source_shape_error(&db, "a")
        else {
            panic!(
                "expected Unparseable, got {:?}",
                dm.lookup_source_shape_error(&db, "a")
            );
        };
        assert_eq!(document, "broken.shex");
        assert!(reason.contains("no shapes here"), "got {reason}");
    }

    /// And a constructor with no `schema =` has no document at all — a third
    /// distinct cause, not the same `None` as the other three.
    #[test]
    fn a_destructuring_source_without_a_schema_argument_says_that() {
        let src = "{ a } := io.rdf(\"g.ttl\")\n";
        let db = new_db();
        let file = fossil_base::SourceFile::new(&db, src.to_string(), "x.fossil".to_string());
        let dm = def_map(&db, file);

        assert_eq!(
            dm.lookup_source_shape_error(&db, "a"),
            Some(ShapeBindError::NoSchema)
        );
    }

    /// `type { … } := io.shex(…)` reaches the `DefMap`, binds by name like
    /// its value-side twin, and carries the document. The document is the datum
    /// that matters: it is where a CSV-sourced program finds its output shape.
    #[test]
    fn a_type_binding_reaches_the_def_map_and_binds_by_name() {
        let src = "type { Alpha as uno, Zeta } := io.shex(\"two_shapes.shex\")\n";
        let (db, file) = db_with_document(src, "two_shapes.shex", TWO_SHAPES);
        let dm = def_map(&db, file);

        assert_eq!(
            dm.lookup_type(&db, "uno").as_deref(),
            Some("https://example.org/Alpha")
        );
        assert_eq!(
            dm.lookup_type(&db, "Zeta").as_deref(),
            Some("https://example.org/Zeta")
        );
        assert_eq!(
            dm.types(&db)[0].document.as_deref(),
            Some("two_shapes.shex")
        );
        assert!(!dm.types(&db)[0].declared);
    }

    /// A declared type is a row of the same table: its name resolves to the IRI
    /// it writes, it names no document, and an import beside it is unaffected.
    #[test]
    fn a_declared_type_is_a_row_of_the_type_table() {
        let src = "@iri(\"https://example.org/City\")\n\
                   type City\n    \
                   @iri(\"https://example.org/name\") name: String\n\
                   type { Zeta } := io.shex(\"two_shapes.shex\")\n";
        let (db, file) = db_with_document(src, "two_shapes.shex", TWO_SHAPES);
        let dm = def_map(&db, file);

        assert_eq!(
            dm.lookup_type(&db, "City").as_deref(),
            Some("https://example.org/City")
        );
        let city = dm
            .type_of_shape(&db, "https://example.org/City")
            .expect("City is a type");
        assert!(city.declared);
        assert_eq!(city.document, None);
        assert_eq!(
            dm.lookup_type(&db, "Zeta").as_deref(),
            Some("https://example.org/Zeta")
        );
    }

    /// A type name is not a source name. Reading one as the other would let a
    /// mapping say `from Person` and get a row out of a shape document, so the
    /// two tables stay disjoint and `source_idx` never advances for a `TYPE_DEF`.
    #[test]
    fn a_type_name_is_not_a_source_and_does_not_take_a_source_slot() {
        let src = "type { Person } := io.shex(\"two_shapes.shex\")\n\
                   users := io.csv(\"u.csv\")\n";
        let (db, file) = db_with_document(src, "two_shapes.shex", TWO_SHAPES);
        let dm = def_map(&db, file);

        assert!(dm.lookup_source(&db, "Person").is_none());
        assert!(dm.lookup_type(&db, "users").is_none());
        assert_eq!(
            dm.lookup_source(&db, "users").map(|l| l.index(&db)),
            Some(0),
            "the type binding must not consume a source index"
        );
    }

    /// The by-name check applies on the type side too, and `type` remains
    /// usable as an ordinary binding name in the same file.
    #[test]
    fn a_type_binding_refuses_an_undeclared_name_and_type_is_still_a_usable_name() {
        let src = "type { Zeta, Omega } := io.shex(\"two_shapes.shex\")\n\
                   type := io.csv(\"type.csv\")\n";
        let (db, file) = db_with_document(src, "two_shapes.shex", TWO_SHAPES);
        let dm = def_map(&db, file);

        assert!(matches!(
            dm.lookup_type_error(&db, "Omega"),
            Some(ShapeBindError::NotInDocument { .. })
        ));
        assert_eq!(
            dm.lookup_source_call(&db, "type"),
            Some(SourceCall {
                constructor: Some("io.csv".into()),
                uri: Some("type.csv".into()),
                delimiter: None,
            }),
            "`type` is contextual, so it is still a binding name"
        );
    }

    #[test]
    fn def_map_source_loc_is_interned_per_file() {
        let (db, file) = db_with_hello();
        let dm = def_map(&db, file);
        let src = dm.lookup_source(&db, "User").unwrap();
        // Per-kind dense indexing. `users` is the only
        // SOURCE_DEF in hello.fossil, so its dense index is 0.
        assert_eq!(src.index(&db), 0);
        assert_eq!(src.file(&db), file);
    }

    #[test]
    fn def_map_mapping_loc_indexes_mapping_position() {
        let (db, file) = db_with_hello();
        let dm = def_map(&db, file);
        let mappings = dm.mappings(&db);
        assert_eq!(mappings.len(), 1);
        // Per-kind dense indexing. `User` is the only MAPPING
        // in hello.fossil, so its dense index is 0 (was 2 under the
        // legacy all-children index space).
        assert_eq!(mappings[0].index(&db), 0);
        assert_eq!(mappings[0].file(&db), file);
    }

    #[test]
    fn def_map_is_memoised_across_invocations() {
        let (db, file) = db_with_hello();
        let a = def_map(&db, file);
        let b = def_map(&db, file);
        // Salsa-tracked structs compare by interned id; identical inputs
        // must yield the same handle (memoisation hit).
        assert_eq!(a, b);
    }
}
