//! The catalogue's own guards: the row set, the receiver, and the templates.

use super::*;

// A `db()` helper stood here, and its only user was the test that materialised
// an interned `FnSig`. The catalogue's own guards need no database: a row is a
// `'static` description, which is the whole point of `SigSpec` being `'db`-free.

/// The registry answers, and says no when it should.
#[test]
fn the_registry_answers_and_refuses() {
    let r = FunctionRegistry::stdlib_default();
    assert!(r.lookup("io.csv").is_some());
    assert!(r.lookup("nonexistent").is_none());
}

// ── The receiver ───────────────────────────────────────────────────────────

/// `recv` and `member` are DERIVED, never passed — so no row can carry a
/// receiver that disagrees with the name written beside it.
///
/// This is the guard the whole change rests on. A `recv` field a call site
/// filled by hand would be a field that drifts, which is the defect one level
/// down (dispatch by string) reappearing one level up.
#[test]
fn every_rows_receiver_agrees_with_its_own_name() {
    let r = FunctionRegistry::stdlib_default();
    for e in r.iter() {
        let (recv, member) = split_receiver(e.name.as_str());
        assert_eq!(
            e.recv, recv,
            "`{}` carries a receiver its name denies",
            e.name
        );
        assert_eq!(
            e.member, member,
            "`{}` carries a member its name denies",
            e.name
        );
        assert!(
            e.name.contains('.'),
            "`{}` is not a dotted catalogue name",
            e.name
        );
    }
}

/// The type path and the value path reach ONE row.
///
/// `str.lower(str.trim(x))` and `x.trim().lower()` are the same entry, which is
/// what `grammar.bnf, PostfixExpr` promises and what the `expressions` and
/// `contacts` conformance programs spell on purpose.
#[test]
fn the_type_path_and_the_value_path_reach_the_same_row() {
    let r = FunctionRegistry::stdlib_default();
    for name in ["str.trim", "str.lower", "str.upper", "str.slug"] {
        let by_path = r.lookup(name).unwrap_or_else(|| panic!("`{name}` present"));
        let member = name.split_once('.').expect("dotted").1;
        let by_member = r
            .lookup_member(Receiver::Scalar(ScalarTy::String), member)
            .unwrap_or_else(|| panic!("`{member}` reachable on a String value"));
        assert_eq!(
            by_path.name, by_member.name,
            "`{name}` must be ONE row reached two ways"
        );
    }
}

/// A namespace row is reachable by its dotted name and by NOTHING else.
///
/// `io` is the clearest case: its entries CREATE the thing, so
/// there is no receiver to hang them off and `x.csv()` must not resolve.
#[test]
fn a_namespace_row_has_no_value_path() {
    let r = FunctionRegistry::stdlib_default();
    assert!(r.lookup("io.csv").is_some());
    assert_eq!(
        r.candidates_for_member("csv").count(),
        0,
        "`io.csv` must not be reachable as a member of a value"
    );
    for e in r.iter().filter(|e| e.recv == Receiver::Namespace) {
        assert!(
            r.lookup_member(Receiver::Namespace, e.member.as_str())
                .is_some(),
            "a Namespace row must still be findable by receiver+member"
        );
    }
}

/// `members_of` is what makes completion stop being approximate, so it has to
/// actually partition the catalogue.
#[test]
fn members_of_partitions_the_catalogue() {
    let r = FunctionRegistry::stdlib_default();
    let total = r.iter().count();
    let by_recv = r.members_of(Receiver::Namespace).count()
        + r.members_of(Receiver::Scalar(ScalarTy::String)).count()
        + r.members_of(Receiver::Relation).count();
    assert_eq!(by_recv, total, "every row belongs to exactly one receiver");
    // The relation's members are the verbs: `where` and `join` are catalogue
    // ROWS and not grammar productions, so a new verb is a row, never a rule.
    assert!(
        r.lookup_member(Receiver::Relation, "where").is_some(),
        "`where` must be a member of a relation"
    );
    assert!(
        r.lookup_member(Receiver::Relation, "join").is_some(),
        "`join` must be a member of a relation"
    );
}

