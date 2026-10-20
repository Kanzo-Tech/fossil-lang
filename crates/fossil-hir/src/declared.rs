//! Types declared in the program — `@iri("…") type Person` and its fields —
//! lowered to the shape vocabulary a `ShEx` or SHACL decoder produces.
//!
//! A declaration is a third decoder whose input is the program
//! (`/docs/design/declared`). What it lowers to is
//! [`fossil_graph_schema::OutputShapes`], and nothing below this module learns
//! whether a shape came from here or from a document: the checker resolves a
//! mapping's target through [`crate::shapes::resolve_target_shape`], the run
//! classifies edges through [`crate::shapes::program_shapes`], and both read
//! one table. The lowering runs inside [`crate::lower::lower_to_hir`] and lands
//! on [`crate::lower::HirFile::declared`].
//!
//! # A field's type
//!
//! A primitive of the lattice, written as the checker prints it
//! ([`fossil_graph_schema::Primitive::from_name`]), is a literal of that datatype, exactly one.
//! `Ref<T>` is an edge to the type `T` names — declared or imported, through
//! [`crate::def_map::DefMap::lookup_type`] like every other use of a type name.
//! `Option<…>` is zero or one and `Seq<…>` zero or more, and either holds a
//! primitive or a `Ref<…>`. `Seq<T, n>` is at least `n` and `Seq<T, n, m>`
//! between `n` and `m`: SHACL's `sh:minCount`/`sh:maxCount` and `ShEx`'s
//! `{n,m}`, each with one spelling.
//!
//! That is the whole grammar, and what it leaves out is decided rather than
//! missing. The vocabulary carries ONE `(min, max)` per property, so a
//! cardinality is said once, outermost: `Option<Seq<T>>` would be a second
//! spelling of `Seq<T>`, `Seq<Option<T>>` a list with holes the graph has no
//! way to write, and `Option<Option<T>>` nothing at all. Bounds that another
//! spelling already says are the same second spelling — `Seq<T, 0, 1>` is
//! `Option<T>`, `Seq<T, 1, 1>` is `T`, `Seq<T, 0>` is `Seq<T>`. `Ref<String>`
//! refers to a value, which is not a node. Each is refused as
//! `type/invalid-field-type`, naming the rule.
//!
//! # What a type says beyond its fields
//!
//! A field's `@severity(…)` and every `@assert(…)` lower beside the shape into
//! [`crate::asserts::DeclaredRules`], keyed by the type's IRI: what they mean is
//! in [`crate::asserts`].
//!
//! # A refused field is still a field
//!
//! A field whose type is refused lowers anyway, accepting any value of any
//! count. Dropping it would add `name/unknown-property` at every mapping that
//! writes it — a second diagnostic about the same line, pointing somewhere
//! else. Its own error is already reported, and the program does not run.

use fossil_base::{Diagnostic, Diagnostics};
use fossil_graph_schema::{
    Annotation, ConstraintSeverity, Occurs, OutputShapes, Primitive, Problem, PropertyConstraint,
    Severity, Shape, Span, Term,
};
use fossil_syntax::ast::{
    AnnotationAttr, AssertAttr, Field, ImportMember, IriAttr, TypeDecl, TypeExpr,
};
use fossil_syntax::{SyntaxKind, SyntaxNode, SyntaxToken};
use smol_str::SmolStr;

use crate::asserts::{CountSeverity, DeclaredAssert, DeclaredRules};
use crate::def_map::DefMap;

/// The constructors a field's type is written with, beside the primitives.
const OPTION: &str = "Option";
const SEQ: &str = "Seq";
const REF: &str = "Ref";

/// Lower every `TYPE_DECL` under `root` — one shape per declared type with an
/// IRI, in written order, and what each constrains beyond its fields' types —
/// and refuse, into `sink`, what a declaration or a type name introduced twice,
/// declared or imported, gets wrong. Every diagnostic is file-absolute.
///
/// `types` is every type name the program binds, which an assertion's
/// condition is lowered against as any expression is.
pub(crate) fn lower_declarations(
    db: &dyn fossil_base::Db,
    dm: DefMap<'_>,
    root: &SyntaxNode,
    types: &[SmolStr],
    sink: &mut Diagnostics,
) -> (OutputShapes, DeclaredRules) {
    refuse_names_introduced_twice(root, sink);
    let mut rules = DeclaredRules::default();
    let shapes = root
        .children()
        .filter_map(TypeDecl::cast)
        .filter_map(|decl| lower_decl(db, dm, types, sink, &mut rules, &decl))
        .collect();
    (OutputShapes::new(shapes, Vec::new()), rules)
}

