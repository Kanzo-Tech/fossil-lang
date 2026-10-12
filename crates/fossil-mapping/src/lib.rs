//! `fossil-mapping` — a mapping as data, and the Fossil program emitted from it.
//!
//! A model does not write Fossil (`/docs/design/profile`). It fills a [`Mapping`]: the shape
//! document and the types it declares, the sources, and per type RML's triples map — a source,
//! the joins it takes, a subject template, and per property a column, a `parse.*` call over one, or
//! a link to another type's subject. Every name in it is one a host can enumerate: a source's
//! columns from its profile, a type's properties from the shapes. [`emit`] writes the program,
//! deterministically, and the program goes through the same checker, conformance and run as one
//! written by hand. So a syntax error is not something a model can cause, and everything it writes
//! is JSON under a schema, the one constraint a browser's model runtime enforces.
//!
//! The emitter refuses only what would not be Fossil at all — a name that is not an identifier, a
//! source or a type the spec does not declare — each at a JSON Pointer into the spec, the shape a
//! Task's diagnostic takes. Whether a column exists, or a property is the shape's, is the checker's.

use std::fmt::Write as _;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A whole mapping: what the program imports, reads and writes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(rename = "MappingSpec")]
pub struct Mapping {
    /// Predicates a shape declares whose last segment is not the key a map should write.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renames: Vec<Rename>,
    pub shapes: Shapes,
    pub sources: Vec<SourceSpec>,
    pub maps: Vec<TriplesMap>,
}

/// `@rename(Shape, "iri" as name)`: the key `name` writes the predicate `iri` of `shape`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Rename {
    pub shape: String,
    pub iri: String,
    pub name: String,
}

/// The shape document, and the types of it the maps write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Shapes {
    /// The constructor that reads it: `shex` or `shacl`.
    pub format: String,
    /// Where it is, as a program writes it: `hello.shex`, `@vocab/person.shex`.
    pub reference: String,
    pub types: Vec<String>,
}

/// One source the program reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceSpec {
    /// The binding every reference to it is qualified by: `User`.
    pub name: String,
    /// The constructor that reads it: `csv`, `json`, `parquet`.
    pub format: String,
    /// Where it is, as a program writes it.
    pub reference: String,
    /// The reader's option — a CSV's delimiter — when it is not the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delimiter: Option<String>,
}

/// One type written from one source: RML's triples map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TriplesMap {
    /// The mapping's own name: `People`.
    pub name: String,
    /// The type it writes, one of [`Shapes::types`].
    pub shape: String,
    /// The source it reads, one of [`Mapping::sources`].
    pub source: String,
    /// The rows it keeps: every condition holds.
    #[serde(default, rename = "where", skip_serializing_if = "Vec::is_empty")]
    pub filter: Vec<Condition>,
    /// Sources joined to it, in order, each on one pair of columns.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub joins: Vec<Join>,
    /// The subject: text and columns, in order — `https://example.org/user/` then `User.id`.
    pub subject: Vec<Part>,
    pub properties: Vec<Property>,
}

/// A column compared with a literal: `PlaceRow.isPartOf == null`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Condition {
    pub column: Column,
    pub op: Op,
    pub value: Literal,
}

/// A comparison a condition makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum Op {
    #[serde(rename = "==")]
    Eq,
    #[serde(rename = "!=")]
    Ne,
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = "<=")]
    Le,
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = ">=")]
    Ge,
}

impl Op {
    const fn text(self) -> &'static str {
        match self {
            Self::Eq => "==",
            Self::Ne => "!=",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
        }
    }
}

/// What a column is compared with: `null`, a boolean, a whole number or text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Literal {
    Null(()),
    Bool(bool),
    Integer(i64),
    Text(String),
}

impl Literal {
    fn text(&self) -> String {
        match self {
            Self::Null(()) => "null".into(),
            Self::Bool(b) => b.to_string(),
            Self::Integer(n) => n.to_string(),
            Self::Text(t) => string(t),
        }
    }
}