/// The value path resolves a member by name, so an ambiguous member would make
/// it guess. Today none is ambiguous, and this is what says so out loud —
/// `crate::lower` has an arm for the ambiguous case that is unreachable while
/// this passes.
#[test]
fn no_member_is_spelled_on_two_receivers() {
    let r = FunctionRegistry::stdlib_default();
    for e in r.iter().filter(|e| e.recv != Receiver::Namespace) {
        let n = r.candidates_for_member(e.member.as_str()).count();
        assert_eq!(
            n, 1,
            "`{}` is spelled on {n} receivers; the value path cannot resolve it",
            e.member
        );
    }
}

// ── The lowering ───────────────────────────────────────────────────────────

#[test]
fn anon_redact_is_a_template_that_ignores_its_argument() {
    let r = FunctionRegistry::stdlib_default();
    let redact = r.lookup("anon.redact").expect("anon.redact present");
    let LoweringKind::Expr(t) = &redact.lowering else {
        panic!("anon.redact must be an Expr row, got {:?}", redact.lowering);
    };
    assert_eq!(t.as_str(), "'[REDACTED]'");
    // The ZERO-hole case, and half the reason the notation is indexed rather
    // than sequential: a sequential placeholder cannot say "ignore".
    assert!(template_holes(t.as_str()).is_empty());
    assert_eq!(redact.sig.params.len(), 1);
}

#[test]
fn core_require_is_a_template_that_reads_its_argument_twice() {
    let r = FunctionRegistry::stdlib_default();
    let require = r.lookup("core.require").expect("core.require present");
    let LoweringKind::Expr(t) = &require.lowering else {
        panic!("core.require must be an Expr row");
    };
    // The REPEATED-hole case, the other half of the reason. `%0` twice, one
    // parameter: a sequential placeholder would demand two.
    assert_eq!(template_holes(t.as_str()), vec![0]);
    assert_eq!(t.as_str().matches("%0").count(), 2);
    assert_eq!(require.sig.params.len(), 1);
}

/// **A template may never name an argument the signature does not have.**
///
/// This is the one invariant the notation needs and the only one it can be
/// given: a template does NOT determine arity (see the two tests above), so the
/// check is one-directional — every hole is a parameter, but not every
/// parameter need be a hole.
#[test]
fn no_template_names_a_hole_its_signature_lacks() {
    let r = FunctionRegistry::stdlib_default();
    for e in r.iter() {
        let LoweringKind::Expr(t) = &e.lowering else {
            continue;
        };
        for hole in template_holes(t.as_str()) {
            assert!(
                hole < e.sig.params.len(),
                "`{}` names `%{hole}` and declares {} parameter(s): {t}",
                e.name,
                e.sig.params.len()
            );
        }
    }
}

#[test]
fn render_template_substitutes_by_index_and_escapes_double_percent() {
    let args = ["A".to_string(), "B".to_string()];
    assert_eq!(render_template("f(%0, %1)", &args), "f(A, B)");
    // Repeated and out-of-order.
    assert_eq!(render_template("%1 || %0 || %1", &args), "B || A || B");
    // `%%` is a literal percent, so a LIKE pattern survives.
    assert_eq!(render_template("like(%0, '%%')", &args), "like(A, '%')");
    // An out-of-range hole is left verbatim rather than dropped: a template
    // that names an argument the signature lacks must be VISIBLE, and the
    // guard above is what catches it.
    assert_eq!(render_template("f(%9)", &args), "f(%9)");
}

// ── The row set ────────────────────────────────────────────────────────────

