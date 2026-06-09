//! E2E del backend DataFusion (paso 3, vertex-only): `hello.fossil` →
//! [`fossil_df::execute_vertex`] → ejecuta el plan sobre un CSV real y comprueba
//! las filas materializadas. Cierra el gap "compila pero no se ha ejecutado":
//! valida MIR-PG → `LogicalPlan` → `read_csv` → `Projection` → `collect()`.
//!
//! El cwd de `cargo test` es la raíz del crate, así que la fuente se referencia
//! como `tests/fixtures/users.csv` (relativa a `crates/fossil-df`).

#![cfg(not(target_arch = "wasm32"))]

use std::sync::Arc;

use datafusion::arrow::array::{Array, StringArray};
use fossil_base::{FossilDb, NativeSystem, SourceFile, System};
use fossil_hir::def_map::def_map;

const HELLO: &str = "\
prefix ex: <https://example.org/>

users := io.csv(\"tests/fixtures/users.csv\")

User : ex:Person from users
    iri = `${ex:}user/${.id}`
    ex:name = .name
";

#[tokio::test]
async fn execute_vertex_projects_subject_and_props() {
    let system: Arc<dyn System> = Arc::new(NativeSystem::default());
    let db = FossilDb::new(system);
    let file = SourceFile::new(&db, HELLO.to_string(), "hello.fossil".to_string());
    let mapping = *def_map(&db, file)
        .mappings(&db)
        .first()
        .expect("hello.fossil must contain one mapping");

    let batches = fossil_df::execute_vertex(&db, mapping)
        .await
        .expect("execute_vertex runs the DataFusion plan");

    let total: usize = batches.iter().map(|b| b.num_rows()).sum();
    assert_eq!(total, 3, "users.csv has 3 data rows");

    let batch = batches.first().expect("at least one RecordBatch");
    // Projection shape: subject (the id template) + each literal prop.
    let schema = batch.schema();
    let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(names, ["subject", "name"], "projection = subject + props");

    let subject = batch
        .column(0)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("subject is a string");
    assert_eq!(
        subject.value(0),
        "https://example.org/user/1",
        "subject = expanded `${{ex:}}user/${{.id}}` template"
    );

    let name = batch
        .column(1)
        .as_any()
        .downcast_ref::<StringArray>()
        .expect("name is a string");
    let values: Vec<&str> = (0..name.len()).map(|i| name.value(i)).collect();
    assert_eq!(values, ["Alice", "Bob", "Carol"]);
}
