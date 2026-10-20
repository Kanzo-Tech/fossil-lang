//! SHACL shapes graph → [`OutputShapes`] — the SHACL row's decode.
//!
//! SHACL **is** RDF: a shapes graph is a set of triples. It is read in two
//! steps, both rudof's: `rudof_rdf`'s in-memory graph parses the Turtle (the
//! reader `io.rdf` reads with), and `shacl`'s `ShaclParser` turns the graph into
//! its AST. This module projects that AST onto the neutral vocabulary:
//!
//! - `sh:NodeShape` + `sh:targetClass T`   → a [`Shape`] whose `iri` is `T`.
//! - `sh:property [ sh:path p ; sh:datatype D ]`         → a literal constraint.
//! - `sh:property [ sh:path p ; sh:nodeKind sh:IRI ]`    → an `Uri` constraint.
//! - `sh:property [ sh:path p ; sh:class C | sh:node S ]`→ a constraint with a target.
//! - `sh:property [ sh:path p ; sh:or ( [sh:class A] [sh:class B] ) ]`
//!   → one constraint with TWO targets (the reference RDF→property-graph union
//!   model, identical to `ShEx`'s `@<A> OR @<B>`).
//! - `sh:minCount` / `sh:maxCount` → [`Occurs`]; absent means SHACL's own
//!   defaults, `0` and unbounded.
//! - `sh:name` / `sh:description` → `rdfs:label` / `rdfs:comment`, and every
//!   triple of a shape outside SHACL's namespace (the AST's `extra`) → itself.
//!
//! Constraint features outside the graph's *shape* (`sh:pattern`, `sh:sparql`,
//! severities, …) are validation concerns and intentionally ignored here — this
//! lowering only needs the node/edge/property structure. **What a language
//! expresses and the neutral vocabulary does not carry is the ROW's to
//! diagnose**, and today this row carries no [`Rejection`] but the parse
//! failure and the order's tie ([`Rejection::AmbiguousOrder`]): everything above
//! is either lowered or is a validation constraint the output model has no place
//! for.
//!
//! # Where `ShaclParser` is stricter, or reads more, than the walk it replaced
//!
//! - **A node shape is what fossil says it is, and the AST says more.** The AST
//!   has a node shape for every IRI or blank node that is a shape by SHACL's
//!   syntax rules (Appendix A), among them every member of an `sh:or` list. fossil
//!   types a document by the ones that declare something: a subject typed
//!   `sh:NodeShape`, or with an `sh:targetClass`, or with an `sh:property`. The
//!   typing is read from the graph, because the AST does not keep `rdf:type`;
//!   a typed shape that is a blank node is not read, which only matters if it
//!   declares nothing else.
//! - **A subject that is only the subject of `sh:property` is not a shape.** With
//!   no type and no target, SHACL's syntax rules do not make it one and the
//!   parser does not return it; the walk it replaced read it as a node shape. A
//!   shape reached by `sh:node` is a shape whatever it declares, and is typed
//!   as before.
//! - **A property shape whose path is not a single predicate is skipped**, as a
//!   `sh:property` with no `sh:path` is. The walk it replaced read a path
//!   expression's blank node as if it were a predicate.
//! - **A malformed constraint is the parser's error**, a [`Rejection::Malformed`]
//!   for the whole document, where the walk ignored a count it could not read.
//! - **`sh:name` and `sh:description` are one value per language**, as the AST
//!   holds them.
//!
//! # Why it produces `OutputShapes` and not a `GraphSchema`
//!
//! A `GraphSchema` is the OUTPUT model, one step past the vocabulary the
//! checker reads: produced directly, SHACL would reach the executor and never
//! the checker, and a program like `catalogue.fossil` would have no output
//! contract and could not write a property. Producing [`OutputShapes`] puts
//! SHACL on the same seam as `ShEx`, and `OutputShapes::to_graph_schema` gives
//! the executor its `GraphSchema`.
//!
//! # Positions: a `.shex` report cites the document and a `.ttl` one does not
//!
//! `fossil_graph_schema::PropertyConstraint::span` is what lets a type error
//! underline the line that declares the constraint it violated, in the shape
//! document itself. `crate::shex::spans` supplies it for `ShExC` and this
//! decoder supplies `None`, so a program checked against a `.ttl` gets a
//! report with the program's span alone. **That is deliberate, and here is the
//! measurement.**
//!
//! `oxttl` carries no position on a parsed triple. Its `TextPosition` exists
//! only inside `TurtleSyntaxError`, and the low-level parser
//! (`LowLevelTurtleParser::parse_next`) yields `Result<Triple, …>` with the
//! same shape — checked against `oxttl` 0.2.3, both the reader and the
//! low-level API. `shex_ast` is in exactly the same state, which is why the
//! `ShEx` side reads the document text.
//!
//! **What does not transfer is that reading the text is SAFE there.**
//! `crate::shex::spans` is a lookup and never a parser: a predicate sits inside
//! `label { … }`, textually delimited, and the shape's identity IS that label,
//! so scoping a search to one shape is matching a brace. Turtle gives none of
//! that. A shape is identified by its `sh:targetClass` and not by the subject
//! it is written under; a property shape is usually a blank node `[ … ]` that
//! may be nested or named or referenced from elsewhere; `;` and `.` terminate
//! at arbitrary depth; and one `sh:path shop:label` may appear in any number of
//! property shapes across the file. Deciding which node shape a given `sh:path`
//! belongs to means tracking nesting and statement termination — that is
//! PARSING Turtle, and a second parser that can disagree with the first is
//! precisely what the `ShEx` module argues against. There the failure mode of a
//! disagreement is a missing label; here it would be a caret on another shape's
//! constraint, in the right file, silently.
//!
//! So the cost is not the work, it is that the cheap version would be wrong.
//!
//! **What reverses this:** `oxttl` exposing the token range of a parsed triple
//! — the lexer already computes one (`toolkit::lexer::last_token_location`), it
//! is simply not on the public iterator. With that, this is a field on the
//! walk's row rather than a reader of anything, and it is strictly better than
//! the `ShEx` side because the position would come from the real parser. It is
//! an upstream ask, not a local one.
//!
//! The corpus has one SHACL document (`docs/programs/catalogue`) and that
//! program is clean, so nothing today would render such a label even if it
//! existed. That is why this is written down rather than built.
//!
//! # Order
//!
//! A shapes graph is a set, so nothing here reads the order a document wrote
//! its triples in. The shapes' order is [`OutputShapes::new`]'s. What this
//! decoder orders is what the AST holds as a list or leaves to the graph:
//!
//! - a shape's properties, by `sh:order` ascending (a property shape with none
//!   after every one with), then the predicate's IRI, then the rest of the
//!   constraint;
//! - the targets of a property, its languages: sorted, without duplicates, since
//!   the order of an `sh:or` or `sh:languageIn` list does not change what the
//!   shape validates;
//! - of several shapes with one `sh:targetClass`, the first is the one with an
//!   IRI, the smallest, and two that are blank nodes are a
//!   [`Rejection::AmbiguousOrder`];
//! - of several `sh:targetClass` values of one shape, the smallest names the type.