#[test]
fn math_namespace_has_exactly_six_no_ceil_floor() {
    let r = FunctionRegistry::stdlib_default();
    let math: Vec<&str> = r
        .iter()
        .map(|en| en.name.as_str())
        .filter(|n| n.starts_with("math."))
        .collect();
    assert_eq!(
        math.len(),
        6,
        "math/ must have exactly 6 functions, got {math:?}"
    );
    assert!(r.lookup("math.ceil").is_none(), "math.ceil must NOT exist");
    assert!(
        r.lookup("math.floor").is_none(),
        "math.floor must NOT exist"
    );
}

/// The authoritative function set. `io/` is excluded (source constructors).
fn expected_stdlib_names() -> Vec<&'static str> {
    vec![
        // core/ (2)
        "core.lang",
        "core.require",
        // seq/ (13) — the relation verbs. `filter`/`project` are spelled
        // `where`/`select`, which is what the surface spells.
        "seq.where",
        "seq.map",
        "seq.flatten",
        "seq.take",
        "seq.drop",
        "seq.distinct",
        "seq.sort",
        "seq.select",
        "seq.join",
        "seq.union",
        "seq.group_by",
        "seq.count",
        // parse/ (7)
        "parse.integer",
        "parse.float",
        "parse.decimal",
        "parse.date",
        "parse.datetime",
        "parse.json",
        "parse.csv_row",
        // math/ (6 — NO ceil/floor)
        "math.sum",
        "math.avg",
        "math.min",
        "math.max",
        "math.abs",
        "math.round",
        // str/ (13) — every operation on a string. `normalize_unicode` is NOT
        // among them: it is not in the language.
        "str.length",
        "str.slice",
        "str.contains",
        "str.starts_with",
        "str.ends_with",
        "str.replace",
        "str.split",
        "str.concat",
        "str.trim",
        "str.lower",
        "str.upper",
        "str.slug",
        "str.strip_html",
        // validate/ (5)
        "validate.email",
        "validate.url",
        "validate.uuid",
        "validate.iso_date",
        "validate.regex",
        // anon/ (2) — `anon.hmac` left the language: HMAC needs a key
        // schedule and DuckDB has sha256 and no HMAC.
        "anon.hash",
        "anon.redact",
    ]
}

#[test]
fn catalog_is_bidirectionally_complete() {
    use std::collections::BTreeSet;

    let r = FunctionRegistry::stdlib_default();
    let catalog: BTreeSet<String> = r
        .iter()
        .map(|en| en.name.to_string())
        .filter(|n| !n.starts_with("io."))
        .collect();
    let expected: BTreeSet<String> = expected_stdlib_names()
        .iter()
        .map(|s| (*s).to_string())
        .collect();

    let missing: Vec<&String> = expected.difference(&catalog).collect();
    let extra: Vec<&String> = catalog.difference(&expected).collect();

    assert!(
        missing.is_empty() && extra.is_empty(),
        "catalog must equal the declared set exactly.\n  MISSING: {missing:?}\n  EXTRA: {extra:?}"
    );
    // 2 + 12 + 7 + 6 + 13 + 5 + 2 = 47 surface functions. `seq/` went from 13
    // to 12 with `seq.aggregate`, whose aggregations `seq.group_by` took.
    assert_eq!(catalog.len(), 47);
    // The three io/ constructors on top.
    assert_eq!(r.iter().count(), 47 + 3);
}

/// `clean/` does not exist, and neither do the two functions that were deleted
/// from the language rather than ported.
#[test]
fn the_deleted_names_are_gone() {
    let r = FunctionRegistry::stdlib_default();
    for gone in [
        // These five live in `str/`; the `clean/` namespace does not exist.
        "clean.trim",
        "clean.lower",
        "clean.upper",
        "clean.slug",
        "clean.strip_html",
        // These two cannot be done honestly in SQL.
        "clean.normalize_unicode",
        "anon.hmac",
        // The old spellings of the two renamed verbs.
        "seq.filter",
        "seq.project",
    ] {
        assert!(r.lookup(gone).is_none(), "`{gone}` must not exist");
    }
    assert!(
        !r.is_catalogued_head("clean"),
        "`clean` must not be a catalogued head — a binding may be called `clean`"
    );
}
