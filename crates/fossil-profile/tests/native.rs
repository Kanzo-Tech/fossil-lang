//! The whole plan, run by the native host on real files: what a profile of a
//! small Nobel-shaped dataset says, as `DuckDB` reads it. CSV only: the
//! bundled native `DuckDB` installs its `json` extension from the network on
//! first use, which a test must not need.

use std::fmt::Write as _;

use fossil_graph_schema::Primitive;
use fossil_profile::{ColumnRef, Inclusion, Options, Profile, Rows, Source};

/// The host's half: a query on a `duckdb::Connection`, its rows as text.
fn rows(conn: &duckdb::Connection, sql: &str) -> duckdb::Result<Rows> {
    let mut stmt = conn.prepare(sql)?;
    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let n = row.as_ref().column_count();
        out.push(
            (0..n)
                .map(|i| row.get::<_, Option<String>>(i))
                .collect::<duckdb::Result<_>>()?,
        );
    }
    Ok(out)
}

fn profile(sources: impl IntoIterator<Item = Source>, options: Options) -> Profile {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory duckdb");
    fossil_profile::run(sources, options, |sql| {
        rows(&conn, sql).map_err(|e| e.to_string())
    })
}

fn source(dir: &std::path::Path, file: &str, format: &str) -> Source {
    Source {
        key: file.into(),
        name: dir.join(file).display().to_string(),
        format: format.into(),
        option: None,
        etag: format!("etag:{file}"),
    }
}

fn nobel() -> (tempfile::TempDir, Vec<Source>) {
    let dir = tempfile::tempdir().expect("a tempdir");
    std::fs::write(
        dir.path().join("laureates.csv"),
        "id,name\n1,Marie Curie\n2,Pierre Curie\n3,Linus Pauling\n",
    )
    .expect("laureates");
    std::fs::write(
        dir.path().join("awards.csv"),
        "id,laureate,category,share\n\
         1903-physics-1,1,physics,0.25\n\
         1903-physics-2,2,physics,0.25\n\
         1911-chemistry-1,1,chemistry,1.0\n\
         1954-chemistry-3,3,chemistry,1.0\n",
    )
    .expect("awards");
    let sources = vec![
        source(dir.path(), "awards.csv", "csv"),
        source(dir.path(), "laureates.csv", "csv"),
    ];
    (dir, sources)
}

#[test]
fn a_profile_says_what_the_files_are() {
    let (_dir, sources) = nobel();
    let profile = profile(
        sources,
        Options {
            sample: 0,
            shown: 2,
            ..Options::default()
        },
    );

    assert!(profile.failed.is_empty(), "{:?}", profile.failed);
    let [awards, laureates] = &profile.sources[..] else {
        panic!("two sources: {profile:?}")
    };

    let columns: Vec<(&str, Primitive)> = awards
        .descriptor
        .columns
        .iter()
        .map(|c| (c.name.as_str(), c.primitive))
        .collect();
    assert_eq!(
        columns,
        [
            ("id", Primitive::String),
            ("laureate", Primitive::Integer),
            ("category", Primitive::String),
            ("share", Primitive::Float)
        ]
    );
    assert_eq!(
        awards.descriptor.etag, "etag:awards.csv",
        "the host's validator is carried through"
    );
    assert_eq!(awards.rows, Some(4));
    assert!(!awards.sampled);
    assert_eq!(
        awards.keys,
        ["id"],
        "a laureate and a category repeat, and a share is a measurement"
    );
    assert_eq!(laureates.keys, ["id", "name"]);
    assert_eq!(awards.sample.len(), 2);
    assert_eq!(awards.sample[0][0].as_deref(), Some("1903-physics-1"));

    assert!(profile.inclusions.contains(&Inclusion {
        from: ColumnRef {
            source: "awards.csv".into(),
            column: "laureate".into()
        },
        to: ColumnRef {
            source: "laureates.csv".into(),
            column: "id".into()
        },
    }));
}

#[test]
fn an_inclusion_is_a_column_whose_values_are_all_another_sources_key() {
    let dir = tempfile::tempdir().expect("a tempdir");
    std::fs::write(
        dir.path().join("laureates.csv"),
        "id,name\n1,Marie Curie\n2,Pierre Curie\n3,Linus Pauling\n",
    )
    .expect("laureates");
    std::fs::write(
        dir.path().join("awards.csv"),
        "id,laureate\na,1\nb,2\nc,1\nd,3\n",
    )
    .expect("awards");
    std::fs::write(dir.path().join("strays.csv"), "id,laureate\nx,1\ny,9\n").expect("strays");
    let sources =
        ["awards.csv", "laureates.csv", "strays.csv"].map(|f| source(dir.path(), f, "csv"));

    let profile = profile(sources, Options::default());

    let edge = Inclusion {
        from: ColumnRef {
            source: "awards.csv".into(),
            column: "laureate".into(),
        },
        to: ColumnRef {
            source: "laureates.csv".into(),
            column: "id".into(),
        },
    };
    assert!(
        profile.inclusions.contains(&edge),
        "{:?}",
        profile.inclusions
    );
    assert!(
        !profile
            .inclusions
            .iter()
            .any(|i| i.from.source == "strays.csv" && i.from.column == "laureate"),
        "9 is no laureate: {:?}",
        profile.inclusions
    );
}