use std::collections::BTreeSet;
use std::collections::HashMap;

use fossil_graph_schema::{
    Annotation, AnnotationObject, ObjectLiteral, Occurs, OutputShapes, PropertyConstraint,
    RDFS_COMMENT, RDFS_LABEL, Rejection, Shape, Xsd,
};
use oxrdf::NamedOrBlankNode;
use oxrdf::vocab::{rdf, xsd};
use rudof_rdf::RDFFormat;
use rudof_rdf::SHACLPath;
use rudof_rdf::backend::{OxigraphInMemory, ReaderMode};
use rudof_rdf::term::Object;
use rudof_rdf::term::literal::NumericLiteral;
use shacl::ast::{ASTComponent, ASTNodeShape, ASTPropertyShape, ASTSchema, ASTShape};
use shacl::rdf::ShaclParser;
use shacl::types::{MessageMap, NodeKind, Target};

const SH_NODE_SHAPE: &str = "http://www.w3.org/ns/shacl#NodeShape";

/// Decode a SHACL shapes graph — the SHACL row's `decode`.
///
/// The `uri` is unread: SHACL's relative-reference base would change which IRIs
/// a document resolves to, and choosing one is a decision rather than a
/// tidy-up — the same call `fossil_hir::providers::decode_shex` makes and for the same reason.
///
/// # Errors
///
/// [`Rejection::Malformed`] when the Turtle does not parse, or `ShaclParser`
/// refuses the graph. A document that parses and declares no node shape is
/// **not** an error: it decodes to zero shapes, and the binding that named it
/// reports its own arity failure.
pub fn decode_shacl(_uri: &str, turtle: &str) -> Result<OutputShapes, Rejection> {
    let graph = OxigraphInMemory::from_str(turtle, &RDFFormat::Turtle, None, &ReaderMode::Strict)
        .map_err(|e| Rejection::Malformed(format!("parse SHACL turtle: {e}")))?;
    let typed = typed_node_shapes(&graph);
    let schema = ShaclParser::new(graph)
        .parse()
        .map_err(|e| Rejection::Malformed(format!("read SHACL shapes: {e}")))?;

    let mut shapes: Vec<Shape> = Vec::new();
    let mut rejections: Vec<Rejection> = Vec::new();
    // The own label of the blank-node shape that took each type IRI, for a tie.
    let mut blank_owner: HashMap<String, String> = HashMap::new();
    // The schema iterates IRIs before blank nodes, each by its string: a
    // function of the graph, so the first of several shapes of one type is too.
    for (id, shape) in schema.iter() {
        let ASTShape::NodeShape(node) = shape else {
            continue;
        };
        if !declares_a_type(node, id, &typed) {
            continue;
        }
        // The shape's type is its `sh:targetClass`; absent that, the shape's own
        // IRI is the type (a common self-typed convention). That choice is what
        // makes `shop:ProductShape` with `sh:targetClass shop:Product` produce a
        // `Product` node type and not a `ProductShape` one.
        let iri = node_type(node);
        if let Some(first) = blank_owner.get(&iri) {
            if is_blank(id) {
                rejections.push(Rejection::AmbiguousOrder {
                    first: first.clone(),
                    second: label_of(id),
                });
            }
            continue;
        }
        if is_blank(id) {
            blank_owner.insert(iri.clone(), label_of(id));
        }
        shapes.push(Shape {
            iri,
            properties: properties_of(&schema, node),
            // **SHACL declares no position, and this is a decision** — see the
            // module's «Positions» section for the measurement and for the one
            // upstream change that reverses it. Every consumer already handles
            // `None`, because `ShExJ` gives it too.
            span: None,
            annotations: extra_annotations(node.extra()),
        });
    }
    Ok(OutputShapes::new(shapes, rejections))
}

