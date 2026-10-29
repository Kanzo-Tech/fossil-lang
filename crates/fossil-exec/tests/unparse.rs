//! **Experiment.** Replays what `src/unparse.rs` recorded: each plan's `DuckDB`
//! SQL, run on native `DuckDB` over the very tables `DataFusion` scanned, and
//! compared with `DataFusion`'s answer — the column types first, then the rows
//! as multisets. Does nothing unless `FOSSIL_UNPARSE_DIR` names a recording.
//!
//! ```text
//! FOSSIL_UNPARSE_DIR=/tmp/u cargo test -p fossil-exec --features unparse --test programs
//! FOSSIL_UNPARSE_DIR=/tmp/u cargo test -p fossil-exec --features unparse --test unparse -- --nocapture
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use duckdb::Connection;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Outcome {
    Same,
    Unparsed,
    Refused,
    Types,
    Rows,
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn types(conn: &Connection, relation: &str) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(&format!("DESCRIBE {relation}"))
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| e.to_string())?;
    rows.collect::<Result<_, _>>().map_err(|e| e.to_string())
}

fn count(conn: &Connection, sql: &str) -> Result<i64, String> {
    conn.query_row(sql, [], |row| row.get(0))
        .map_err(|e| e.to_string())
}

fn replay(dir: &Path) -> (Outcome, String) {
    let Ok(sql) = fs::read_to_string(dir.join("query.sql")) else {
        let why = fs::read_to_string(dir.join("error.txt")).unwrap_or_default();
        return (Outcome::Unparsed, why);
    };
    let conn = Connection::open_in_memory().expect("an in-memory DuckDB");
    let tables = fs::read_to_string(dir.join("tables.tsv")).unwrap_or_default();
    for line in tables.lines() {
        let Some((name, file)) = line.split_once('\t') else {
            continue;
        };
        let path = dir.join(file);
        let view = format!(
            "CREATE OR REPLACE VIEW \"{}\" AS SELECT * FROM read_parquet({})",
            name.replace('"', "\"\""),
            quote(&path.to_string_lossy())
        );
        if let Err(e) = conn.execute_batch(&view) {
            return (Outcome::Refused, format!("view {name}: {e}"));
        }
    }
    if let Err(e) = conn.execute_batch(&format!("CREATE TEMP TABLE q AS {sql}")) {
        return (Outcome::Refused, format!("{e}\n  SQL: {sql}"));
    }
    let expected = format!(
        "read_parquet({})",
        quote(&dir.join("result.parquet").to_string_lossy())
    );
    let (ours, theirs) = match (types(&conn, "q"), types(&conn, &format!("SELECT * FROM {expected}"))) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return (Outcome::Refused, e),
    };
    if ours != theirs {
        return (
            Outcome::Types,
            format!("DuckDB {ours:?}\n  DataFusion {theirs:?}\n  SQL: {sql}"),
        );
    }
    let missing = count(
        &conn,
        &format!("SELECT count(*) FROM (SELECT * FROM {expected} EXCEPT ALL SELECT * FROM q)"),
    );
    let extra = count(
        &conn,
        &format!("SELECT count(*) FROM (SELECT * FROM q EXCEPT ALL SELECT * FROM {expected})"),
    );
    match (missing, extra) {
        (Ok(0), Ok(0)) => (Outcome::Same, String::new()),
        (Ok(m), Ok(x)) => (
            Outcome::Rows,
            format!("{m} rows missing, {x} extra\n  SQL: {sql}"),
        ),
        (Err(e), _) | (_, Err(e)) => (Outcome::Refused, e),
    }
}

#[test]
fn every_recorded_plan_replays_on_duckdb() {
    let Ok(root) = std::env::var("FOSSIL_UNPARSE_DIR") else {
        return;
    };
    let mut dirs: Vec<_> = fs::read_dir(&root)
        .expect("the recording")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    let mut tally: BTreeMap<Outcome, Vec<(String, String)>> = BTreeMap::new();
    for dir in &dirs {
        let (outcome, why) = replay(dir);
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        tally.entry(outcome).or_default().push((name, why));
    }
    println!("{} plans recorded", dirs.len());
    for (outcome, cases) in &tally {
        println!("{outcome:?}: {}", cases.len());
    }
    for (outcome, cases) in &tally {
        if *outcome == Outcome::Same {
            continue;
        }
        println!("\n== {outcome:?}");
        for (name, why) in cases.iter().take(12) {
            println!("- {name}: {}", why.chars().take(1200).collect::<String>());
        }
    }
}
