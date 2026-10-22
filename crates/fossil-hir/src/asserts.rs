//! What a declared type says beyond its fields' types: `@assert(…)` on a type
//! or a field, and a field's `@severity(…)` — and the checker's half of
//! enforcing them (`/docs/design/declared`).
//!
//! # The scope rule
//!
//! An assertion is an expression over bare names, and which names depends on
//! where it is written. **On a field**, the field's own name is ONE of its
//! values, and the assertion holds of each value: a `Seq<String>` is checked
//! element by element. **On a type**, the names are its single-valued fields,
//! and the assertion holds of each node: a field with many values has no one
//! value to compare. A value that is absent is not checked — SHACL's rule,
//! which is what makes `Option<T>` and an assertion compose.
//!
//! # What is decided here and what waits for the data
//!
//! [`check_asserts`](crate::asserts::check_asserts) decides what the program alone decides: that a condition
//! is a `Bool` and reads only names in scope. The checker
//! ([`crate::check::Checker::check_property`]) folds a CONSTANT a mapping
//! writes through the field's assertions, at that line ([`fold`](crate::asserts::fold)). Everything
//! else is a per-value check the run makes, and there are no facts about the
//! data here: the checker reads the program and its documents, nothing else.

use fossil_base::{Diagnostic, Diagnostics, SourceFile, SpanFrame};
use fossil_graph_schema::{ConstraintSeverity, Problem, PropertyConstraint, Span};
use smol_str::SmolStr;

use crate::lower::{BinOp, HirExpr, UnOp};
use crate::ty::{NamedRow, Record, RecordField, Rows, Ty, TyKind};

/// One `@assert(…)` of a declared type, lowered.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeclaredAssert {
    /// The IRI of the type it is written on.
    pub shape: SmolStr,
    /// The type's name in the program, for a message.
    pub type_name: SmolStr,
    /// The field it is written on; `None` on the type.
    pub field: Option<SmolStr>,
    /// The condition.
    pub condition: HirExpr,
    /// The condition as written, for a message.
    pub text: SmolStr,
    /// How it is enforced — `severity = …`, else a violation.
    pub severity: ConstraintSeverity,
    /// Where the condition is, file-absolute.
    pub span: Span,
}

impl DeclaredAssert {
    /// «the field `edad`», «the type `Persona`» — which assertion a message is
    /// about.
    fn on(&self) -> String {
        self.field.as_ref().map_or_else(
            || format!("the type `{}`", self.type_name),
            |f| format!("the field `{f}`"),
        )
    }
}

/// A field's `@severity(…)`: how its count is enforced. A field without one
/// is enforced as a [`ConstraintSeverity::Violation`] and has no row here.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CountSeverity {
    /// The IRI of the type.
    pub shape: SmolStr,
    /// The field.
    pub field: SmolStr,
    /// How its count is enforced.
    pub severity: ConstraintSeverity,
}

/// Everything a program's declarations constrain beyond their fields' types.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct DeclaredRules {
    /// Every `@assert`, in written order.
    pub asserts: Vec<DeclaredAssert>,
    /// Every `@severity`, in written order.
    pub counts: Vec<CountSeverity>,
}

impl DeclaredRules {
    /// The assertions on `field` of the type `shape`.
    pub fn on_field<'a>(
        &'a self,
        shape: &'a str,
        field: &'a str,
    ) -> impl Iterator<Item = &'a DeclaredAssert> + 'a {
        self.asserts
            .iter()
            .filter(move |a| a.shape == shape && a.field.as_deref() == Some(field))
    }
}

/// Check every assertion the program declares: it is a `Bool`, and it reads
/// only the names its scope gives it. File-absolute, one diagnostic per
/// refusal.
///
/// A query of its own, beside [`crate::identity::check_identities`], and not
/// part of [`crate::lower::lower_to_hir`]: typing an expression may ask which
/// types exist, and that question is answered by the lowering this would be
/// inside.
#[salsa::tracked(returns(ref))]
pub fn check_asserts(db: &dyn fossil_base::Db, file: SourceFile) -> Vec<Diagnostic> {
    let hir = crate::lower::lower_to_hir(db, file);
    let mut sink = Diagnostics::new();
    for assert in &hir.rules(db).asserts {
        let Some(shape) = hir
            .declared(db)
            .shapes()
            .find(|s| s.iri == assert.shape.as_str())
        else {
            continue;
        };
        check_one(db, file, &mut sink, assert, &shape.properties);
    }
    sink.into_vec()
}