/// The IRIs the graph types `sh:NodeShape`: the one thing about a shape the AST
/// does not keep.
fn typed_node_shapes(graph: &OxigraphInMemory) -> BTreeSet<String> {
    graph
        .quads()
        .filter(|q| {
            q.predicate.as_ref() == rdf::TYPE
                && matches!(&q.object, oxrdf::Term::NamedNode(n) if n.as_str() == SH_NODE_SHAPE)
        })
        .filter_map(|q| match q.subject {
            NamedOrBlankNode::NamedNode(n) => Some(n.into_string()),
            NamedOrBlankNode::BlankNode(_) => None,
        })
        .collect()
}

/// Does this node shape declare something fossil types a document by: it is
/// typed `sh:NodeShape`, or has an `sh:targetClass`, or an `sh:property`.
fn declares_a_type(node: &ASTNodeShape, id: &Object, typed: &BTreeSet<String>) -> bool {
    matches!(id, Object::Iri(iri) if typed.contains(iri.as_str()))
        || !target_classes(node).is_empty()
        || !node.property_shapes().is_empty()
}

/// The IRIs of a shape's `sh:targetClass` values, smallest first.
fn target_classes(node: &ASTNodeShape) -> Vec<&str> {
    let mut classes: Vec<&str> = node
        .targets()
        .iter()
        .filter_map(|t| match t {
            Target::Class(Object::Iri(iri)) => Some(iri.as_str()),
            _ => None,
        })
        .collect();
    classes.sort_unstable();
    classes.dedup();
    classes
}