/// A source joined to a map's rows where `left` equals `right`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Join {
    pub source: String,
    pub left: Column,
    pub right: Column,
}

/// A column of a source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Column {
    pub source: String,
    pub column: String,
}

/// A piece of a subject template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Part {
    Text { text: String },
    Column { column: Column },
}

/// One property of the type, and where its value comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Property {
    /// The property's key: the last segment of a predicate the shape declares.
    pub key: String,
    pub value: Value,
}

/// A property's value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Value {
    /// The column, as it is.
    Column { column: Column },
    /// A `parse.*` function of the catalogue over the column — `parse.date`, with its format.
    Parse {
        function: String,
        column: Column,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        format: Option<String>,
    },
    /// The subject of a `shape` whose key is the column: an edge.
    Link { shape: String, column: Column },
}

/// Why a spec is not a program, and where in it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{at}: {message}")]
pub struct Invalid {
    /// A JSON Pointer into the spec: `/maps/0/properties/2/value/column/source`.
    pub at: String,
    pub message: String,
}

const KEYWORDS: [&str; 7] = ["from", "and", "or", "not", "true", "false", "null"];

/// `IDENT` in `grammar.bnf`, less the words that are tokens of their own.
fn is_ident(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
        && !KEYWORDS.contains(&name)
}

/// A string literal's content: its escapes, and `{{` for a brace no hole opens.
fn escaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '{' => out.push_str("{{"),
            c => out.push(c),
        }
    }
    out
}

fn string(text: &str) -> String {
    format!("\"{}\"", escaped(text))
}

/// `name`, or why a program cannot write it.
fn named(at: &str, name: &str) -> Result<(), Invalid> {
    if is_ident(name) {
        Ok(())
    } else {
        Err(Invalid {
            at: at.into(),
            message: format!("`{name}` is not a name a program can write"),
        })
    }
}

struct Emitter<'a> {
    spec: &'a Mapping,
}