fn check_one(
    db: &dyn fossil_base::Db,
    file: SourceFile,
    sink: &mut Diagnostics,
    assert: &DeclaredAssert,
    fields: &[PropertyConstraint],
) {
    let in_scope: Vec<&PropertyConstraint> = fields
        .iter()
        .filter(|f| {
            assert
                .field
                .as_ref()
                .map_or_else(|| f.occurs.is_single_valued(), |own| f.name == own.as_str())
        })
        .collect();

    // A name the type has and the scope does not give is refused for what it
    // is, before the expression is typed: «not a field of `Persona`» would be
    // false of a field `Persona` has.
    let mut names = Vec::new();
    field_refs(&assert.condition, &mut names);
    let mut refused = false;
    for name in names {
        if in_scope.iter().any(|f| f.name == name.as_str())
            || !fields.iter().any(|f| f.name == name.as_str())
        {
            continue;
        }
        refused = true;
        let help = if assert.field.is_some() {
            format!(
                "an assertion on a field reads that field's value alone. To relate two fields, \
                 assert on the type: `@assert(…)` above `type {}`",
                assert.type_name
            )
        } else {
            format!(
                "an assertion on a type reads one value of each field, and `{name}` holds \
                 many. Assert on the field instead, and each of its values is checked"
            )
        };
        sink.push(
            Diagnostic::new(
                fossil_graph_schema::Severity::Error,
                Problem::OutOfScope {
                    field: name.to_string(),
                    on: assert.on(),
                },
                assert.span,
            )
            .with_help(help)
            .file_absolute(),
        );
    }
    if refused {
        return;
    }

    // A field whose own type was refused has no type to read; its refusal is
    // already reported, and anything said here would be about a guess.
    let Some(record) = in_scope
        .iter()
        .map(|f| {
            crate::shapes::expected_value_ty(db, f).map(|ty| RecordField {
                name: SmolStr::from(f.name.as_str()),
                ty,
            })
        })
        .collect::<Option<Vec<_>>>()
    else {
        return;
    };
    let row = Ty::new(db, TyKind::Record(Record::new(db, record)));
    // An unnameable binding: a name is written bare, and `Persona.edad` is a
    // second spelling the scope does not give.
    let rows = Rows::of(vec![NamedRow {
        binding: SmolStr::default(),
        row: Some(row),
    }]);
    let mut cx =
        crate::check::Expr::over_relation(db, file, assert.type_name.clone(), rows, assert.span);
    let ty = cx.synth(crate::body::ExprId(0), &assert.condition);
    sink.extend(cx.sink.into_vec());
    if let Some(ty) = ty
        && !matches!(ty.kind(db), TyKind::Error(_))
        && !crate::check::subtypes(
            db,
            ty,
            Ty::new(db, TyKind::Primitive(fossil_graph_schema::Primitive::Bool)),
        )
    {
        sink.push(
            Diagnostic::new(
                fossil_graph_schema::Severity::Error,
                Problem::ExpectedBool {
                    operand: format!("the assertion on {}", assert.on()),
                    actual: crate::ty::display::render_ty_kind(db, ty.kind(db)),
                },
                assert.span,
            )
            .file_absolute(),
        );
    }
}

