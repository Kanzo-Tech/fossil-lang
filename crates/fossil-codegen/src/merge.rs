//! Multi-mapping `SinkPlan` merge — combine the per-mapping decompositions of a
//! whole `.fossil` program into ONE writable plan.
//!
//! [`crate::decompose_for_writer`] runs per mapping; a program with N mappings
//! yields N `(prelude, SinkPlan)` parts. The W0b writer, however, keys vertex
//! output by `type_name` (one Parquet set per type) and edge output by the
//! `src_predicate_dst` triple, so the parts cannot simply be concatenated:
//!
//! - **shared sources** (two mappings reading the same `io.csv`) emit the same
//!   `CREATE VIEW` — `DuckDB` rejects the duplicate. The prelude is de-duplicated.
//! - **same-type mappings** (`Product : ex:Product` AND `ProductCategory :
//!   ex:Product`, both subjects `${prod:}${.sku}`) describe ONE vertex type with
//!   complementary properties. They are merged: each contributor's rows are
//!   `UNION ALL BY NAME`-aligned (missing columns → NULL) and folded to one row
//!   per subject with `max(col)` (NULL-skipping, deterministic). This is RML's
//!   "multiple triples maps, same subject" union, recovered from the mappings.
//! - **same-signature edges** are unioned the same way.
//!
//! The merge is a pure function over the resolved structs (no `Db`, no `DuckDB`) —
//! it only rewrites each merged table's `source_relation` to a union/aggregate
//! subquery, so `writer.rs` needs no change (its `DISTINCT ON (iri)` then runs
//! over already-unique subjects, a harmless no-op).

use std::collections::HashSet;
use std::fmt::Write as _;

use fossil_sinks::decomp::{EdgeTable, IRI_COLUMN, SinkPlan, VertexProperty, VertexTable};
use fossil_sinks::manifest::DEFAULT_CHUNK_SIZE;

/// Merge one program's per-mapping `(prelude, SinkPlan)` parts into a single
/// writable `(prelude, SinkPlan)`.
///
/// The W0b writer materialises in one pass over de-duplicated source views, one
/// vertex table per type (same-type mappings unioned), and one edge table per
/// `src–predicate–dst` signature (same-signature edges unioned).
#[must_use]
pub fn merge_decomposed(parts: &[(String, SinkPlan)]) -> (String, SinkPlan) {
    let chunk_size = parts
        .first()
        .map_or(DEFAULT_CHUNK_SIZE, |(_, p)| p.chunk_size);

    let prelude = merge_prelude(parts.iter().map(|(p, _)| p.as_str()));

    // Group vertices by type and edges by signature, preserving first-seen order
    // so the merged plan (and any snapshot of it) is deterministic.
    let mut vtype_order: Vec<String> = Vec::new();
    let mut vgroups: Vec<(String, Vec<VertexTable>)> = Vec::new();
    let mut etype_order: Vec<(String, String, String)> = Vec::new();
    let mut egroups: Vec<((String, String, String), Vec<EdgeTable>)> = Vec::new();

    for (_, plan) in parts {
        for v in &plan.vertices {
            push_grouped(&mut vtype_order, &mut vgroups, v.type_name.clone(), v.clone());
        }
        for e in &plan.edges {
            let key = (e.src_type.clone(), e.predicate.clone(), e.dst_type.clone());
            push_grouped(&mut etype_order, &mut egroups, key, e.clone());
        }
    }

    let vertices = vgroups
        .into_iter()
        .map(|(_, group)| merge_vertex_group(group))
        .collect();
    let edges = egroups
        .into_iter()
        .map(|(_, group)| merge_edge_group(group))
        .collect();

    (
        prelude,
        SinkPlan {
            vertices,
            edges,
            chunk_size,
        },
    )
}

/// Append `value` to the group keyed by `key`, recording the key on first sight
/// so iteration order is the program's first-seen order (not hash order).
fn push_grouped<K: PartialEq + Clone, V>(
    order: &mut Vec<K>,
    groups: &mut Vec<(K, Vec<V>)>,
    key: K,
    value: V,
) {
    if let Some((_, group)) = groups.iter_mut().find(|(k, _)| *k == key) {
        group.push(value);
    } else {
        order.push(key.clone());
        groups.push((key, vec![value]));
    }
}

