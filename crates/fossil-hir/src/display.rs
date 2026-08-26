//! The source spelling of a LOWERED node — what the compiler understood, said
//! back in the surface syntax.
//!
//! Not a formatter. A formatter reads the CST and preserves what the author
//! wrote; this reads the HIR and can therefore only say what survived lowering.
//! That difference is the point of the module: the census in
//! `crates/fossil-cli/tests/census` commits a per-program artefact
//! recording the compiler's READING, and an
//! artefact rendered from source text proves nothing, because the source text is
//! what it would be checked against.
//!
//! It is why `compound-key` could not be blessed. Its whole claim is that a join
//! on two columns is a join on two columns, and the census printed
//! `Joined := LineRow.join(?)` — the constructor scan finds a callee and a first
//! positional string, and a pipeline has no string. Dropping the `tenant`
//! conjunct turns four joined rows into seven, but the seven mint the same four
//! subjects, so the vertex count the artefact DID carry is equal either way.
//! The key had to be in the artefact or the artefact could not tell the two
//! apart.
//!
//! Spans are not consulted, and **there is no round-trip claim**. Nothing here
//! promises the author's spacing, their parentheses, or that re-parsing the
//! output yields the tree it came from — no test asserts it and no caller needs
//! it. What the guards below DO prove is the one property the artefact rests on:
//! two programs that differ in the tree render differently.

use std::fmt::Write as _;

use crate::lower::{BinOp, HirExpr, HirSourceOp, HirSourcePipe, InterpolationPart, UnOp};

/// The source spelling of a unary operator.
#[must_use]
pub const fn un_op_text(op: UnOp) -> &'static str {
    match op {
        UnOp::Neg => "-",
        UnOp::Not => "not",
    }
}

/// The source spelling of a binary operator.
#[must_use]
pub const fn op_text(op: BinOp) -> &'static str {
    match op {
        BinOp::Eq => "==",
        BinOp::Ne => "!=",
        BinOp::Lt => "<",
        BinOp::Le => "<=",
        BinOp::Gt => ">",
        BinOp::Ge => ">=",
        BinOp::And => "and",
        BinOp::Or => "or",
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
    }
}

/// A lowered expression, back in surface syntax.
///
/// Nested binary and ternary operands are parenthesised unconditionally. The
/// alternative is a precedence table, which would be a SECOND statement of the
/// precedences the parser already fixes, and the two would drift the first time
/// one moved. Parentheses that the author did not write are noise; parentheses
/// that contradict the tree are a lie, and only one of those is worth avoiding.
#[must_use]
pub fn expr_text(expr: &HirExpr) -> String {
    let mut out = String::new();
    write_expr(&mut out, expr);
    out
}

fn write_expr(out: &mut String, expr: &HirExpr) {
    match expr {
        HirExpr::Interpolation(parts) => {
            out.push('"');
            for part in parts {
                match part {
                    InterpolationPart::Text(t) => out.push_str(t),
                    InterpolationPart::Hole(e) => {
                        out.push('{');
                        write_expr(out, e);
                        out.push('}');
                    }
                }
            }
            out.push('"');
        }
        HirExpr::NullLit => out.push_str("null"),
        HirExpr::FieldRef(name) => out.push_str(name),
        HirExpr::ColumnRef { binding, column } => {
            let _ = write!(out, "{binding}.{column}");
        }
        // The literal runs of an interpolation are already inside quotes, and a
        // `StringLit` reaching here is a whole expression, so it needs its own.
        HirExpr::StringLit(s) => {
            let _ = write!(out, "\"{s}\"");
        }
        HirExpr::Call { func, args } => write_call(out, func, args),
        // `Person(User.email)` — an edge is spelled as the TYPE applied to the
        // identity of the node it reaches, which is a call shape whose callee is
        // a type name.
        HirExpr::Edge { target, args } => write_call(out, target, args),
        HirExpr::IntLit(n) => {
            let _ = write!(out, "{n}");
        }
        HirExpr::FloatLit(f) => {
            let _ = write!(out, "{}", f.get());
        }
        HirExpr::BoolLit(b) => {
            let _ = write!(out, "{b}");
        }
        HirExpr::BinOp { op, lhs, rhs } => {
            write_operand(out, lhs);
            let _ = write!(out, " {} ", op_text(*op));
            write_operand(out, rhs);
        }
        HirExpr::UnaryOp { op, operand } => {
            out.push_str(un_op_text(*op));
            // `not x`, `-x`: the word operator needs the space and the sigil
            // does not take one.
            if un_op_text(*op).chars().all(char::is_alphabetic) {
                out.push(' ');
            }
            write_operand(out, operand);
        }
        HirExpr::Ternary {
            cond,
            then,
            otherwise,
        } => {
            // `cond ? a : b` — `grammar.bnf`, `TernaryExpr`. There is no
            // `if`/`then`/`else` in the surface; `IfThenElseExpr` is listed
            // under what the grammar reaches ONLY via `?:`.
            write_operand(out, cond);
            out.push_str(" ? ");
            write_operand(out, then);
            out.push_str(" : ");
            write_operand(out, otherwise);
        }
    }
}