/// Every bare name `e` reads.
fn field_refs(e: &HirExpr, out: &mut Vec<SmolStr>) {
    match e {
        HirExpr::FieldRef(name) => out.push(name.clone()),
        HirExpr::BinOp { lhs, rhs, .. } => {
            field_refs(lhs, out);
            field_refs(rhs, out);
        }
        HirExpr::UnaryOp { operand, .. } => field_refs(operand, out),
        HirExpr::Call { args, .. } | HirExpr::Edge { args, .. } => {
            for a in args {
                field_refs(a, out);
            }
        }
        HirExpr::Ternary {
            cond,
            then,
            otherwise,
        } => {
            field_refs(cond, out);
            field_refs(then, out);
            field_refs(otherwise, out);
        }
        HirExpr::Interpolation(parts) => {
            for p in parts {
                if let crate::lower::InterpolationPart::Hole(h) = p {
                    field_refs(h, out);
                }
            }
        }
        _ => {}
    }
}

/// A constant, as [`fold`] computes with it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Constant {
    Int(i64),
    Float(f64),
    Text(SmolStr),
    Bool(bool),
}

impl Constant {
    /// The value as a program writes it, for a message.
    pub(crate) fn spelled(&self) -> String {
        match self {
            Self::Int(i) => i.to_string(),
            Self::Float(f) => f.to_string(),
            Self::Text(t) => format!("{t:?}"),
            Self::Bool(b) => b.to_string(),
        }
    }

    const fn number(&self) -> Option<f64> {
        match self {
            // An `i64` past 2^53 rounds; a comparison against a literal that
            // large is not one a program writes.
            #[allow(clippy::cast_precision_loss)]
            Self::Int(i) => Some(*i as f64),
            Self::Float(f) => Some(*f),
            _ => None,
        }
    }
}

/// `e`'s value, when it is decided without the data: literals, `field` bound
/// to `value`, and the operators and calls whose meaning is fixed here. `None`
/// is «not decided», never «false» — a call this does not know, `null`, an
/// overflow — and the run decides it.
pub(crate) fn fold(e: &HirExpr, field: &str, value: Option<&Constant>) -> Option<Constant> {
    use Constant::{Bool, Float, Int, Text};
    Some(match e {
        HirExpr::IntLit(i) => Int(*i),
        HirExpr::FloatLit(f) => Float(f.get()),
        HirExpr::StringLit(s) => Text(s.clone()),
        HirExpr::LangStringLit { value, .. } => Text(value.clone()),
        HirExpr::BoolLit(b) => Bool(*b),
        HirExpr::FieldRef(name) if name == field => value?.clone(),
        HirExpr::UnaryOp { op, operand } => match (op, fold(operand, field, value)?) {
            (UnOp::Not, Bool(b)) => Bool(!b),
            (UnOp::Neg, Int(i)) => Int(i.checked_neg()?),
            (UnOp::Neg, Float(f)) => Float(-f),
            _ => return None,
        },
        HirExpr::BinOp { op, lhs, rhs } => {
            let l = fold(lhs, field, value)?;
            let r = fold(rhs, field, value)?;
            binop(*op, &l, &r)?
        }
        HirExpr::Call { func, args } => {
            let args: Vec<Constant> = args
                .iter()
                .map(|a| fold(a, field, value))
                .collect::<Option<_>>()?;
            match (func.as_str(), args.as_slice()) {
                ("str.length", [Text(t)]) => Int(i64::try_from(t.chars().count()).ok()?),
                ("str.contains", [Text(t), Text(n)]) => Bool(t.contains(n.as_str())),
                ("str.starts_with", [Text(t), Text(p)]) => Bool(t.starts_with(p.as_str())),
                ("str.ends_with", [Text(t), Text(s)]) => Bool(t.ends_with(s.as_str())),
                _ => return None,
            }
        }
        _ => return None,
    })
}