impl Emitter<'_> {
    fn source(&self, at: &str, name: &str) -> Result<(), Invalid> {
        if self.spec.sources.iter().any(|s| s.name == name) {
            Ok(())
        } else {
            Err(Invalid {
                at: at.into(),
                message: format!("`{name}` is not one of the sources"),
            })
        }
    }

    fn shape(&self, at: &str, name: &str) -> Result<(), Invalid> {
        if self.spec.shapes.types.iter().any(|t| t == name) {
            Ok(())
        } else {
            Err(Invalid {
                at: at.into(),
                message: format!("`{name}` is not one of the shapes' types"),
            })
        }
    }

    /// `Source.column`, or `Source."a.column"` for one that is not a name.
    fn column(&self, at: &str, column: &Column) -> Result<String, Invalid> {
        self.source(&format!("{at}/source"), &column.source)?;
        Ok(if is_ident(&column.column) {
            format!("{}.{}", column.source, column.column)
        } else {
            format!("{}.{}", column.source, string(&column.column))
        })
    }

    fn value(&self, at: &str, value: &Value) -> Result<String, Invalid> {
        match value {
            Value::Column { column } => self.column(&format!("{at}/column"), column),
            Value::Parse {
                function,
                column,
                format,
            } => {
                let call = function
                    .strip_prefix("parse.")
                    .filter(|f| is_ident(f))
                    .ok_or_else(|| Invalid {
                        at: format!("{at}/function"),
                        message: format!("`{function}` is not a `parse.` function"),
                    })?;
                let column = self.column(&format!("{at}/column"), column)?;
                let format = format
                    .as_deref()
                    .map(|f| format!(", {}", string(f)))
                    .unwrap_or_default();
                Ok(format!("parse.{call}({column}{format})"))
            }
            Value::Link { shape, column } => {
                self.shape(&format!("{at}/shape"), shape)?;
                Ok(format!(
                    "{shape}({})",
                    self.column(&format!("{at}/column"), column)?
                ))
            }
        }
    }

    fn map(&self, at: &str, map: &TriplesMap, out: &mut String) -> Result<(), Invalid> {
        named(&format!("{at}/name"), &map.name)?;
        self.shape(&format!("{at}/shape"), &map.shape)?;
        self.source(&format!("{at}/source"), &map.source)?;
        let mut from = map.source.clone();
        if !map.filter.is_empty() {
            let mut conditions = Vec::new();
            for (n, condition) in map.filter.iter().enumerate() {
                let column = self.column(&format!("{at}/where/{n}/column"), &condition.column)?;
                conditions.push(format!(
                    "{column} {} {}",
                    condition.op.text(),
                    condition.value.text()
                ));
            }
            let _ = write!(from, ".where({})", conditions.join(" and "));
        }
        for (n, join) in map.joins.iter().enumerate() {
            let at = format!("{at}/joins/{n}");
            self.source(&format!("{at}/source"), &join.source)?;
            let left = self.column(&format!("{at}/left"), &join.left)?;
            let right = self.column(&format!("{at}/right"), &join.right)?;
            let _ = write!(from, ".join({}, on = {left} == {right})", join.source);
        }
        let _ = writeln!(out, "{} : {} from {from}", map.name, map.shape);

        let mut subject = String::new();
        for (n, part) in map.subject.iter().enumerate() {
            match part {
                Part::Text { text } => subject.push_str(&escaped(text)),
                Part::Column { column } => {
                    let _ = write!(
                        subject,
                        "{{{}}}",
                        self.column(&format!("{at}/subject/{n}/column"), column)?
                    );
                }
            }
        }
        let mut lines = vec![("@subject".to_string(), format!("\"{subject}\""))];
        for (n, property) in map.properties.iter().enumerate() {
            let at = format!("{at}/properties/{n}");
            named(&format!("{at}/key"), &property.key)?;
            lines.push((
                property.key.clone(),
                self.value(&format!("{at}/value"), &property.value)?,
            ));
        }
        let width = lines
            .iter()
            .map(|(key, _)| key.chars().count())
            .max()
            .unwrap_or(0);
        for (key, value) in lines {
            let _ = writeln!(out, "    {key:width$} = {value}");
        }
        Ok(())
    }
}