/// De-duplicate the `CREATE VIEW` statements across preludes (a source bound by
/// several mappings is declared once), preserving first-seen order.
fn merge_prelude<'a>(preludes: impl Iterator<Item = &'a str>) -> String {
    let mut seen = HashSet::new();
    let mut out = String::new();
    for prelude in preludes {
        for stmt in prelude.split_inclusive(';') {
            let stmt = stmt.trim();
            if stmt.is_empty() || !seen.insert(stmt.to_string()) {
                continue;
            }
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(stmt);
        }
    }
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

/// Merge same-type vertex tables (≥1) into one. A single contributor is returned
/// verbatim; multiple are unioned by column name and folded to one row per
/// subject (`max(col)` skips the NULLs introduced by the alignment).
fn merge_vertex_group(mut group: Vec<VertexTable>) -> VertexTable {
    if group.len() == 1 {
        return group.pop().expect("len == 1");
    }

    // Union of property columns, first-seen data_type wins.
    let mut properties: Vec<VertexProperty> = Vec::new();
    for table in &group {
        for prop in &table.properties {
            if !properties.iter().any(|p| p.name == prop.name) {
                properties.push(prop.clone());
            }
        }
    }

    // Each contributor projects `iri` + its own property columns; UNION ALL BY
    // NAME aligns the differing column sets (missing → NULL).
    let mut union = String::new();
    for (i, table) in group.iter().enumerate() {
        if i > 0 {
            union.push_str(" UNION ALL BY NAME ");
        }
        let _ = write!(union, "SELECT {IRI_COLUMN}");
        for prop in &table.properties {
            let _ = write!(union, ", {}", prop.name);
        }
        let _ = write!(union, " FROM {}", table.source_relation);
    }

    // Fold to one row per subject: max() is NULL-skipping + deterministic.
    let mut agg = String::new();
    for prop in &properties {
        let _ = write!(agg, ", max({name}) AS {name}", name = prop.name);
    }
    let type_name = group[0].type_name.clone();
    let source_relation = format!(
        "(SELECT {IRI_COLUMN}{agg} FROM ({union}) GROUP BY {IRI_COLUMN}) AS merged_{}",
        type_name.to_lowercase(),
    );

    VertexTable {
        type_name,
        vertex_id_col: IRI_COLUMN.to_string(),
        properties,
        source_relation,
    }
}

/// Merge same-signature edge tables (≥1) into one. A single contributor is
/// returned verbatim; multiple are `UNION ALL`-ed over their `(src, dst)` pairs.
fn merge_edge_group(mut group: Vec<EdgeTable>) -> EdgeTable {
    if group.len() == 1 {
        return group.pop().expect("len == 1");
    }

    let head = &group[0];
    // Every contributor exposes the subject as `iri` and the object as the
    // predicate-local column (decomp + base_relation_sql convention), so each
    // projects those two and UNION ALL BY NAME stacks the edge rows.
    let mut union = String::new();
    for (i, edge) in group.iter().enumerate() {
        if i > 0 {
            union.push_str(" UNION ALL BY NAME ");
        }
        let _ = write!(
            union,
            "SELECT {src} AS {IRI_COLUMN}, {dst} AS {pred} FROM {rel}",
            src = edge.src_id_expr,
            dst = edge.dst_id_expr,
            pred = head.predicate,
            rel = edge.source_relation,
        );
    }
    // single_valued only if EVERY contributor is (a conservative AND).
    let single_valued = group.iter().all(|e| e.single_valued);

    EdgeTable {
        src_type: head.src_type.clone(),
        predicate: head.predicate.clone(),
        dst_type: head.dst_type.clone(),
        src_id_expr: IRI_COLUMN.to_string(),
        dst_id_expr: head.predicate.clone(),
        single_valued,
        source_relation: format!("({union}) AS merged_edge"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vtable(type_name: &str, props: &[&str], rel: &str) -> VertexTable {
        VertexTable {
            type_name: type_name.to_string(),
            vertex_id_col: IRI_COLUMN.to_string(),
            properties: props
                .iter()
                .map(|n| VertexProperty {
                    name: (*n).to_string(),
                    data_type: "string".to_string(),
                    single_valued: true,
                })
                .collect(),
            source_relation: rel.to_string(),
        }
    }

    fn plan(vertices: Vec<VertexTable>, edges: Vec<EdgeTable>) -> SinkPlan {
        SinkPlan {
            vertices,
            edges,
            chunk_size: DEFAULT_CHUNK_SIZE,
        }
    }

    #[test]
    fn distinct_types_concatenate() {
        let parts = vec![
            (
                "CREATE VIEW a AS SELECT * FROM x;".to_string(),
                plan(vec![vtable("Person", &["name"], "base_a")], vec![]),
            ),
            (
                "CREATE VIEW b AS SELECT * FROM y;".to_string(),
                plan(vec![vtable("Order", &["total"], "base_b")], vec![]),
            ),
        ];
        let (_pre, merged) = merge_decomposed(&parts);
        assert_eq!(merged.vertices.len(), 2);
        assert_eq!(merged.vertices[0].type_name, "Person");
        assert_eq!(merged.vertices[1].type_name, "Order");
        // Distinct types keep their original source relation verbatim.
        assert_eq!(merged.vertices[0].source_relation, "base_a");
    }

    #[test]
    fn shared_source_view_is_declared_once() {
        let parts = vec![
            (
                "CREATE VIEW products AS SELECT * FROM read_csv('p');".to_string(),
                plan(vec![vtable("Product", &["name"], "base1")], vec![]),
            ),
            (
                "CREATE VIEW products AS SELECT * FROM read_csv('p');".to_string(),
                plan(vec![vtable("PriceSnapshot", &["price"], "base2")], vec![]),
            ),
        ];
        let (prelude, _merged) = merge_decomposed(&parts);
        assert_eq!(
            prelude.matches("CREATE VIEW products").count(),
            1,
            "shared source view de-duplicated: {prelude}"
        );
    }

    #[test]
    fn same_type_mappings_union_with_property_superset() {
        let parts = vec![
            (
                String::new(),
                plan(vec![vtable("Product", &["name", "price"], "base1")], vec![]),
            ),
            (
                String::new(),
                plan(vec![vtable("Product", &["displayName"], "base2")], vec![]),
            ),
        ];
        let (_pre, merged) = merge_decomposed(&parts);
        assert_eq!(merged.vertices.len(), 1, "one merged Product vertex");
        let names: Vec<&str> = merged.vertices[0]
            .properties
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(names, ["name", "price", "displayName"], "property superset");
        let rel = &merged.vertices[0].source_relation;
        assert!(rel.contains("UNION ALL BY NAME"), "{rel}");
        assert!(rel.contains("GROUP BY iri"), "{rel}");
        assert!(rel.contains("max(name) AS name"), "{rel}");
        assert!(rel.contains("max(displayName) AS displayName"), "{rel}");
    }

    #[test]
    fn same_signature_edges_union() {
        let edge = |rel: &str| EdgeTable {
            src_type: "Order".to_string(),
            predicate: "placedBy".to_string(),
            dst_type: "Person".to_string(),
            src_id_expr: IRI_COLUMN.to_string(),
            dst_id_expr: "placedBy".to_string(),
            single_valued: true,
            source_relation: rel.to_string(),
        };
        let parts = vec![
            (String::new(), plan(vec![], vec![edge("e1")])),
            (String::new(), plan(vec![], vec![edge("e2")])),
        ];
        let (_pre, merged) = merge_decomposed(&parts);
        assert_eq!(merged.edges.len(), 1);
        let rel = &merged.edges[0].source_relation;
        assert!(rel.contains("UNION ALL BY NAME"), "{rel}");
        assert!(rel.contains("FROM e1"), "{rel}");
        assert!(rel.contains("FROM e2"), "{rel}");
    }
}