fn node_type(node: &ASTNodeShape) -> String {
    target_classes(node)
        .first()
        .map_or_else(|| label_of(node.id()), |c| (*c).to_string())
}

/// A shape's own IRI, or a blank node's `_:label`.
fn label_of(id: &Object) -> String {
    match id {
        Object::Iri(iri) => iri.as_str().to_string(),
        Object::BlankNode(label) => format!("_:{label}"),
        other => other.to_string(),
    }
}

const fn is_blank(id: &Object) -> bool {
    matches!(id, Object::BlankNode(_))
}

/// A node shape's property shapes as constraints, in the decoder's order — see
/// the module's «Order» section.
fn properties_of(schema: &ASTSchema, node: &ASTNodeShape) -> Vec<PropertyConstraint> {
    let mut properties = Vec::new();
    for reference in node.property_shapes() {
        let Some(ASTShape::PropertyShape(property)) = schema.get_shape(reference) else {
            continue;
        };
        let SHACLPath::Predicate { pred } = property.path() else {
            continue;
        };
        let order = property.order().and_then(NumericLiteral::to_decimal);
        let constraint = property_constraint(schema, property, pred.as_str());
        properties.push((order, constraint));
    }
    properties.sort_by(|(oa, a), (ob, b)| {
        oa.is_none()
            .cmp(&ob.is_none())
            .then_with(|| oa.cmp(ob))
            .then_with(|| a.predicate.cmp(&b.predicate))
            .then_with(|| a.targets.cmp(&b.targets))
            .then_with(|| format!("{:?}", a.term).cmp(&format!("{:?}", b.term)))
            .then_with(|| (a.occurs.min, a.occurs.max).cmp(&(b.occurs.min, b.occurs.max)))
    });
    properties.into_iter().map(|(_, c)| c).collect()
}

fn property_constraint(
    schema: &ASTSchema,
    property: &ASTPropertyShape,
    predicate: &str,
) -> PropertyConstraint {
    let components = property.components();
    let mut targets = Vec::new();
    let mut languages: Vec<String> = Vec::new();
    let mut occurs = Occurs { min: 0, max: None };
    for component in components {
        match component {
            ASTComponent::MinCount(n) => occurs.min = u32::try_from(*n).unwrap_or(0),
            ASTComponent::MaxCount(n) => occurs.max = u32::try_from(*n).ok(),
            ASTComponent::LanguageIn(tags) => {
                languages.extend(tags.iter().map(ToString::to_string));
            }
            other => targets_of(schema, other, &mut targets),
        }
    }
    targets.sort();
    targets.dedup();
    languages.sort();
    languages.dedup();
    PropertyConstraint {
        name: fossil_graph_schema::local_name(predicate).to_string(),
        predicate: predicate.to_string(),
        term: term_of(components),
        targets,
        occurs,
        span: None,
        annotations: property_annotations(property),
        languages,
    }
}