/// One declaration's shape. `None` without an IRI or a name, which the parser
/// has already refused: there is no shape to key.
fn lower_decl(
    db: &dyn fossil_base::Db,
    dm: DefMap<'_>,
    types: &[SmolStr],
    sink: &mut Diagnostics,
    rules: &mut DeclaredRules,
    decl: &TypeDecl,
) -> Option<Shape> {
    let type_name = decl.name()?;
    let iri_attr = decl.iri_attr()?;
    refuse_prefixed_iri(&iri_attr, sink);
    let iri = iri_attr.iri()?;
    let on = |field: Option<SmolStr>| Owner {
        shape: iri.clone(),
        type_name: type_name.clone(),
        field,
    };
    for attr in decl.asserts() {
        rules
            .asserts
            .extend(lower_assert(db, types, sink, &attr, on(None)));
    }
    let mut properties: Vec<PropertyConstraint> = Vec::new();
    for field in decl.fields() {
        if let Some(attr) = field.iri_attr() {
            refuse_prefixed_iri(&attr, sink);
        }
        let annotations = lower_annotations(field.annotations(), sink);
        let (Some(name), Some(predicate)) = (field.name(), field.iri_attr().and_then(|a| a.iri()))
        else {
            continue;
        };
        if properties.iter().any(|p| p.name == name.as_str()) {
            emit(
                sink,
                field_name_span(&field),
                Problem::DuplicateField {
                    type_name: type_name.to_string(),
                    field: name.to_string(),
                },
                Some("a field is the one place its name is given a predicate; keep one".into()),
            );
            continue;
        }
        // A field with no type at all is the parser's to refuse.
        let (occurs, term, targets) =
            field
                .type_expr()
                .map_or((UNCHECKED, None, Vec::new()), |te| {
                    field_type(db, dm, &te).unwrap_or_else(|refusal| {
                        sink.push(*refusal);
                        (UNCHECKED, None, Vec::new())
                    })
                });
        for attr in field.asserts() {
            rules
                .asserts
                .extend(lower_assert(db, types, sink, &attr, on(Some(name.clone()))));
        }
        if let Some(severity) = field
            .severity()
            .and_then(|s| s.level())
            .and_then(|level| severity(&level, sink))
        {
            rules.counts.push(CountSeverity {
                shape: iri.clone(),
                field: name.clone(),
                severity,
            });
        }
        properties.push(PropertyConstraint {
            name: name.to_string(),
            predicate: predicate.to_string(),
            term,
            targets,
            occurs,
            span: Some(field_name_span(&field)),
            annotations,
            languages: Vec::new(),
        });
    }
    Some(Shape {
        iri: iri.to_string(),
        properties,
        span: decl_name_span(decl),
        annotations: lower_annotations(decl.annotations(), sink),
    })
}

/// Whose an assertion is: the type, and the field when it is on one.
struct Owner {
    shape: SmolStr,
    type_name: SmolStr,
    field: Option<SmolStr>,
}

/// An `@assert(…)`, its condition lowered as any expression is. `None` when
/// there is no condition, which the parser has refused, or when the condition
/// did not lower, which lowering has reported.
fn lower_assert(
    db: &dyn fossil_base::Db,
    types: &[SmolStr],
    sink: &mut Diagnostics,
    attr: &AssertAttr,
    owner: Owner,
) -> Option<DeclaredAssert> {
    let node = attr.condition()?;
    // Lowering reports in the frame of whatever it was handed; this node is
    // the whole file's, so what it says is file-absolute.
    let mut lowering = Diagnostics::new();
    let condition = crate::lower::lower_expr(db, &mut lowering, &node, types);
    for d in lowering.into_vec() {
        sink.push(d.file_absolute());
    }
    let severity = match attr.severity() {
        Some(level) => severity(&level, sink)?,
        None => ConstraintSeverity::Violation,
    };
    Some(DeclaredAssert {
        shape: owner.shape,
        type_name: owner.type_name,
        field: owner.field,
        condition: condition?,
        text: SmolStr::from(node.text().to_string().trim()),
        severity,
        span: node_span(&node),
    })
}