/// An operand of an operator: parenthesised iff it is itself compound.
fn write_operand(out: &mut String, expr: &HirExpr) {
    let compound = matches!(
        expr,
        HirExpr::BinOp { .. } | HirExpr::UnaryOp { .. } | HirExpr::Ternary { .. }
    );
    if compound {
        out.push('(');
    }
    write_expr(out, expr);
    if compound {
        out.push(')');
    }
}

fn write_call(out: &mut String, callee: &str, args: &[HirExpr]) {
    out.push_str(callee);
    out.push('(');
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        write_expr(out, arg);
    }
    out.push(')');
}

/// A derived source binding's right-hand side, back in surface syntax —
/// `LineRow.join(OrderRow, on = LineRow.order_id == OrderRow.id)`.
///
/// The base name and every stage in written order, which is the order the field
/// holds them in. The binding's own name is NOT here: a caller that has the pipe
/// has the name beside it, and rendering `name := rhs` from inside would decide
/// the binder's spelling for every caller.
#[must_use]
pub fn pipe_text(pipe: &HirSourcePipe) -> String {
    let mut out = pipe.base.to_string();
    for op in &pipe.ops {
        match op {
            HirSourceOp::Where(pred) => {
                out.push_str(".where(");
                write_expr(&mut out, pred);
                out.push(')');
            }
            HirSourceOp::Select(cols) => {
                out.push_str(".select(");
                for (i, c) in cols.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    let _ = write!(out, "{}.{}", c.binding, c.column);
                }
                out.push(')');
            }
            HirSourceOp::Join { right, alias, on } => {
                let _ = write!(out, ".join({right}");
                if let Some(alias) = alias {
                    let _ = write!(out, " as {alias}");
                }
                out.push_str(", on = ");
                write_expr(&mut out, on);
                out.push(')');
            }
            HirSourceOp::Distinct => out.push_str(".distinct()"),
            HirSourceOp::GroupBy { keys, aggs } => {
                out.push_str(".group_by(");
                let mut parts: Vec<String> = keys
                    .iter()
                    .map(|k| format!("{}.{}", k.binding, k.column))
                    .collect();
                parts.extend(aggs.iter().map(|a| {
                    format!(
                        "{} = {}({}.{})",
                        a.out, a.func, a.column.binding, a.column.column
                    )
                }));
                out.push_str(&parts.join(", "));
                out.push(')');
            }
            HirSourceOp::Union { right } => {
                let _ = write!(out, ".union({right})");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use fossil_base::test_support::NativeSystem;
    use fossil_base::{FossilDb, SourceFile, System};

    use super::{expr_text, pipe_text};
    use crate::lower::{HirSourceOp, lower_to_hir};

    fn pipes(src: &str) -> Vec<String> {
        let system: Arc<dyn System> = Arc::new(NativeSystem::default());
        let db = FossilDb::new(system);
        let file = SourceFile::new(&db, src.to_string(), "test.fossil".to_string());
        lower_to_hir(&db, file)
            .source_pipes(&db)
            .iter()
            .map(pipe_text)
            .collect()
    }

    /// The claim the module exists for, stated as the difference it has to
    /// carry.
    ///
    /// Not «the rendering equals this string» — that restates the renderer. Two
    /// programs are compiled that differ ONLY in whether the join key has a
    /// second conjunct, and the two renderings have to differ. They are the two
    /// programs the conformance artefact could not tell apart: `compound-key`
    /// with its `tenant` conjunct joins four rows and without it seven, and the
    /// seven mint the same four subjects, so every count downstream is equal.
    #[test]
    fn a_dropped_conjunct_changes_the_rendering() {
        const ONE: &str = "\
L := io.csv(\"lines.csv\")
O := io.csv(\"orders.csv\")
J := L.join(O, on = L.order_id == O.id)
";
        const TWO: &str = "\
L := io.csv(\"lines.csv\")
O := io.csv(\"orders.csv\")
J := L.join(O, on = L.order_id == O.id and L.tenant == O.tenant)
";
        let one = pipes(ONE);
        let two = pipes(TWO);
        assert_eq!(one.len(), 1, "one pipeline, got {one:?}");
        assert_eq!(two.len(), 1, "one pipeline, got {two:?}");
        assert_ne!(
            one[0], two[0],
            "a compound key and a single-column key must not render the same"
        );
        assert!(
            two[0].contains("L.tenant") && two[0].contains("O.tenant"),
            "both halves of the second conjunct belong in the rendering: {}",
            two[0]
        );
    }

    /// The alias is the only thing that tells the two sides of a self-join
    /// apart, so a rendering that drops it describes a relation joined to
    /// itself on nothing distinguishable.
    #[test]
    fn a_self_join_renders_its_alias() {
        const SRC: &str = "\
N := io.csv(\"categories.csv\")
P := N.join(N as Other, on = N.parent == Other.id)
";
        let rendered = pipes(SRC);
        assert_eq!(rendered.len(), 1, "one pipeline, got {rendered:?}");
        assert!(
            rendered[0].contains("N as Other"),
            "the alias is the whole content of a self-join: {}",
            rendered[0]
        );
    }

    /// Every stage, in written order — a pipeline is not its last verb.
    #[test]
    fn the_stages_render_in_written_order() {
        const SRC: &str = "\
U := io.csv(\"users.csv\")
O := io.csv(\"orders.csv\")
P := U.where(U.age >= 18).join(O, on = U.id == O.user_id)
";
        let rendered = pipes(SRC);
        assert_eq!(rendered.len(), 1, "one pipeline, got {rendered:?}");
        let where_at = rendered[0].find(".where(").expect("the where stage");
        let join_at = rendered[0].find(".join(").expect("the join stage");
        assert!(
            where_at < join_at,
            "the filter was written before the join: {}",
            rendered[0]
        );
    }

    /// A nested operand is parenthesised, so that the rendering cannot be read
    /// as a tree the parser would not build. `a and b == c` and `a and (b == c)`
    /// are the same tree; `(a and b) == c` is a different one, and nothing in
    /// the rendering may leave a reader to guess which was lowered.
    #[test]
    fn a_nested_operand_is_parenthesised() {
        const SRC: &str = "\
U := io.csv(\"users.csv\")
P := U.where(U.a == U.b and U.c == U.d)
";
        let rendered = pipes(SRC);
        assert_eq!(rendered.len(), 1, "one pipeline, got {rendered:?}");
        assert!(
            rendered[0].contains("(U.a == U.b) and (U.c == U.d)"),
            "each conjunct is a compound operand and takes its parentheses: {}",
            rendered[0]
        );
    }

    /// The renderer reads the HIR and nothing else. A predicate the lowering
    /// did not build cannot appear, which is the property that makes the
    /// artefact evidence rather than an echo of its input.
    #[test]
    fn the_rendering_comes_from_the_hir_and_not_the_text() {
        const SRC: &str = "\
U := io.csv(\"users.csv\")
P := U.where(U.age >= 18)
";
        let system: Arc<dyn System> = Arc::new(NativeSystem::default());
        let db = FossilDb::new(system);
        let file = SourceFile::new(&db, SRC.to_string(), "test.fossil".to_string());
        let hir = lower_to_hir(&db, file);
        let pipe = hir.source_pipes(&db).first().expect("one pipeline");
        let HirSourceOp::Where(pred) = &pipe.ops[0] else {
            panic!("the one stage is a where, got {:?}", pipe.ops[0]);
        };
        assert_eq!(
            expr_text(pred),
            "U.age >= 18",
            "the predicate rendered is the predicate lowered"
        );
    }
}