/// The destination type IRIs one component points at: `sh:class`, the type of an
/// `sh:node` shape, and every alternative inside an `sh:or` list.
fn targets_of(schema: &ASTSchema, component: &ASTComponent, out: &mut Vec<String>) {
    match component {
        ASTComponent::Class(classes) => {
            out.extend(
                classes
                    .iter()
                    .filter_map(|c| c.get_iri().ok())
                    .map(|i| i.as_str().to_string()),
            );
        }
        ASTComponent::Node(shape) => out.push(node_target(schema, shape)),
        ASTComponent::Or(members) => {
            for member in members {
                if let Some(shape) = schema.get_shape(member) {
                    for inner in shape.components() {
                        if matches!(inner, ASTComponent::Class(_) | ASTComponent::Node(_)) {
                            targets_of(schema, inner, out);
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// A `sh:node`-referenced shape's destination type: its `sh:targetClass`, or the
/// referenced shape's own IRI when it declares none.
fn node_target(schema: &ASTSchema, node_shape: &Object) -> String {
    match schema.get_shape(node_shape) {
        Some(ASTShape::NodeShape(node)) => node_type(node),
        _ => label_of(node_shape),
    }
}

/// The RDF term a property shape declares, verbatim —
/// [`PropertyConstraint::term`]'s contract: `sh:datatype D` is a literal of `D`
/// as written, `sh:nodeKind sh:Literal` a literal with none, and
/// `sh:nodeKind sh:IRI` an IRI — with no `sh:class`, an **opaque IRI column**,
/// not an edge.
fn term_of(components: &[ASTComponent]) -> Option<fossil_graph_schema::Term> {
    use fossil_graph_schema::Term;
    let datatype = components
        .iter()
        .filter_map(|c| match c {
            ASTComponent::Datatype(datatypes) => Some(datatypes),
            _ => None,
        })
        .flatten()
        .filter_map(|d| d.get_iri().ok())
        .map(rudof_iri::IriS::as_str)
        .min();
    if let Some(dt) = datatype {
        return Some(Term::Literal(Some(Xsd::new(dt))));
    }
    let node_kind = components.iter().find_map(|c| match c {
        ASTComponent::NodeKind(kinds) => kinds.first(),
        _ => None,
    });
    match node_kind {
        Some(NodeKind::Iri) => Some(Term::Iri),
        Some(NodeKind::Lit) => Some(Term::Literal(None)),
        _ => None,
    }
}

/// A property shape's annotations: SHACL's own two, `sh:name` and
/// `sh:description`, read as `rdfs:label` and `rdfs:comment` — one predicate
/// for a label whichever document gave it — and first, as SHACL §2.3.2.1 says
/// a tool prefers the locally specified label; then every non-SHACL triple.
fn property_annotations(property: &ASTPropertyShape) -> Vec<Annotation> {
    messages(RDFS_LABEL, property.name())
        .chain(messages(RDFS_COMMENT, property.description()))
        .chain(extra_annotations(property.extra()))
        .collect()
}

fn messages<'a>(predicate: &'a str, map: &'a MessageMap) -> impl Iterator<Item = Annotation> + 'a {
    map.iter().map(move |(language, value)| {
        Annotation::literal(
            predicate,
            value,
            language.as_ref().map(ToString::to_string).as_deref(),
        )
    })
}

/// Every triple of a shape outside SHACL's namespace and not `rdf:type` — the
/// non-validating properties §2.3.2 says a processor ignores — as an
/// annotation, in the AST's order (predicate, then value).
fn extra_annotations(extra: &[(rudof_iri::IriS, Object)]) -> Vec<Annotation> {
    extra
        .iter()
        .map(|(predicate, object)| Annotation {
            predicate: predicate.as_str().to_string(),
            object: match object {
                Object::Iri(_) | Object::BlankNode(_) => AnnotationObject::Iri(label_of(object)),
                Object::Literal(literal) => {
                    let language = literal.lang().map(|l| l.to_string());
                    // A tagged string has `rdf:langString` for a datatype and a
                    // plain one `xsd:string`; neither is written beside the value.
                    let datatype = if language.is_some() {
                        None
                    } else {
                        literal
                            .datatype()
                            .get_iri()
                            .ok()
                            .map(|d| d.as_str().to_string())
                            .filter(|d| d != xsd::STRING.as_str())
                    };
                    AnnotationObject::Literal(ObjectLiteral {
                        value: literal.lexical_form(),
                        language,
                        datatype,
                    })
                }
                Object::Triple { .. } => AnnotationObject::Literal(ObjectLiteral {
                    value: object.to_string(),
                    language: None,
                    datatype: None,
                }),
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use fossil_graph_schema::{Cardinality, EdgeType};

    use super::*;
    use fossil_graph_schema::Primitive;

    // One node shape exercising every canonical mapping: a typed literal
    // (→ a narrowed constraint, single-valued via sh:maxCount 1), an opaque IRI
    // (sh:nodeKind sh:IRI → Uri), a plain edge (sh:class), and an OR edge
    // (sh:or → two targets on one constraint, one EdgeType each).
    const SHAPES: &str = r"
@prefix sh:  <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ex:  <https://ex.org/> .

ex:PersonShape a sh:NodeShape ;
    sh:targetClass ex:Person ;
    sh:property [ sh:path ex:name     ; sh:datatype xsd:string ; sh:maxCount 1 ] ;
    sh:property [ sh:path ex:homepage ; sh:nodeKind sh:IRI ] ;
    sh:property [ sh:path ex:knows    ; sh:class ex:Person ] ;
    sh:property [ sh:path ex:contact  ; sh:or ( [ sh:class ex:Person ] [ sh:class ex:Org ] ) ] .
";

    #[test]
    fn shacl_lowers_literals_iris_and_edges_into_the_neutral_vocabulary() {
        let doc = decode_shacl("shapes.ttl", SHAPES).expect("turtle parses");
        assert_eq!(doc.shapes().count(), 1);
        let person = doc.lookup("https://ex.org/Person").expect("targetClass");

        let by = |p: &str| {
            person
                .properties
                .iter()
                .find(|c| c.predicate == format!("https://ex.org/{p}"))
                .unwrap_or_else(|| panic!("{p}"))
        };
        assert_eq!(by("name").datatype(), Some(Primitive::STRING));
        assert_eq!(
            by("name").occurs,
            Occurs {
                min: 0,
                max: Some(1)
            }
        );
        assert_eq!(
            by("homepage").datatype(),
            Some(Primitive::Uri),
            "`sh:nodeKind sh:IRI` with no class is an opaque IRI column"
        );
        assert!(by("homepage").targets.is_empty(), "and not an edge");
        assert_eq!(by("knows").targets, ["https://ex.org/Person"]);
        assert_eq!(
            by("contact").targets,
            ["https://ex.org/Org", "https://ex.org/Person"],
            "`sh:or` is one constraint with two destinations, sorted: the order \
             of an `sh:or` list does not change what the shape validates"
        );
    }

    /// The executor consumes `GraphSchema`: one node per shape, its literal
    /// properties with their cardinalities, and one edge per alternative.
    #[test]
    fn the_graph_schema_has_the_nodes_properties_and_edges_the_document_declares() {
        let gs = decode_shacl("shapes.ttl", SHAPES)
            .expect("parses")
            .to_graph_schema();
        assert_eq!(gs.nodes.len(), 1);
        assert_eq!(gs.nodes[0].label, "Person");
        assert_eq!(gs.nodes[0].iri.as_deref(), Some("https://ex.org/Person"));

        let names: Vec<&str> = gs.nodes[0]
            .properties
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["homepage", "name"],
            "edges are not properties; the rest are by predicate, the document's order unread"
        );
        assert_eq!(gs.nodes[0].properties[0].cardinality, Cardinality::Multi);
        assert_eq!(gs.nodes[0].properties[1].cardinality, Cardinality::Single);

        let mut contact: Vec<&EdgeType> =
            gs.edges.iter().filter(|e| e.label == "contact").collect();
        contact.sort_by(|a, b| a.destination.cmp(&b.destination));
        assert_eq!(contact.len(), 2, "one edge per alternative");
        assert_eq!(contact[0].destination, "Org");
        assert_eq!(contact[1].destination, "Person");
        assert!(contact.iter().all(|e| e.source == "Person"));
    }

    /// Shapes come out by the type's IRI, whichever way the document lists them:
    /// the same two shapes written in either order decode to one document.
    #[test]
    fn shapes_come_out_by_iri_whichever_way_the_document_lists_them() {
        const PREFIXES: &str = r"
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <https://ex.org/> .
";
        const ZETA: &str = "ex:ZetaShape  a sh:NodeShape ; sh:targetClass ex:Zeta .\n";
        const ALPHA: &str = "ex:AlphaShape a sh:NodeShape ; sh:targetClass ex:Alpha .\n";
        let decode = |body: String| decode_shacl("two.ttl", &body).expect("parses");
        let zeta_first = decode(format!("{PREFIXES}{ZETA}{ALPHA}"));
        let alpha_first = decode(format!("{PREFIXES}{ALPHA}{ZETA}"));
        let order: Vec<&str> = zeta_first.shapes().map(|s| s.iri.as_str()).collect();
        assert_eq!(order, ["https://ex.org/Alpha", "https://ex.org/Zeta"]);
        assert_eq!(zeta_first, alpha_first, "the document's order is not read");
    }

    /// A property shape with `sh:order` comes before one without, ascending, and
    /// the rest by the predicate's IRI — in any order the document lists them.
    #[test]
    fn properties_come_out_by_sh_order_then_predicate() {
        const A: &str = "[ sh:path ex:alpha ; sh:datatype xsd:string ]";
        const B: &str = "[ sh:path ex:beta ; sh:datatype xsd:string ; sh:order 1 ]";
        const C: &str = "[ sh:path ex:gamma ; sh:datatype xsd:string ; sh:order 0 ]";
        const D: &str = "[ sh:path ex:delta ; sh:datatype xsd:string ]";
        let decode = |properties: [&str; 4]| {
            let property = properties
                .map(|p| format!("sh:property {p}"))
                .join(" ;\n    ");
            let doc = format!(
                r"@prefix sh:  <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ex:  <https://ex.org/> .
ex:PersonShape a sh:NodeShape ; sh:targetClass ex:Person ;
    {property} ."
            );
            let doc = decode_shacl("p.ttl", &doc).expect("parses");
            let person = doc.lookup("https://ex.org/Person").expect("shape");
            person
                .properties
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>()
        };
        let expected = ["gamma", "beta", "alpha", "delta"];
        assert_eq!(decode([A, B, C, D]), expected);
        assert_eq!(decode([D, C, B, A]), expected, "the listing is not read");
    }

    /// Blank-node shapes with no `sh:targetClass` have no IRI: they follow the
    /// shapes that have one, by their predicates, and two the key cannot tell
    /// apart are the error `shape/ambiguous-order`.
    #[test]
    fn shapes_that_are_blank_nodes_follow_and_a_tie_is_a_rejection() {
        const HEAD: &str = r"
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <https://ex.org/> .
ex:Zeta a sh:NodeShape ; sh:targetClass ex:Zeta .
";
        let one = |path: &str| format!("[] a sh:NodeShape ; sh:property [ sh:path ex:{path} ] .\n");
        let doc =
            decode_shacl("b.ttl", &format!("{HEAD}{}{}", one("b"), one("a"))).expect("parses");
        let first_predicate: Vec<&str> = doc
            .shapes()
            .map(|s| s.properties.first().map_or("", |p| p.name.as_str()))
            .collect();
        assert_eq!(first_predicate, ["", "a", "b"], "Zeta, then by predicate");
        assert!(doc.rejections().is_empty());

        let tied =
            decode_shacl("b.ttl", &format!("{HEAD}{}{}", one("a"), one("a"))).expect("parses");
        assert!(
            matches!(tied.rejections(), [Rejection::AmbiguousOrder { .. }]),
            "got {:?}",
            tied.rejections()
        );
    }

    /// What fossil types a document by is not what the AST holds as a shape: the
    /// members of an `sh:or` list are shapes to SHACL and not to fossil, a bare
    /// `sh:NodeShape` is both, and a shape with several `sh:targetClass` is named
    /// by the smallest.
    #[test]
    fn a_node_shape_is_one_that_declares_a_type() {
        const DOC: &str = r"
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <https://ex.org/> .
ex:Bare a sh:NodeShape .
ex:OnlyComponents sh:class ex:Thing .
ex:Two sh:targetClass ex:Zed, ex:Aye .
ex:Holder a sh:NodeShape ; sh:property [ sh:path ex:p ; sh:or ( [ sh:class ex:A ] ex:OnlyComponents ) ] .
";
        let doc = decode_shacl("n.ttl", DOC).expect("parses");
        let iris: Vec<&str> = doc.shapes().map(|s| s.iri.as_str()).collect();
        assert_eq!(
            iris,
            [
                "https://ex.org/Aye",
                "https://ex.org/Bare",
                "https://ex.org/Holder"
            ]
        );
    }

    /// A shape with no `sh:targetClass` is typed by its own IRI.
    #[test]
    fn a_shape_without_a_target_class_is_typed_by_itself() {
        const SELF_TYPED: &str = r"
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <https://ex.org/> .

ex:Thing a sh:NodeShape ; sh:property [ sh:path ex:label ] .
";
        let doc = decode_shacl("t.ttl", SELF_TYPED).expect("parses");
        assert!(doc.lookup("https://ex.org/Thing").is_some());
    }

    /// Text that is not Turtle is a rejection the caller can put in front of a
    /// user, not a panic and not a silent empty document.
    #[test]
    fn text_that_is_not_turtle_is_malformed() {
        let err = decode_shacl("broken.ttl", "{ this is not turtle").expect_err("must fail");
        assert!(matches!(err, Rejection::Malformed(_)), "got {err:?}");
    }

    /// `sh:name` and `sh:description` are read as `rdfs:label` and
    /// `rdfs:comment`, ahead of any the property shape writes under those
    /// predicates; every other non-SHACL triple is carried as itself; and a
    /// `sh:datatype rdf:langString` with `sh:languageIn` is a `String<Lang>`
    /// whose tags are the list.
    #[test]
    fn sh_name_is_a_label_and_language_in_is_the_languages() {
        use fossil_graph_schema::{Annotation, RDF_LANG_STRING, RDFS_COMMENT, RDFS_LABEL};
        const LABELLED: &str = r#"
@prefix sh:   <http://www.w3.org/ns/shacl#> .
@prefix rdf:  <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ex:   <https://ex.org/> .

ex:PersonShape a sh:NodeShape ;
    sh:targetClass ex:Person ;
    rdfs:label "person"@en ;
    sh:property [
        sh:path ex:name ;
        sh:datatype rdf:langString ;
        sh:languageIn ( "en" "es" ) ;
        sh:name "nombre"@es ;
        sh:description "how they are called" ;
        rdfs:label "name"@en ;
    ] .
"#;
        let doc = decode_shacl("p.ttl", LABELLED).expect("parses");
        let person = doc.lookup("https://ex.org/Person").expect("the shape");
        assert_eq!(
            person.annotations,
            [Annotation::literal(RDFS_LABEL, "person", Some("en"))]
        );
        let name = &person.properties[0];
        assert_eq!(
            name.annotations,
            [
                Annotation::literal(RDFS_LABEL, "nombre", Some("es")),
                Annotation::literal(RDFS_COMMENT, "how they are called", None),
                Annotation::literal(RDFS_LABEL, "name", Some("en")),
            ]
        );
        assert_eq!(name.datatype(), Some(Primitive::LANG_STRING));
        assert_eq!(
            name.term,
            Some(fossil_graph_schema::Term::Literal(Some(
                fossil_graph_schema::Xsd::new(RDF_LANG_STRING)
            )))
        );
        assert_eq!(name.languages, ["en", "es"]);
    }
}