#[test]
fn a_file_duckdb_cannot_read_is_a_failure_and_the_rest_is_profiled() {
    let (dir, mut sources) = nobel();
    sources.push(source(dir.path(), "missing.csv", "csv"));
    let profile = profile(sources, Options::default());

    assert_eq!(profile.sources.len(), 2);
    assert_eq!(profile.failed.len(), 1);
    assert_eq!(profile.failed[0].source, "missing.csv");
}

#[test]
fn a_sample_reads_the_counts_from_the_first_rows_and_says_so() {
    let dir = tempfile::tempdir().expect("a tempdir");
    let mut body = String::from("id,group\n");
    for i in 0..50 {
        let _ = writeln!(body, "{i},{}", i % 3);
    }
    std::fs::write(dir.path().join("rows.csv"), body).expect("rows");
    let profile = profile(
        [source(dir.path(), "rows.csv", "csv")],
        Options {
            sample: 10,
            shown: 0,
            ..Options::default()
        },
    );

    let rows = &profile.sources[0];
    assert_eq!(rows.rows, Some(10));
    assert!(
        rows.sampled,
        "the counts read a sample, so the keys are candidates"
    );
    assert_eq!(rows.keys, ["id"]);
    assert!(rows.sample.is_empty(), "shown: 0 keeps no rows");
}

/// Every spelling `DuckDB` has for an instant maps to an instant.
///
/// **The table is derived from `DuckDB` and not from memory**: the arms are
/// asserted against the `DESCRIBE` of a value `DuckDB` itself typed, so a
/// version that renames a type fails here rather than silently widening a
/// column to `String`. That widening is what this test exists for —
/// `read_csv_auto` infers `TIMESTAMP WITH TIME ZONE` for any ISO-8601 string
/// carrying an offset, and the checker compares this answer against the
/// shape's declared `Primitive`. So a missing arm is not a slow path: it makes
/// `xsd:dateTime` a hard type error over every offset-bearing column, which is
/// how LDBC-SNB dates every row.
#[test]
fn every_duckdb_instant_is_an_instant() {
    let conn = duckdb::Connection::open_in_memory().expect("in-memory duckdb");
    for (sql, expected) in [
        ("TIMESTAMP '2012-01-01 00:00:00'", Primitive::DateTime),
        ("TIMESTAMPTZ '2012-01-01 00:00:00+01'", Primitive::DateTime),
        (
            "CAST('2012-01-01T00:00:00.000+00:00' AS TIMESTAMP WITH TIME ZONE)",
            Primitive::DateTime,
        ),
        ("DATE '2012-01-01'", Primitive::Date),
        ("TIME '12:00:00'", Primitive::Time),
        ("CAST(1 AS BIGINT)", Primitive::Integer),
        ("CAST(1.5 AS DOUBLE)", Primitive::Float),
        ("CAST(1.5 AS DECIMAL(38,18))", Primitive::Float),
        ("true", Primitive::Bool),
        ("'x'", Primitive::String),
    ] {
        let described = rows(&conn, &format!("DESCRIBE SELECT {sql} AS v")).expect("describe runs");
        let declared = described[0][1].clone().expect("a type");
        assert_eq!(
            fossil_profile::generated::duckdb_primitive(&declared),
            expected,
            "DuckDB types `{sql}` as `{declared}`, which must not widen to String"
        );
    }
}

/// A types profile reads the same file the run reads: the delimiter the
/// binding wrote, so a pipe-delimited file is three columns and not one called
/// `id|name|city`.
#[test]
fn a_pipe_delimited_source_is_described_with_the_delimiter_it_was_written_with() {
    let dir = tempfile::tempdir().expect("a tempdir");
    std::fs::write(
        dir.path().join("people.csv"),
        "id|name|city\n1|Ada|Kent\n2|Bo|Arles\n",
    )
    .expect("people");
    let piped = Source {
        option: Some("|".into()),
        ..source(dir.path(), "people.csv", "csv")
    };
    let profile = profile([piped], Options::TYPES);
    let names: Vec<&str> = profile.sources[0]
        .descriptor
        .columns
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(names, ["id", "name", "city"]);
}