/// The program `spec` is: the shapes imported, the sources bound, and a mapping per triples map,
/// in the spec's order, aligned as a person would align them.
///
/// # Errors
///
/// [`Invalid`], at the first place in the spec that no program could say.
pub fn emit(spec: &Mapping) -> Result<String, Invalid> {
    let emitter = Emitter { spec };
    let mut out = String::new();

    // One attribute per shape, its renames in the spec's order.
    let mut shapes: Vec<&str> = Vec::new();
    for rename in &spec.renames {
        if !shapes.contains(&rename.shape.as_str()) {
            shapes.push(&rename.shape);
        }
    }
    for shape in shapes {
        let mut renames = Vec::new();
        for (n, rename) in spec
            .renames
            .iter()
            .enumerate()
            .filter(|(_, r)| r.shape == shape)
        {
            emitter.shape(&format!("/renames/{n}/shape"), &rename.shape)?;
            named(&format!("/renames/{n}/name"), &rename.name)?;
            renames.push(format!("{} as {}", string(&rename.iri), rename.name));
        }
        let _ = writeln!(out, "@rename({shape}, {})", renames.join(", "));
    }

    named("/shapes/format", &spec.shapes.format)?;
    for (n, name) in spec.shapes.types.iter().enumerate() {
        named(&format!("/shapes/types/{n}"), name)?;
    }
    let _ = writeln!(
        out,
        "type {{ {} }} := io.{}({})\n",
        spec.shapes.types.join(", "),
        spec.shapes.format,
        string(&spec.shapes.reference)
    );

    let width = spec
        .sources
        .iter()
        .map(|s| s.name.chars().count())
        .max()
        .unwrap_or(0);
    for (n, source) in spec.sources.iter().enumerate() {
        named(&format!("/sources/{n}/name"), &source.name)?;
        named(&format!("/sources/{n}/format"), &source.format)?;
        let delimiter = source
            .delimiter
            .as_deref()
            .map(|d| format!(", delimiter = {}", string(d)))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "{:width$} := io.{}({}{delimiter})",
            source.name,
            source.format,
            string(&source.reference)
        );
    }

    for (n, map) in spec.maps.iter().enumerate() {
        out.push('\n');
        emitter.map(&format!("/maps/{n}"), map, &mut out)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hello() -> Mapping {
        let user = |column: &str| Column {
            source: "User".into(),
            column: column.into(),
        };
        Mapping {
            renames: vec![],
            shapes: Shapes {
                format: "shex".into(),
                reference: "hello.shex".into(),
                types: vec!["Person".into()],
            },
            sources: vec![SourceSpec {
                name: "User".into(),
                format: "csv".into(),
                reference: "users.csv".into(),
                delimiter: None,
            }],
            maps: vec![TriplesMap {
                name: "People".into(),
                shape: "Person".into(),
                source: "User".into(),
                filter: vec![],
                joins: vec![],
                subject: vec![
                    Part::Text {
                        text: "https://example.org/user/".into(),
                    },
                    Part::Column { column: user("id") },
                ],
                properties: vec![Property {
                    key: "name".into(),
                    value: Value::Column {
                        column: user("name"),
                    },
                }],
            }],
        }
    }

    #[test]
    fn the_rows_a_map_keeps_are_a_where_on_its_source() {
        let mut spec = hello();
        let parent = Column {
            source: "User".into(),
            column: "isPartOf".into(),
        };
        spec.maps[0].filter = vec![
            Condition {
                column: parent.clone(),
                op: Op::Eq,
                value: Literal::Null(()),
            },
            Condition {
                column: parent,
                op: Op::Ne,
                value: Literal::Text("x".into()),
            },
        ];
        let program = emit(&spec).unwrap();
        assert!(
            program.contains(
                r#"People : Person from User.where(User.isPartOf == null and User.isPartOf != "x")"#
            ),
            "{program}"
        );
        let read: Mapping = serde_json::from_str(r#"{"shapes":{"format":"shex","reference":"s","types":["P"]},"sources":[],"maps":[],"renames":[]}"#).unwrap();
        assert!(read.renames.is_empty());
    }

    #[test]
    fn a_name_that_is_not_one_is_refused_where_it_is() {
        let mut spec = hello();
        spec.maps[0].properties[0].key = "full name".into();
        assert_eq!(emit(&spec).unwrap_err().at, "/maps/0/properties/0/key");
        let mut spec = hello();
        spec.maps[0].properties[0].value = Value::Column {
            column: Column {
                source: "Users".into(),
                column: "name".into(),
            },
        };
        assert_eq!(
            emit(&spec).unwrap_err().at,
            "/maps/0/properties/0/value/column/source"
        );
    }

    #[test]
    fn a_column_that_is_not_a_name_is_quoted_and_text_is_escaped() {
        let mut spec = hello();
        spec.maps[0].subject[0] = Part::Text {
            text: "urn:{x}:\"q\"/".into(),
        };
        spec.maps[0].properties[0].value = Value::Column {
            column: Column {
                source: "User".into(),
                column: "Person.id".into(),
            },
        };
        let program = emit(&spec).unwrap();
        assert!(
            program.contains(r#"@subject = "urn:{{x}:\"q\"/{User.id}""#),
            "{program}"
        );
        assert!(
            program.contains(r#"name     = User."Person.id""#),
            "{program}"
        );
    }
}