/// The severity a program wrote, or `None` and a refusal when it is not one.
fn severity(level: &SyntaxToken, sink: &mut Diagnostics) -> Option<ConstraintSeverity> {
    let written = level.text();
    ConstraintSeverity::from_name(written).or_else(|| {
        let names = ConstraintSeverity::ALL.map(ConstraintSeverity::name);
        let help = crate::didyoumean::did_you_mean(written, names.iter().copied())
            .map(|s| format!("did you mean `{s}`?"));
        emit(
            sink,
            token_span(level),
            Problem::UnknownSeverity {
                written: written.to_string(),
                severities: names.map(String::from).to_vec(),
            },
            help,
        );
        None
    })
}

/// A declaration's `@annotation`s as the vocabulary carries them — `ShEx`'s
/// Annotation, the object a plain or a language-tagged string. A predicate
/// written as a prefixed name and a tag that is not BCP 47 are refused, and
/// the annotation is left out: it would carry an IRI nobody meant, or a
/// literal RDF forbids.
fn lower_annotations(
    attrs: impl Iterator<Item = AnnotationAttr>,
    sink: &mut Diagnostics,
) -> Vec<Annotation> {
    let mut out = Vec::new();
    for attr in attrs {
        let (Some(predicate), Some((value, tag))) = (attr.predicate_text(), attr.object_value())
        else {
            // A missing argument is the parser's to refuse.
            continue;
        };
        if let Some(expands_to) = crate::terms::prefixed_name(&predicate) {
            let at = attr
                .predicate()
                .map_or_else(|| node_span(attr.syntax()), |t| token_span(&t));
            emit(
                sink,
                at,
                Problem::PrefixedIri {
                    written: predicate.to_string(),
                    expands_to: expands_to.clone(),
                },
                Some(prefixed_help(&expands_to)),
            );
            continue;
        }
        if let Some(tag) = tag.as_deref()
            && !crate::terms::well_formed_language_tag(tag)
        {
            let at = attr
                .object()
                .map_or_else(|| node_span(attr.syntax()), |t| token_span(&t));
            emit(
                sink,
                at,
                Problem::InvalidLanguageTag {
                    tag: tag.to_string(),
                },
                Some(LANGUAGE_TAG_HELP.to_string()),
            );
            continue;
        }
        out.push(Annotation::literal(&predicate, &value, tag.as_deref()));
    }
    out
}

/// What a refused language tag's help says.
pub(crate) const LANGUAGE_TAG_HELP: &str = "a language tag is BCP 47: a language, then optional \
     script, region and variants joined by `-` — `en`, `en-US`, `zh-Hant`, `de-CH-1996`";

/// What a refused prefixed name's help says.
fn prefixed_help(expands_to: &str) -> String {
    format!(
        "there are no prefixes, so a name nobody wrote never becomes an IRI: write it whole, \
         `\"{expands_to}\"`"
    )
}

/// `@iri("rdfs:Class")` is the same mistake as an annotation's predicate.
fn refuse_prefixed_iri(attr: &IriAttr, sink: &mut Diagnostics) {
    let Some(iri) = attr.iri() else { return };
    if let Some(expands_to) = crate::terms::prefixed_name(&iri) {
        emit(
            sink,
            node_span(attr.syntax()),
            Problem::PrefixedIri {
                written: iri.to_string(),
                expands_to: expands_to.clone(),
            },
            Some(prefixed_help(&expands_to)),
        );
    }
}

/// The occurrence a refused field lowers with: any count, so nothing else is
/// reported about it.
const UNCHECKED: Occurs = Occurs { min: 0, max: None };

/// A refusal, as the diagnostic it is reported as.
type Refusal = Box<Diagnostic>;

fn refusal(at: Span, problem: Problem, help: Option<String>) -> Refusal {
    let d = Diagnostic::new(Severity::Error, problem, at).file_absolute();
    Box::new(match help {
        Some(help) => d.with_help(help),
        None => d,
    })
}

