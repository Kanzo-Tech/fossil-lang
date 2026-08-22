//! `catalogue.bnf` → the provider statics.
//!
//! # Why this exists
//!
//! `catalogue.bnf` says WHAT NAMES EXIST, and it has said so since ruling 14 of
//! `SURFACE-PLAN.md` — the name written after `io.`, the extensions the row
//! accepts, and which capabilities it declares. What the tree did with that file
//! until now was *check* the Rust against it: a parity test parsed the rows and
//! failed when the hand-written statics drifted. The file's own `# Status`
//! section named the step after, in as many words:
//!
//! > Generating those statics from this file — so the Rust cannot drift at all
//! > rather than being caught drifting — is the step after.
//!
//! This is that step. The reference is rust-analyzer's `rust.ungram`, which
//! `catalogue.bnf` already cites: a data file beside the compiler, a generator,
//! and a checked-in output that a `--check` mode proves current.
//!
//! # What generation closes that the parity test could not
//!
//! The parity test said what it could not prove, and it was this: *«that
//! `decodes decode_shex` names the function actually installed. A `fn` pointer
//! has no name at run time.»* A generated file writes `decode_shex` as a Rust
//! PATH, so the compiler resolves it. A row that names a decoder nobody wrote is
//! now a build failure rather than a row whose capability is declared and whose
//! behaviour is somebody else's.
//!
//! It also collapses two hand-written derivations of one datum. `NativeReader`'s
//! variants and the `DuckDB` table function each names were two spellings of the
//! `native <fn>` token — the enum in `fossil-base`, and a `native_fn` in the
//! parity test that mapped it back to the string the file had written in the
//! first place. Both come off the token now: the variant name is derived from
//! it and `NativeReader::table_function` gives it back.
//!
//! # Where a row goes, and why that is derivable
//!
//! `catalogue.bnf` draws the DATA/BEHAVIOUR line itself: a row's data lives in
//! the file, and the function that turns bytes into rows or into types is
//! supplied by the crate that can link it. So a row that `decodes` is emitted
//! into `fossil-descriptors-output`, which links the shape-language parsers, and
//! every other row into `fossil-base`, which the compiler always has. That is
//! also why `DATA` — the default of `System::providers` — is exactly the
//! `fossil-base` half: a host that links no schema language still gets every row
//! it can execute.
//!
//! # What this does NOT generate, deliberately
//!
//! The ARGUMENT. `catalogue.bnf` keeps it in the `(* … *)` commentary, and the
//! doc comments emitted here are the one-line derived kind — what the row is,
//! not why it is that. Prose that reasons belongs next to the reasoning, and a
//! generator that tried to carry it would make the `.bnf` a second Rust file
//! with different syntax.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// How a row's bytes become rows a plan can scan.
#[derive(Debug, PartialEq, Eq)]
pub enum Reads {
    /// `reads native <fn>` — a `DuckDB` table function, named by the token.
    Native(String),
    /// `reads materialised` — something outside the reader produces the
    /// relation and the core only scans it.
    Materialised,
}

/// One `row … = … .` line of `catalogue.bnf`.
#[derive(Debug)]
pub struct Row {
    /// What is written after `io.`.
    pub name: String,
    /// The extensions the row accepts, in file order.
    pub extensions: Vec<String>,
    /// The `read rows` capability.
    pub reads: Option<Reads>,
    /// The `read types` capability — the name of the decoding function the
    /// implementing crate supplies.
    pub decodes: Option<String>,
}

impl Row {
    /// The Rust identifier of this row's static: the name, upper-cased.
    fn static_ident(&self) -> String {
        self.name.to_ascii_uppercase()
    }

    /// Does this row's behaviour have to be linked by a crate that owns a
    /// shape-language parser? See the module doc.
    fn needs_a_decoder(&self) -> bool {
        self.decodes.is_some()
    }
}