fn binop(op: BinOp, l: &Constant, r: &Constant) -> Option<Constant> {
    use Constant::{Bool, Float, Int, Text};
    use std::cmp::Ordering;
    let ordering = || -> Option<Ordering> {
        match (l, r) {
            (Text(a), Text(b)) => Some(a.cmp(b)),
            (Bool(a), Bool(b)) => Some(a.cmp(b)),
            (Int(a), Int(b)) => Some(a.cmp(b)),
            _ => l.number()?.partial_cmp(&r.number()?),
        }
    };
    Some(match op {
        BinOp::Eq => Bool(ordering()? == Ordering::Equal),
        BinOp::Ne => Bool(ordering()? != Ordering::Equal),
        BinOp::Lt => Bool(ordering()? == Ordering::Less),
        BinOp::Le => Bool(ordering()? != Ordering::Greater),
        BinOp::Gt => Bool(ordering()? == Ordering::Greater),
        BinOp::Ge => Bool(ordering()? != Ordering::Less),
        BinOp::And => match (l, r) {
            (Bool(a), Bool(b)) => Bool(*a && *b),
            _ => return None,
        },
        BinOp::Or => match (l, r) {
            (Bool(a), Bool(b)) => Bool(*a || *b),
            _ => return None,
        },
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Rem => {
            if let (Int(a), Int(b)) = (l, r) {
                Int(match op {
                    BinOp::Add => a.checked_add(*b)?,
                    BinOp::Sub => a.checked_sub(*b)?,
                    BinOp::Mul => a.checked_mul(*b)?,
                    _ => a.checked_rem(*b)?,
                })
            } else {
                let (a, b) = (l.number()?, r.number()?);
                Float(match op {
                    BinOp::Add => a + b,
                    BinOp::Sub => a - b,
                    BinOp::Mul => a * b,
                    _ => a % b,
                })
            }
        }
        // `/` is always float division (`crate::check`'s `synth_binop`).
        BinOp::Div => Float(l.number()? / r.number()?),
    })
}

/// The label an [`Problem::AssertFailed`] carries at the assertion it failed.
pub(crate) fn label(d: Diagnostic, assert: &DeclaredAssert) -> Diagnostic {
    d.with_label(assert.span, "the assertion", SpanFrame::FileAbsolute)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lower::FloatBits;

    fn field(name: &str) -> HirExpr {
        HirExpr::FieldRef(name.into())
    }

    #[test]
    fn a_constant_folds_through_the_condition() {
        // edad >= 0 and edad < 150
        let cond = HirExpr::BinOp {
            op: BinOp::And,
            lhs: Box::new(HirExpr::BinOp {
                op: BinOp::Ge,
                lhs: Box::new(field("edad")),
                rhs: Box::new(HirExpr::IntLit(0)),
            }),
            rhs: Box::new(HirExpr::BinOp {
                op: BinOp::Lt,
                lhs: Box::new(field("edad")),
                rhs: Box::new(HirExpr::IntLit(150)),
            }),
        };
        assert_eq!(
            fold(&cond, "edad", Some(&Constant::Int(200))),
            Some(Constant::Bool(false))
        );
        assert_eq!(
            fold(&cond, "edad", Some(&Constant::Int(30))),
            Some(Constant::Bool(true))
        );
        assert_eq!(
            fold(&cond, "edad", Some(&Constant::Float(149.5))),
            Some(Constant::Bool(true))
        );
        // No value: not decided, and the run decides it.
        assert_eq!(fold(&cond, "edad", None), None);
    }

    #[test]
    fn what_is_not_known_here_is_not_decided() {
        let unknown_call = HirExpr::Call {
            func: "str.matches".into(),
            args: vec![
                HirExpr::FieldRef("email".into()),
                HirExpr::StringLit("@".into()),
            ],
        };
        assert_eq!(
            fold(&unknown_call, "email", Some(&Constant::Text("a@b".into()))),
            None
        );
        let overflow = HirExpr::BinOp {
            op: BinOp::Add,
            lhs: Box::new(HirExpr::IntLit(i64::MAX)),
            rhs: Box::new(HirExpr::IntLit(1)),
        };
        assert_eq!(fold(&overflow, "x", None), None);
        let length = HirExpr::BinOp {
            op: BinOp::Le,
            lhs: Box::new(HirExpr::Call {
                func: "str.length".into(),
                args: vec![HirExpr::FieldRef("code".into())],
            }),
            rhs: Box::new(HirExpr::FloatLit(FloatBits::new(3.0))),
        };
        assert_eq!(
            fold(&length, "code", Some(&Constant::Text("ñandú".into()))),
            Some(Constant::Bool(false))
        );
    }
}