/// A field's whole type — the cardinality, outermost, then the value.
fn field_type(
    db: &dyn fossil_base::Db,
    dm: DefMap<'_>,
    te: &TypeExpr,
) -> Result<(Occurs, Option<Term>, Vec<String>), Refusal> {
    let occurs = match te.name().as_deref() {
        Some(OPTION) => {
            no_bounds(te)?;
            Occurs {
                min: 0,
                max: Some(1),
            }
        }
        Some(SEQ) => seq_bounds(te)?,
        _ => {
            no_bounds(te)?;
            let (term, targets) = value_type(db, dm, te)?;
            return Ok((Occurs::ONE, term, targets));
        }
    };
    let held = te.argument().ok_or_else(|| {
        invalid(
            te,
            format!(
                "`{}` says how many values a field holds, and needs the type of one: `{}<String>`",
                te.name().unwrap_or_default(),
                te.name().unwrap_or_default()
            ),
        )
    })?;
    if matches!(held.name().as_deref(), Some(OPTION | SEQ)) {
        return Err(invalid(
            te,
            "a field says how many values it holds once, outermost — `Option<T>` or `Seq<T>` \
             of a primitive or a `Ref<T>` — and the vocabulary has one count per property"
                .into(),
        ));
    }
    let (term, targets) = value_type(db, dm, &held)?;
    Ok((occurs, term, targets))
}

/// `Seq<T>`'s count, from the bounds after `T`: at least the first, at most
/// the second. A pair another spelling says is refused for that spelling.
fn seq_bounds(te: &TypeExpr) -> Result<Occurs, Refusal> {
    let bounds = te
        .bounds()
        .map(|t| {
            t.text().replace('_', "").parse::<u32>().map_err(|_| {
                invalid(
                    te,
                    format!("`{}` is not a count of values a field can hold", t.text()),
                )
            })
        })
        .collect::<Result<Vec<u32>, _>>()?;
    let occurs = match bounds.as_slice() {
        [] => return Ok(Occurs { min: 0, max: None }),
        [min] => Occurs {
            min: *min,
            max: None,
        },
        [min, max, ..] => Occurs {
            min: *min,
            max: Some(*max),
        },
    };
    let said_otherwise = match (occurs.min, occurs.max) {
        (0, None) => Some("`Seq<T>`"),
        (0, Some(1)) => Some("`Option<T>`"),
        (1, Some(1)) => Some("`T`"),
        _ => None,
    };
    if let Some(spelling) = said_otherwise {
        return Err(invalid(
            te,
            format!("that count has one spelling, and it is {spelling}"),
        ));
    }
    if let Some(max) = occurs.max
        && (max == 0 || max < occurs.min)
    {
        return Err(invalid(
            te,
            format!(
                "a field holds at least {} and at most {max} values, which no field can",
                occurs.min
            ),
        ));
    }
    Ok(occurs)
}

/// Bounds belong to `Seq` alone: one value or none is already a count.
fn no_bounds(te: &TypeExpr) -> Result<(), Refusal> {
    if te.bounds().next().is_some() {
        return Err(invalid(
            te,
            format!(
                "only `Seq` takes bounds — `Seq<T, 1, 3>`; `{}` already says how many",
                te.name().unwrap_or_default()
            ),
        ));
    }
    Ok(())
}