/// The `NativeReader` variant a `native <fn>` token names.
///
/// `read_csv_auto` → `CsvAuto`, `read_parquet` → `Parquet`: strip the `read_`
/// that every table function starts with, then upper-camel the rest. The rule is
/// here rather than in a table so that a new reader is a row in the file and
/// nothing else — which is the whole point of generating this.
fn variant_of(table_fn: &str) -> String {
    table_fn
        .strip_prefix("read_")
        .unwrap_or(table_fn)
        .split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// Parse every `row` line of `catalogue.bnf`.
///
/// Everything outside a `row` line is `(* … *)` commentary, which is where the
/// argument lives and which this ignores. A malformed row is a panic and not a
/// skip: this is the source of truth now, so a line that does not parse is a
/// build failure rather than a name that quietly stops existing.
///
/// # Panics
///
/// On any `row` line that is not `row <name> = extensions "…"+ ; <clause> .`,
/// and on a file that declares no rows at all.
#[must_use]
pub fn parse(text: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut in_comment = false;
    for line in text.lines() {
        if line.contains("(*") {
            in_comment = !line.contains("*)");
            continue;
        }
        if in_comment {
            in_comment = !line.contains("*)");
            continue;
        }
        let Some(rest) = line.trim().strip_prefix("row ") else {
            continue;
        };
        rows.push(parse_row(rest, line));
    }
    assert!(!rows.is_empty(), "catalogue.bnf declares no rows at all");
    rows
}

/// One row line, with `line` carried only so a panic can quote it.
fn parse_row(rest: &str, line: &str) -> Row {
    let (name, body) = rest
        .split_once('=')
        .unwrap_or_else(|| panic!("a `row` line needs an `=`: {line}"));
    let name = name.trim().to_owned();

    // Extensions are the quoted tokens; the clauses are what is left.
    let extensions: Vec<String> = body
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect();
    assert!(!extensions.is_empty(), "row `{name}` declares no extension");

    let words: Vec<&str> = body
        .split('"')
        .step_by(2)
        .flat_map(str::split_whitespace)
        .collect();

    let mut reads = None;
    let mut decodes = None;
    let mut i = 0;
    while i < words.len() {
        match words[i] {
            "reads" => {
                let what = words.get(i + 1).unwrap_or_else(|| {
                    panic!("`reads` needs `native <fn>` or `materialised`: {line}")
                });
                match *what {
                    "native" => {
                        let f = words
                            .get(i + 2)
                            .unwrap_or_else(|| panic!("`reads native` needs a function: {line}"));
                        reads = Some(Reads::Native((*f).to_owned()));
                        i += 3;
                    }
                    "materialised" => {
                        reads = Some(Reads::Materialised);
                        i += 2;
                    }
                    other => panic!("unknown `reads` form `{other}`: {line}"),
                }
            }
            "decodes" => {
                let f = words
                    .get(i + 1)
                    .unwrap_or_else(|| panic!("`decodes` needs a function: {line}"));
                decodes = Some((*f).to_owned());
                i += 2;
            }
            _ => i += 1,
        }
    }

    assert!(
        reads.is_some() || decodes.is_some(),
        "row `{name}` declares no capability at all: {line}"
    );
    Row {
        name,
        extensions,
        reads,
        decodes,
    }
}

/// The header every generated file carries.
fn header(module_doc: &str) -> String {
    format!(
        "//! @generated by `cargo xtask catalogue` from `catalogue.bnf`. DO NOT EDIT.\n\
         //!\n\
         {module_doc}\n\
         //!\n\
         //! Edit `catalogue.bnf` and re-run `cargo xtask catalogue`.\n\n"
    )
}

/// A `&[\"a\", \"b\"]` literal.
fn extensions_literal(extensions: &[String]) -> String {
    let inner: Vec<String> = extensions.iter().map(|e| format!("{e:?}")).collect();
    format!("&[{}]", inner.join(", "))
}

/// Emit one `pub static NAME: Provider = …`.
fn emit_row(out: &mut String, row: &Row) {
    let reads_rows = match &row.reads {
        Some(Reads::Native(f)) => {
            format!("Some(RowReader::Native(NativeReader::{}))", variant_of(f))
        }
        Some(Reads::Materialised) => "Some(RowReader::Materialised)".to_owned(),
        None => "None".to_owned(),
    };
    let reads_types = match &row.decodes {
        Some(f) => format!("Some({f})"),
        None => "None".to_owned(),
    };
    let what = match (&row.reads, &row.decodes) {
        (Some(Reads::Native(f)), _) => format!("reads rows via `{f}`"),
        (Some(Reads::Materialised), _) => "reads rows, materialised outside the reader".to_owned(),
        (None, Some(f)) => format!("reads types via `{f}`"),
        (None, None) => unreachable!("parse rejects a row with no capability"),
    };

    let _ = writeln!(out, "/// `io.{}` — {what}.", row.name);
    let _ = writeln!(
        out,
        "pub static {}: Provider = Provider {{",
        row.static_ident()
    );
    let _ = writeln!(out, "    name: {:?},", row.name);
    let _ = writeln!(
        out,
        "    extensions: {},",
        extensions_literal(&row.extensions)
    );
    let _ = writeln!(out, "    reads_rows: {reads_rows},");
    let _ = writeln!(out, "    reads_types: {reads_types},");
    let _ = writeln!(out, "}};\n");
}

/// The `fossil-base` half: `NativeReader`, the rows the compiler can always
/// name, and `DATA`.
#[must_use]
pub fn emit_base(rows: &[Row]) -> String {
    let data: Vec<&Row> = rows.iter().filter(|r| !r.needs_a_decoder()).collect();

    let mut out = header(
        "//! The rows whose behaviour the compiler can always link: every row that\n\
         //! reads DATA. A row that decodes a shape language is emitted into\n\
         //! `fossil-descriptors-output` instead — see `xtask::catalogue`.",
    );
    out.push_str("use super::{Provider, RowReader};\n\n");

    // The reader enum, one variant per DISTINCT table function, in file order.
    let mut readers: Vec<&String> = Vec::new();
    for row in &data {
        if let Some(Reads::Native(f)) = &row.reads
            && !readers.contains(&f)
        {
            readers.push(f);
        }
    }

    out.push_str(
        "/// The native `DuckDB` readers a [`RowReader::Native`] maps to.\n\
         ///\n\
         /// `fossil-mir` exhaustively maps each to a `SourceFormat`, so a new reader is\n\
         /// a compile error until handled — which is the property generating this enum\n\
         /// from `catalogue.bnf` is meant to keep, not trade away.\n\
         #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]\n\
         pub enum NativeReader {\n",
    );
    for f in &readers {
        let _ = writeln!(out, "    /// `{f}`.");
        let _ = writeln!(out, "    {},", variant_of(f));
    }
    out.push_str("}\n\n");

    out.push_str(
        "impl NativeReader {\n\
         \x20   /// The `DuckDB` table function this reader names — the token\n\
         \x20   /// `catalogue.bnf` writes after `native`.\n\
         \x20   #[must_use]\n\
         \x20   pub const fn table_function(self) -> &'static str {\n\
         \x20       match self {\n",
    );
    for f in &readers {
        let _ = writeln!(out, "            Self::{} => {f:?},", variant_of(f));
    }
    out.push_str("        }\n    }\n}\n\n");

    for row in &data {
        emit_row(&mut out, row);
    }

    out.push_str(
        "/// The rows the compiler can name on its own: everything that reads DATA.\n\
         ///\n\
         /// This is [`crate::system::System::providers`]'s default, and it is a real\n\
         /// answer rather than a stub — a host that decodes no shape document still has\n\
         /// to recognise `io.csv`. A host that compiles programs installs a superset\n\
         /// (`fossil_descriptors_output::PROVIDERS`), and the rows it adds are exactly\n\
         /// the ones `catalogue.bnf` gives a `decodes`.\n",
    );
    let refs: Vec<String> = data
        .iter()
        .map(|r| format!("&{}", r.static_ident()))
        .collect();
    let _ = writeln!(
        out,
        "pub static DATA: &[&Provider] = &[{}];",
        refs.join(", ")
    );
    out
}

/// The `fossil-descriptors-output` half: the rows that decode a shape language,
/// and the assembled table.
#[must_use]
pub fn emit_descriptors(rows: &[Row]) -> String {
    let typed: Vec<&Row> = rows.iter().filter(|r| r.needs_a_decoder()).collect();

    let mut out = header(
        "//! The rows that read TYPES, and [`PROVIDERS`] — every row in\n\
         //! `catalogue.bnf`, which is what a COMPILING host installs.\n\
         //!\n\
         //! Each `decodes` token below is a Rust path, so the compiler resolves it.\n\
         //! That is what the parity test this replaces said it could not prove: a `fn`\n\
         //! pointer has no name at run time, and a row could declare a capability whose\n\
         //! function nobody had written.",
    );
    out.push_str("use fossil_base::Provider;\n");
    out.push_str("use fossil_base::providers::{");
    let data_idents: Vec<String> = rows
        .iter()
        .filter(|r| !r.needs_a_decoder())
        .map(Row::static_ident)
        .collect();
    out.push_str(&data_idents.join(", "));
    out.push_str("};\n\n");

    let decoders: Vec<String> = typed
        .iter()
        .filter_map(|r| r.decodes.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let _ = writeln!(out, "use super::{{{}}};\n", decoders.join(", "));

    for row in &typed {
        emit_row(&mut out, row);
    }

    out.push_str(
        "/// **The registry a compiling host installs**: every row `catalogue.bnf`\n\
         /// declares, in file order.\n\
         ///\n\
         /// A host that returns this from `System::providers` recognises every `io.*`\n\
         /// the language has. The default (`fossil_base::providers::DATA`) is the data\n\
         /// rows alone, which is correct for a host that runs a plan somebody else\n\
         /// compiled and wrong for the host the plan comes from.\n",
    );
    let refs: Vec<String> = rows
        .iter()
        .map(|r| format!("&{}", r.static_ident()))
        .collect();
    let _ = writeln!(
        out,
        "pub static PROVIDERS: &[&Provider] = &[{}];",
        refs.join(", ")
    );
    out
}

/// Run the source text through `rustfmt`, so the generated files are formatted
/// the way `cargo fmt --check` demands rather than the way this emitter happens
/// to write them.
///
/// # Panics
///
/// If `rustfmt` is missing or rejects the emitted source — the second is a bug
/// in this emitter and should fail loudly rather than commit unformatted Rust.
#[must_use]
pub fn rustfmt(source: &str) -> String {
    use std::io::Write as _;

    let mut child = Command::new("rustfmt")
        .args(["--edition", "2024", "--emit", "stdout", "--quiet"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn rustfmt");
    child
        .stdin
        .take()
        .expect("rustfmt stdin")
        .write_all(source.as_bytes())
        .expect("write to rustfmt");
    let out = child.wait_with_output().expect("wait for rustfmt");
    assert!(
        out.status.success(),
        "rustfmt rejected the generated source"
    );
    String::from_utf8(out.stdout).expect("rustfmt emits utf-8")
}

/// The repository root — the directory holding `catalogue.bnf`.
#[must_use]
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("xtask lives two directories below the repo root")
        .to_path_buf()
}

/// Every generated file: its path, and what it should contain.
///
/// # Panics
///
/// If `catalogue.bnf` cannot be read or does not parse.
#[must_use]
pub fn generated() -> Vec<(PathBuf, String)> {
    let root = repo_root();
    let text = std::fs::read_to_string(root.join("catalogue.bnf")).expect("read catalogue.bnf");
    let rows = parse(&text);
    vec![
        (
            root.join("crates/fossil-base/src/providers/generated.rs"),
            rustfmt(&emit_base(&rows)),
        ),
        (
            root.join("crates/fossil-descriptors-output/src/generated.rs"),
            rustfmt(&emit_descriptors(&rows)),
        ),
    ]
}