/// What ONE value of a field is: a primitive, or a reference to a type.
fn value_type(
    db: &dyn fossil_base::Db,
    dm: DefMap<'_>,
    te: &TypeExpr,
) -> Result<(Option<Term>, Vec<String>), Refusal> {
    let name = te.name().unwrap_or_default();
    if name == REF {
        let target = te
            .argument()
            .ok_or_else(|| invalid(te, "`Ref` needs the type it refers to: `Ref<City>`".into()))?;
        let target_name = target.name().unwrap_or_default();
        if target.argument().is_some()
            || Primitive::from_name(&target_name).is_some()
            || [OPTION, SEQ, REF].contains(&target_name.as_str())
        {
            return Err(invalid(
                te,
                "a reference is to a node of a declared or imported type, written by its name: \
                 `Ref<City>`"
                    .into(),
            ));
        }
        return dm.lookup_type(db, &target_name).map_or_else(
            || {
                let (problem, help) = crate::lower::unbound_shape_problem(db, dm, &target_name);
                Err(refusal(node_span(target.syntax()), problem, help))
            },
            |iri| Ok((None, vec![iri.to_string()])),
        );
    }
    if let Some(p) = Primitive::from_name(&name) {
        if te.argument().is_some() {
            return Err(invalid(
                te,
                format!("`{p}` is a primitive and takes no argument"),
            ));
        }
        return Ok((
            Some(Term::Literal(Some(p.to_xsd_iri().to_string()))),
            Vec::new(),
        ));
    }
    if dm.lookup_type(db, &name).is_some() || dm.lookup_type_error(db, &name).is_some() {
        return Err(invalid(
            te,
            format!(
                "`{name}` is a type of this program, and a field refers to it as `Ref<{name}>`"
            ),
        ));
    }
    let types = field_type_names();
    let help = crate::didyoumean::did_you_mean(&name, types.iter().map(String::as_str))
        .map(|s| format!("did you mean `{s}`?"));
    Err(refusal(
        node_span(te.syntax()),
        Problem::UnknownType {
            name: name.to_string(),
            types,
        },
        help,
    ))
}

/// Every name a field's type may start with: the lattice, then the three
/// constructors.
fn field_type_names() -> Vec<String> {
    Primitive::ALL
        .iter()
        .map(ToString::to_string)
        .chain([OPTION, SEQ, REF].map(String::from))
        .collect()
}

fn invalid(te: &TypeExpr, reason: String) -> Refusal {
    refusal(
        node_span(te.syntax()),
        Problem::InvalidFieldType {
            written: te.syntax().text().to_string().trim().to_string(),
            reason,
        },
        None,
    )
}

/// A name introduced twice — declared twice, imported twice, or one of each —
/// refused at the second. A type name names one type; the lookup would take
/// the first and the second would vanish without a word.
fn refuse_names_introduced_twice(root: &SyntaxNode, sink: &mut Diagnostics) {
    let mut seen: Vec<SmolStr> = Vec::new();
    for item in root.children() {
        let introduced: Vec<(SmolStr, Span)> = match item.kind() {
            SyntaxKind::TYPE_DECL => TypeDecl::cast(item.clone())
                .and_then(|d| Some((d.name()?, decl_name_span(&d)?)))
                .into_iter()
                .collect(),
            SyntaxKind::TYPE_DEF => item
                .children()
                .filter_map(ImportMember::cast)
                .filter_map(|m| {
                    let token = m.name_token()?;
                    Some((SmolStr::from(token.text()), token_span(&token)))
                })
                .collect(),
            _ => continue,
        };
        for (name, span) in introduced {
            if seen.contains(&name) {
                emit(
                    sink,
                    span,
                    Problem::DuplicateType {
                        name: name.to_string(),
                    },
                    Some("rename one: a declaration by its own name, an import with `as`".into()),
                );
            } else {
                seen.push(name);
            }
        }
    }
}

fn emit(sink: &mut Diagnostics, span: Span, problem: Problem, help: Option<String>) {
    sink.push(*refusal(span, problem, help));
}

/// The `IDENT` after `type` in a declaration.
fn decl_name_span(decl: &TypeDecl) -> Option<Span> {
    let mut idents = decl
        .syntax()
        .children_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .filter(|t| t.kind() == SyntaxKind::IDENT)
        .skip_while(|t| t.text() != "type");
    idents.nth(1).map(|t| token_span(&t))
}

/// A field's name, or the whole field when it has none.
fn field_name_span(field: &Field) -> Span {
    field
        .syntax()
        .children_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .find(|t| t.kind() == SyntaxKind::IDENT)
        .map_or_else(|| node_span(field.syntax()), |t| token_span(&t))
}

fn token_span(token: &fossil_syntax::SyntaxToken) -> Span {
    let range = token.text_range();
    Span::new(range.start().into(), range.end().into())
}

/// The node's extent, trivia trimmed — [`crate::def_map::item_span`].
fn node_span(node: &SyntaxNode) -> Span {
    crate::def_map::item_span(node)
}
