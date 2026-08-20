//! F7 · `DuckDB COPY` against `arrow-rs`, in footers, requests, bytes and pages.
//!
//! The plan blocks F7 on a measurement that was never taken: write 5M rows both
//! ways and count what a camera window costs. This is that harness. It is a
//! spike — it writes nothing into `crates/`, is not a workspace member, and
//! keeps its own `target/`.
//!
//! # The two ways
//!
//! * **A — today.** `DuckDB COPY (… ORDER BY dense_id) TO 'chunk{k}.parquet'
//!   (FORMAT PARQUET)`, one file per 4,096-row tile, the statement
//!   `fossil-runtime/src/layout.rs:570` emits verbatim (no options: DuckDB's own
//!   defaults, which are snappy and 122,880-row row groups — the latter is moot
//!   at 4,096 rows a file). Two more DuckDB shapes are written so the writer can
//!   be told apart from the layout: one file with DuckDB's default row groups,
//!   and one file with `ROW_GROUP_SIZE 4096`.
//! * **B — proposed.** `arrow-rs` `ArrowWriter`, one file, 4,096-row row groups,
//!   everything else left at the crate default — which is where the page index
//!   comes from, and it is the point.
//!
//! The rows are the same rows in the same order in every case: the corpus is
//! written once by `arrow-rs`, and DuckDB re-encodes *that file*.
//!
//! # Running it
//!
//!     ./run.sh                       # 5M rows, drawing schema, into the scratch dir
//!     cargo run --release -- --rows 5000000 --dir <dir> --schema draw|full
//!
//! Needs the `duckdb` CLI on `PATH`; without it the DuckDB rows are absent and
//! every arrow-rs row is still produced.

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{ArrayRef, Float32Array, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::basic::Compression;
use parquet::file::metadata::{PageIndexPolicy, ParquetMetaData, ParquetMetaDataReader};
use parquet::file::properties::WriterProperties;
use parquet::file::reader::{FileReader, SerializedFileReader};
use parquet::file::statistics::Statistics;

/// `fossil_sinks::manifest::DEFAULT_CHUNK_SIZE`, restated (the spike must not
/// depend on the crate that owns the manifest).
const TILE_ROWS: usize = 4_096;
/// Nine windows, so the overread band is comparable to the 2026-08-06 figure
/// the plan carries (1.05×–1.21×).
const WINDOWS: usize = 9;
/// Vertices a window is sized to contain — the same 20k the tile-size and
/// edge-placement measurements used.
const WINDOW_VERTICES: usize = 20_000;

// ── the corpus ────────────────────────────────────────────────────────────
// `cluster_layout` + `morton2` are `fossil_runtime::layout`'s, copied because
// that crate owns DuckDB and cannot be depended on from a spike. If those
// constants move, this measures a corpus the writer no longer produces.

const GOLDEN_ANGLE: f32 = 2.399_963_2;
const CLUSTER_SPACING: f32 = 100.0;
const INTRA_CLUSTER_RADIUS: f32 = 12.0;

struct Corpus {
    x: Vec<f32>,
    y: Vec<f32>,
    cluster: Vec<u32>,
    orig: Vec<u32>,
    full: bool,
}

fn morton_decode(code: u32) -> (u32, u32) {
    fn compact(mut n: u32) -> u32 {
        n &= 0x5555_5555;
        n = (n | (n >> 1)) & 0x3333_3333;
        n = (n | (n >> 2)) & 0x0f0f_0f0f;
        n = (n | (n >> 4)) & 0x00ff_00ff;
        n = (n | (n >> 8)) & 0x0000_ffff;
        n
    }
    (compact(code), compact(code >> 1))
}

fn morton2(x: u16, y: u16) -> u32 {
    fn spread(n: u16) -> u32 {
        let mut n = u32::from(n);
        n = (n | (n << 8)) & 0x00ff_00ff;
        n = (n | (n << 4)) & 0x0f0f_0f0f;
        n = (n | (n << 2)) & 0x3333_3333;
        n = (n | (n << 1)) & 0x5555_5555;
        n
    }
    spread(x) | (spread(y) << 1)
}

fn cluster_layout(cluster_ids: &[u32]) -> Vec<(f32, f32)> {
    let num_clusters = cluster_ids.iter().copied().max().map_or(0, |m| m + 1);
    if num_clusters == 0 {
        return Vec::new();
    }
    let mut sizes = vec![0u32; num_clusters as usize];
    for &c in cluster_ids {
        sizes[c as usize] += 1;
    }
    let largest = sizes.iter().copied().max().unwrap_or(0);
    let pitch = 2.0f32.mul_add(
        INTRA_CLUSTER_RADIUS * (largest as f32).sqrt(),
        CLUSTER_SPACING,
    );
    let mut seen = vec![0u32; num_clusters as usize];
    let mut out = Vec::with_capacity(cluster_ids.len());
    for &c in cluster_ids {
        let (col, row) = morton_decode(c);
        let k = seen[c as usize];
        seen[c as usize] += 1;
        let angle = k as f32 * GOLDEN_ANGLE;
        let radius = INTRA_CLUSTER_RADIUS * ((k as f32) + 1.0).sqrt();
        out.push((
            radius.mul_add(angle.cos(), col as f32 * pitch),
            radius.mul_add(angle.sin(), row as f32 * pitch),
        ));
    }
    out
}

fn build_corpus(n: usize, k: u32, full: bool) -> Corpus {
    let cluster_ids: Vec<u32> = (0..n)
        .map(|i| (i as u64 * k as u64 / n as u64) as u32)
        .collect();
    let positions = cluster_layout(&cluster_ids);

    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for &(x, y) in &positions {
        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x);
        max_y = max_y.max(y);
    }
    let quantize = |v: f32, lo: f32, hi: f32| -> u16 {
        if hi <= lo {
            return 0;
        }
        let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
        (t * f32::from(u16::MAX)).round() as u16
    };
    let codes: Vec<u32> = positions
        .iter()
        .map(|&(x, y)| morton2(quantize(x, min_x, max_x), quantize(y, min_y, max_y)))
        .collect();

    let mut order: Vec<u32> = (0..n as u32).collect();
    order.sort_unstable_by_key(|&i| (codes[i as usize], i));

    let mut x = Vec::with_capacity(n);
    let mut y = Vec::with_capacity(n);
    let mut cluster = Vec::with_capacity(n);
    for &old in &order {
        let (px, py) = positions[old as usize];
        x.push(px);
        y.push(py);
        cluster.push(cluster_ids[old as usize]);
    }
    Corpus {
        x,
        y,
        cluster,
        orig: order,
        full,
    }
}

fn schema(full: bool) -> Arc<Schema> {
    let mut fields = vec![
        Field::new("dense_id", DataType::UInt32, false),
        Field::new("x", DataType::Float32, false),
        Field::new("y", DataType::Float32, false),
        Field::new("cluster_id", DataType::UInt32, false),
    ];
    if full {
        fields.push(Field::new("subject", DataType::Utf8, false));
        fields.push(Field::new("community", DataType::UInt32, false));
    }
    Arc::new(Schema::new(fields))
}

fn batch(c: &Corpus, lo: usize, hi: usize) -> RecordBatch {
    let mut cols: Vec<ArrayRef> = vec![
        Arc::new(UInt32Array::from_iter_values(lo as u32..hi as u32)),
        Arc::new(Float32Array::from_iter_values(c.x[lo..hi].iter().copied())),
        Arc::new(Float32Array::from_iter_values(c.y[lo..hi].iter().copied())),
        Arc::new(UInt32Array::from_iter_values(
            c.cluster[lo..hi].iter().copied(),
        )),
    ];
    if c.full {
        cols.push(Arc::new(StringArray::from_iter_values(
            c.orig[lo..hi]
                .iter()
                .map(|o| format!("https://example.org/v/{o}")),
        )));
        cols.push(Arc::new(UInt32Array::from_iter_values(
            c.cluster[lo..hi].iter().map(|c| c % 8),
        )));
    }
    RecordBatch::try_new(schema(c.full), cols).expect("corpus columns are the schema")
}

// ── the arrow-rs writers ──────────────────────────────────────────────────

/// One file. `row_group` of `None` leaves the crate default (1,048,576 rows),
/// which is the only shape in which a data page is smaller than a row group and
/// therefore the only one where a page index has anything to skip. Everything
/// not named here is the crate default, which is what puts the page index in the
/// file at all.
fn write_single(
    path: &Path,
    c: &Corpus,
    n: usize,
    row_group: Option<usize>,
    compression: Compression,
    dictionary: bool,
) {
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(row_group)
        .set_compression(compression)
        .set_dictionary_enabled(dictionary)
        .build();
    let file = fs::File::create(path).expect("create");
    let mut w = parquet::arrow::ArrowWriter::try_new(file, schema(c.full), Some(props))
        .expect("arrow writer");
    // Fed a tile at a time so the row-group boundaries are exact rather than a
    // consequence of batch size; with `row_group: None` the writer coalesces
    // them itself and the boundaries are its own.
    for t in 0..n.div_ceil(TILE_ROWS) {
        let lo = t * TILE_ROWS;
        let hi = ((t + 1) * TILE_ROWS).min(n);
        w.write(&batch(c, lo, hi)).expect("write row group");
    }
    w.close().expect("close");
}

/// One file per tile — `fossil_df::files::batches_to_parquet`'s shape, which is
/// the arrow-rs encoder that already ships, at tile granularity.
fn write_per_tile(dir: &Path, c: &Corpus, n: usize, compression: Compression) -> Vec<PathBuf> {
    fs::create_dir_all(dir).expect("mkdir");
    let props = WriterProperties::builder()
        .set_max_row_group_row_count(Some(TILE_ROWS))
        .set_compression(compression)
        .build();
    let mut paths = Vec::new();
    for t in 0..n.div_ceil(TILE_ROWS) {
        let lo = t * TILE_ROWS;
        let hi = ((t + 1) * TILE_ROWS).min(n);
        let path = dir.join(format!("chunk{t}.parquet"));
        let file = fs::File::create(&path).expect("create tile");
        let mut w =
            parquet::arrow::ArrowWriter::try_new(file, schema(c.full), Some(props.clone()))
                .expect("arrow writer");
        w.write(&batch(c, lo, hi)).expect("write tile");
        w.close().expect("close tile");
        paths.push(path);
    }
    paths
}

// ── the DuckDB writer, through its CLI ────────────────────────────────────

/// Runs a SQL script through the `duckdb` CLI. Returns false if the CLI is
/// missing or the script failed — the report then says the rows are absent
/// rather than inventing them.
fn duckdb(sql: &str, script_path: &Path) -> bool {
    fs::write(script_path, sql).expect("write sql");
    match Command::new("duckdb")
        .arg(":memory:")
        .arg("-c")
        .arg(format!(".read {}", script_path.display()))
        .output()
    {
        Ok(out) if out.status.success() => true,
        Ok(out) => {
            eprintln!(
                "duckdb failed: {}\n{}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            );
            false
        }
        Err(e) => {
            eprintln!("duckdb not runnable: {e}");
            false
        }
    }
}

fn duckdb_version() -> String {
    Command::new("duckdb")
        .arg("--version")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "absent".to_string())
}

// ── reading the bytes back ────────────────────────────────────────────────

#[derive(Clone)]
struct PageInfo {
    /// Which column chunk it belongs to — two pages of different columns are
    /// never one byte range, however adjacent their rows.
    col: usize,
    /// First row of this page, counted from the start of the *file*.
    row_start: usize,
    rows: usize,
    /// Compressed page size, off the `OffsetIndex` — exact, and only available
    /// where the writer wrote that index. `PageReader` hands back *decompressed*
    /// bodies, which is why they are not the source here.
    bytes: u64,
}

struct RowGroupFacts {
    row_start: usize,
    rows: usize,
    bytes: u64,
    /// `(xmin, xmax, ymin, ymax)` off the column statistics — the box a reader
    /// with the footer in hand prunes with.
    bbox: (f32, f32, f32, f32),
    /// `src_dense` / `dst_dense` min-max, off the same statistics. An edge tile
    /// is not addressed by a box: it is addressed by a `dense_id` RANGE, which is
    /// what a reader intersects a selected vertex tile against.
    src_range: (i64, i64),
    dst_range: (i64, i64),
}

struct FileFacts {
    file_bytes: u64,
    /// Thrift `FileMetaData` + the 4-byte length + `PAR1`.
    footer_bytes: u64,
    /// `ColumnIndex` + `OffsetIndex`, which live outside the footer blob.
    page_index_bytes: u64,
    page_index_present: bool,
    /// Whether page byte sizes could be observed at all — they come off the
    /// `OffsetIndex`, so a file without one has none.
    page_bytes_exact: bool,
    row_groups: Vec<RowGroupFacts>,
    /// Data pages (dictionary pages counted separately), across all columns.
    data_pages: usize,
    dict_pages: usize,
    /// Data pages per row group, all columns flattened.
    pages: Vec<Vec<PageInfo>>,
    /// Per row group, per column: the dictionary page's on-disk bytes.
    dict_bytes: Vec<Vec<u64>>,
    columns: BTreeMap<String, u64>,
}

fn read_tail(path: &Path, n: usize) -> Vec<u8> {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let mut f = fs::File::open(path).expect("open tail");
    let len = f.metadata().expect("stat").len();
    f.seek(SeekFrom::Start(len - n as u64)).expect("seek");
    let mut buf = vec![0u8; n];
    f.read_exact(&mut buf).expect("read tail");
    buf
}

/// `with_pages`: enumerating pages means reading every page header in the file,
/// which is cheap for one file and 1,221 file-opens for the per-tile layouts —
/// so it is a parameter and the report says where it was skipped.
fn read_facts(path: &Path, row_offset: usize, with_pages: bool) -> FileFacts {
    let file = fs::File::open(path).expect("open");
    let file_bytes = file.metadata().expect("stat").len();
    let meta: ParquetMetaData = ParquetMetaDataReader::new()
        .with_page_index_policy(PageIndexPolicy::Optional)
        .parse_and_finish(&file)
        .expect("parse metadata");

    let tail = read_tail(path, 8);
    let meta_len = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]) as u64;
    let footer_bytes = meta_len + 8;

    let mut page_index_bytes = 0u64;
    let mut columns: BTreeMap<String, u64> = BTreeMap::new();
    let mut row_groups = Vec::with_capacity(meta.num_row_groups());
    let mut row_cursor = row_offset;
    for rg in meta.row_groups() {
        let (mut xmin, mut xmax, mut ymin, mut ymax) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        let mut src_range = (i64::MAX, i64::MIN);
        let mut dst_range = (i64::MAX, i64::MIN);
        for col in rg.columns() {
            page_index_bytes += col.column_index_length().unwrap_or(0).max(0) as u64;
            page_index_bytes += col.offset_index_length().unwrap_or(0).max(0) as u64;
            let name = col.column_descr().name().to_string();
            *columns.entry(name.clone()).or_default() += col.compressed_size().max(0) as u64;
            // `src_dense`/`dst_dense` are UInt32 in arrow, INT32 in Parquet.
            let int_range = match col.statistics() {
                Some(Statistics::Int32(v)) => v
                    .min_opt()
                    .copied()
                    .zip(v.max_opt().copied())
                    .map(|(a, b)| (i64::from(a), i64::from(b))),
                Some(Statistics::Int64(v)) => v.min_opt().copied().zip(v.max_opt().copied()),
                _ => None,
            };
            if let Some(r) = int_range {
                if name == "src_dense" {
                    src_range = r;
                } else if name == "dst_dense" {
                    dst_range = r;
                }
            }
            if let Some(Statistics::Float(v)) = col.statistics() {
                if let (Some(lo), Some(hi)) = (v.min_opt().copied(), v.max_opt().copied()) {
                    if name == "x" {
                        xmin = lo;
                        xmax = hi;
                    } else if name == "y" {
                        ymin = lo;
                        ymax = hi;
                    }
                }
            }
        }
        let rows = rg.num_rows() as usize;
        row_groups.push(RowGroupFacts {
            row_start: row_cursor,
            rows,
            bytes: rg.compressed_size().max(0) as u64,
            bbox: (xmin, xmax, ymin, ymax),
            src_range,
            dst_range,
        });
        row_cursor += rows;
    }

    // Page bytes come from the `OffsetIndex` — `PageLocation::compressed_page_size`
    // is the on-disk size INCLUDING the page header, which is the same unit as
    // every other byte in this report. `PageReader` is only used to COUNT pages,
    // because the buffer it hands back is decompressed and comparing that with
    // footer bytes would be comparing two different things. A file with no page
    // index therefore has page counts but no page byte figure, which is exactly
    // the reader's situation too.
    let (mut data_pages, mut dict_pages) = (0usize, 0usize);
    let mut pages: Vec<Vec<PageInfo>> = vec![Vec::new(); meta.num_row_groups()];
    let mut dict_bytes: Vec<Vec<u64>> = vec![Vec::new(); meta.num_row_groups()];
    let offsets = meta.offset_index();
    if with_pages {
        let reader = SerializedFileReader::new(fs::File::open(path).expect("open pages"))
            .expect("serialized reader");
        for (r, rgf) in row_groups.iter().enumerate() {
            let rg = reader.get_row_group(r).expect("row group");
            let cols = rg.metadata().columns();
            dict_bytes[r] = cols
                .iter()
                .map(|col| match (col.dictionary_page_offset(), col.data_page_offset()) {
                    // The dictionary page sits immediately before the first data
                    // page, so their offsets bracket it. A reader that fetches one
                    // dictionary-encoded page must fetch this too.
                    (Some(d), p) if p > d => (p - d) as u64,
                    _ => 0,
                })
                .collect();
            for c in 0..cols.len() {
                let mut pr = rg.get_column_page_reader(c).expect("page reader");
                let mut row = rgf.row_start;
                let mut seen = 0usize;
                while let Some(page) = pr.get_next_page().expect("page") {
                    let n = page.num_values() as usize;
                    if matches!(page.page_type(), parquet::basic::PageType::DICTIONARY_PAGE) {
                        dict_pages += 1;
                        continue;
                    }
                    data_pages += 1;
                    let bytes = offsets
                        .and_then(|o| o.get(r))
                        .and_then(|rgi| rgi.get(c))
                        .and_then(|oi| oi.page_locations.get(seen))
                        .map_or(0, |loc| loc.compressed_page_size.max(0) as u64);
                    pages[r].push(PageInfo {
                        col: c,
                        row_start: row,
                        rows: n,
                        bytes,
                    });
                    row += n;
                    seen += 1;
                }
            }
        }
    }

    FileFacts {
        file_bytes,
        footer_bytes,
        page_index_bytes,
        page_index_present: meta.column_index().is_some() && page_index_bytes > 0,
        page_bytes_exact: offsets.is_some(),
        row_groups,
        data_pages,
        dict_pages,
        pages,
        dict_bytes,
        columns,
    }
}

// ── a layout is a set of files plus a rule for what one request fetches ───

struct Unit {
    bbox: (f32, f32, f32, f32),
    src_range: (i64, i64),
    row_start: usize,
    rows: usize,
    /// What one request for this unit costs: the row group's bytes inside one
    /// file, or the whole file when a tile is a file.
    bytes: u64,
    pages: Vec<PageInfo>,
    /// Per column: the dictionary page a reader must also fetch when it wants
    /// any dictionary-encoded page of that column chunk.
    dict_bytes: Vec<u64>,
}

struct Layout {
    name: &'static str,
    how: String,
    files: usize,
    file_bytes: u64,
    footer_bytes: u64,
    page_index_bytes: u64,
    page_index_present: bool,
    page_bytes_exact: bool,
    data_pages: usize,
    dict_pages: usize,
    pages_measured: bool,
    columns: BTreeMap<String, u64>,
    /// A file boundary is a request boundary: consecutive tiles in separate
    /// files cannot be coalesced into one range request.
    per_file: bool,
    units: Vec<Unit>,
}

impl Layout {
    fn single(name: &'static str, how: String, f: FileFacts) -> Self {
        let units = f
            .row_groups
            .iter()
            .enumerate()
            .map(|(i, rg)| Unit {
                bbox: rg.bbox,
                src_range: rg.src_range,
                row_start: rg.row_start,
                rows: rg.rows,
                bytes: rg.bytes,
                pages: f.pages.get(i).cloned().unwrap_or_default(),
                dict_bytes: f.dict_bytes.get(i).cloned().unwrap_or_default(),
            })
            .collect();
        Self {
            name,
            how,
            files: 1,
            file_bytes: f.file_bytes,
            footer_bytes: f.footer_bytes,
            page_index_bytes: f.page_index_bytes,
            page_index_present: f.page_index_present,
            page_bytes_exact: f.page_bytes_exact,
            data_pages: f.data_pages,
            dict_pages: f.dict_pages,
            pages_measured: true,
            columns: f.columns,
            per_file: false,
            units,
        }
    }

    fn per_tile(name: &'static str, how: String, files: Vec<FileFacts>, pages_measured: bool) -> Self {
        let mut units = Vec::with_capacity(files.len());
        let mut columns: BTreeMap<String, u64> = BTreeMap::new();
        let (mut file_bytes, mut footer, mut pi, mut dp, mut kp) = (0u64, 0u64, 0u64, 0usize, 0usize);
        let mut present = true;
        let mut exact = true;
        for f in &files {
            // A tile that is a file is ONE request whatever its internal row
            // groups, so the file collapses to one unit. A vertex tile is always
            // one row group (4,096 rows); an edge tile is however many rows the
            // degree of those 4,096 sources came to, and can be more.
            let rg = &f.row_groups[0];
            let last = f.row_groups.last().expect("a file has a row group");
            units.push(Unit {
                bbox: f.row_groups.iter().fold(rg.bbox, |b, g| {
                    (b.0.min(g.bbox.0), b.1.max(g.bbox.1), b.2.min(g.bbox.2), b.3.max(g.bbox.3))
                }),
                src_range: f.row_groups.iter().fold(rg.src_range, |r, g| {
                    (r.0.min(g.src_range.0), r.1.max(g.src_range.1))
                }),
                row_start: rg.row_start,
                rows: last.row_start + last.rows - rg.row_start,
                bytes: f.file_bytes,
                pages: f.pages.iter().flatten().cloned().collect(),
                dict_bytes: f.dict_bytes.first().cloned().unwrap_or_default(),
            });
            file_bytes += f.file_bytes;
            footer += f.footer_bytes;
            pi += f.page_index_bytes;
            dp += f.data_pages;
            kp += f.dict_pages;
            present &= f.page_index_present;
            exact &= f.page_bytes_exact;
            for (k, v) in &f.columns {
                *columns.entry(k.clone()).or_default() += v;
            }
        }
        Self {
            name,
            how,
            files: files.len(),
            file_bytes,
            footer_bytes: footer,
            page_index_bytes: pi,
            page_index_present: present,
            page_bytes_exact: exact,
            data_pages: dp,
            dict_pages: kp,
            pages_measured,
            columns,
            per_file: true,
            units,
        }
    }

    fn row_groups(&self) -> usize {
        self.units.len()
    }
}

// ── the windows ───────────────────────────────────────────────────────────

struct WindowCost {
    drawn: usize,
    needed: usize,
    selected: usize,
    requests: usize,
    bytes_selected: u64,
    bytes_needed: u64,
    /// Data pages a reader must fetch for the selected units, whole-unit.
    pages_whole: usize,
    /// Data pages that actually hold a drawn vertex — what a page-index reader
    /// could prune to.
    pages_touched: usize,
    /// Bytes of those touched pages (bodies only), and the range requests they
    /// would take: a run of consecutive touched pages inside one column chunk.
    bytes_pages_touched: u64,
    page_requests: usize,
}

fn hits(c: &Corpus, cx: f32, cy: f32, h: f32) -> Vec<u32> {
    let (x0, x1, y0, y1) = (cx - h, cx + h, cy - h, cy + h);
    (0..c.x.len() as u32)
        .filter(|&i| {
            let (x, y) = (c.x[i as usize], c.y[i as usize]);
            x >= x0 && x <= x1 && y >= y0 && y <= y1
        })
        .collect()
}

fn size_window(c: &Corpus, cx: f32, cy: f32, span: f32) -> f32 {
    let (mut lo, mut hi) = (span * 1e-4, span);
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if hits(c, cx, cy, mid).len() < WINDOW_VERTICES {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    hi
}

/// Any drawn row in `[start, start+rows)`? `h` is sorted.
fn touches(h: &[u32], start: usize, rows: usize) -> bool {
    let lo = h.partition_point(|&r| (r as usize) < start);
    h.get(lo).is_some_and(|&r| (r as usize) < start + rows)
}

fn window_cost(l: &Layout, h: &[u32], cx: f32, cy: f32, half: f32) -> WindowCost {
    let (x0, x1, y0, y1) = (cx - half, cx + half, cy - half, cy + half);
    let mut needed = 0;
    let mut selected = 0;
    let mut runs = 0;
    let mut prev = false;
    let (mut bytes_sel, mut bytes_need) = (0u64, 0u64);
    let (mut pages_whole, mut pages_touched) = (0usize, 0usize);
    let mut bytes_pages_touched = 0u64;
    let mut page_requests = 0usize;
    for u in &l.units {
        let (xmin, xmax, ymin, ymax) = u.bbox;
        let sel = xmin <= x1 && xmax >= x0 && ymin <= y1 && ymax >= y0;
        let need = touches(h, u.row_start, u.rows);
        assert!(!need || sel, "{}: a needed unit is not selected", l.name);
        if need {
            needed += 1;
            bytes_need += u.bytes;
        }
        if sel {
            selected += 1;
            bytes_sel += u.bytes;
            if !prev {
                runs += 1;
            }
            pages_whole += u.pages.len();
            // A page-index reader fetches only the pages that hold a drawn
            // vertex; consecutive ones inside one column chunk coalesce into a
            // single range request.
            let mut prev_end: Option<(usize, usize)> = None;
            let mut dict_wanted: Vec<usize> = Vec::new();
            for p in &u.pages {
                if !touches(h, p.row_start, p.rows) {
                    continue;
                }
                pages_touched += 1;
                bytes_pages_touched += p.bytes;
                if prev_end != Some((p.col, p.row_start)) {
                    page_requests += 1;
                }
                prev_end = Some((p.col, p.row_start + p.rows));
                if !dict_wanted.contains(&p.col) {
                    dict_wanted.push(p.col);
                }
            }
            // A dictionary-encoded data page is undecodable without its
            // dictionary page, so any touched page in a chunk drags that in —
            // one more range request, and the bytes with it.
            for c in dict_wanted {
                let d = u.dict_bytes.get(c).copied().unwrap_or(0);
                if d > 0 {
                    bytes_pages_touched += d;
                    page_requests += 1;
                }
            }
        }
        prev = sel;
    }
    WindowCost {
        drawn: h.len(),
        needed,
        selected,
        requests: if l.per_file { selected } else { runs },
        bytes_selected: bytes_sel,
        bytes_needed: bytes_need,
        pages_whole,
        pages_touched,
        bytes_pages_touched,
        page_requests,
    }
}


// ── the edges ─────────────────────────────────────────────────────────────
// An edge lives in its SOURCE's tile (CSR), so edge tile `k` holds every edge
// whose `src_dense` is in vertex tile `k` — `fossil_sinks::manifest::EdgeInfo`
// says so and `fossil_runtime::layout` writes it. That is the whole of the
// mapping from a camera window to an edge URL: no box, no index, the same `k`.

/// SplitMix64 — a deterministic corpus is a re-runnable one, and no dependency.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// `(src, dst)` pairs, sorted by `(src, dst)` — CSR order, which is the order
/// `layout.rs` writes and the order every layout below shares, so a row index is
/// the same row in all of them.
struct Edges {
    src: Vec<u32>,
    dst: Vec<u32>,
}

/// `m` edges over the corpus: `intra` of them inside the source's own cluster,
/// the rest uniform. **Stipulated, not measured** — a real degree distribution
/// is skewed and a real community graph is not uniform inside a community. What
/// the measurement depends on is that most edges are short in `dense_id` after
/// the Morton renumbering, and that is what this reproduces.
fn build_edges(c: &Corpus, m: usize, intra: f64, seed: u64) -> Edges {
    let n = c.x.len();
    // Members of each cluster, as a counting-sort index over the NEW ids.
    let k = c.cluster.iter().copied().max().map_or(0, |v| v as usize + 1);
    let mut counts = vec![0usize; k + 1];
    for &cl in &c.cluster {
        counts[cl as usize + 1] += 1;
    }
    for i in 0..k {
        counts[i + 1] += counts[i];
    }
    let offsets = counts.clone();
    let mut members = vec![0u32; n];
    let mut cursor = counts;
    for (i, &cl) in c.cluster.iter().enumerate() {
        members[cursor[cl as usize]] = i as u32;
        cursor[cl as usize] += 1;
    }

    let mut rng = Rng(seed);
    let cut = (intra * u64::MAX as f64) as u64;
    let mut keys: Vec<u64> = Vec::with_capacity(m);
    while keys.len() < m {
        let s = rng.below(n);
        let d = if rng.next() < cut {
            let cl = c.cluster[s] as usize;
            let (lo, hi) = (offsets[cl], offsets[cl + 1]);
            if hi > lo {
                members[lo + rng.below(hi - lo)] as usize
            } else {
                rng.below(n)
            }
        } else {
            rng.below(n)
        };
        if s != d {
            keys.push(((s as u64) << 32) | d as u64);
        }
    }
    keys.sort_unstable();
    let mut src = Vec::with_capacity(m);
    let mut dst = Vec::with_capacity(m);
    for key in keys {
        src.push((key >> 32) as u32);
        dst.push((key & 0xFFFF_FFFF) as u32);
    }
    Edges { src, dst }
}

fn edge_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("src_dense", DataType::UInt32, false),
        Field::new("dst_dense", DataType::UInt32, false),
    ]))
}

fn edge_batch(e: &Edges, lo: usize, hi: usize) -> RecordBatch {
    RecordBatch::try_new(
        edge_schema(),
        vec![
            Arc::new(UInt32Array::from_iter_values(e.src[lo..hi].iter().copied())),
            Arc::new(UInt32Array::from_iter_values(e.dst[lo..hi].iter().copied())),
        ],
    )
    .expect("edge columns are the schema")
}

/// Row ranges of each occupied source tile, in tile order: `(k, lo, hi)`.
/// Production writes a file only for an occupied tile — "a 404 says it for free".
fn occupied_tiles(e: &Edges) -> Vec<(u64, usize, usize)> {
    let mut out: Vec<(u64, usize, usize)> = Vec::new();
    let mut i = 0usize;
    while i < e.src.len() {
        let k = u64::from(e.src[i]) >> 12;
        let lo = i;
        while i < e.src.len() && u64::from(e.src[i]) >> 12 == k {
            i += 1;
        }
        out.push((k, lo, i));
    }
    out
}

/// One file, one row group per occupied source tile — the edge analogue of
/// "a row group is a tile". Row groups are cut with `flush()`, so a boundary is
/// a tile boundary and not a batch-size accident.
fn write_edge_single(
    path: &Path,
    e: &Edges,
    tiles: &[(u64, usize, usize)],
    compression: Compression,
    dictionary: bool,
) {
    let props = WriterProperties::builder()
        .set_compression(compression)
        .set_dictionary_enabled(dictionary)
        .build();
    let file = fs::File::create(path).expect("create edges");
    let mut w =
        parquet::arrow::ArrowWriter::try_new(file, edge_schema(), Some(props)).expect("writer");
    for &(_, lo, hi) in tiles {
        w.write(&edge_batch(e, lo, hi)).expect("write");
        w.flush().expect("close row group at the tile boundary");
    }
    w.close().expect("close");
}

fn write_edge_per_tile(
    dir: &Path,
    e: &Edges,
    tiles: &[(u64, usize, usize)],
    compression: Compression,
    dictionary: bool,
) -> Vec<(PathBuf, usize)> {
    fs::create_dir_all(dir).expect("mkdir");
    let props = WriterProperties::builder()
        .set_compression(compression)
        .set_dictionary_enabled(dictionary)
        .build();
    let mut out = Vec::with_capacity(tiles.len());
    for &(k, lo, hi) in tiles {
        let path = dir.join(format!("tile{k}.parquet"));
        let file = fs::File::create(&path).expect("create tile");
        let mut w = parquet::arrow::ArrowWriter::try_new(file, edge_schema(), Some(props.clone()))
            .expect("writer");
        w.write(&edge_batch(e, lo, hi)).expect("write tile");
        w.close().expect("close tile");
        out.push((path, lo));
    }
    out
}

/// What one window costs on the edge side, given the vertex tiles it selected.
struct EdgeCost {
    /// Units whose `src_dense` range meets a selected vertex tile.
    selected: usize,
    requests: usize,
    bytes: u64,
    rows_fetched: usize,
    /// Edges with BOTH endpoints inside the window rectangle — what can be drawn.
    rows_drawable: usize,
    pages_whole: usize,
    pages_touched: usize,
    /// On-disk bytes of the touched pages (`OffsetIndex` sizes, dictionary pages
    /// included), and the range requests they would take.
    bytes_pages_touched: u64,
    page_requests: usize,
    /// Pages a reader driving the `ColumnIndex` would actually fetch: those whose
    /// own `src_dense` min-max meets a selected vertex tile. The rows are sorted
    /// by `src_dense`, so a page's first and last row ARE its min and max — this
    /// is the index's own numbers, not an oracle over the data.
    pages_index: usize,
    bytes_index: u64,
}

/// `selected_tile`: indexed by vertex tile id. `drawable`: sorted edge row
/// indices whose two endpoints are both inside the window.
fn edge_window_cost(
    l: &Layout,
    selected_tile: &[bool],
    drawable: &[u32],
    src: &[u32],
) -> EdgeCost {
    let (mut selected, mut runs, mut bytes, mut rows) = (0usize, 0usize, 0u64, 0usize);
    let (mut pages_whole, mut pages_touched) = (0usize, 0usize);
    let (mut bytes_pages_touched, mut page_requests) = (0u64, 0usize);
    let (mut pages_index, mut bytes_index) = (0usize, 0u64);
    let mut prev = false;
    for u in &l.units {
        let (a, b) = u.src_range;
        let sel = a <= b && {
            let (ka, kb) = ((a >> 12) as usize, (b >> 12) as usize);
            (ka..=kb).any(|k| selected_tile.get(k).copied().unwrap_or(false))
        };
        if sel {
            selected += 1;
            bytes += u.bytes;
            rows += u.rows;
            if !prev {
                runs += 1;
            }
            pages_whole += u.pages.len();
            let mut prev_end: Option<(usize, usize)> = None;
            let mut dict_wanted: Vec<usize> = Vec::new();
            for pg in &u.pages {
                if !touches(drawable, pg.row_start, pg.rows) {
                    continue;
                }
                pages_touched += 1;
                bytes_pages_touched += pg.bytes;
                if prev_end != Some((pg.col, pg.row_start)) {
                    page_requests += 1;
                }
                prev_end = Some((pg.col, pg.row_start + pg.rows));
                if !dict_wanted.contains(&pg.col) {
                    dict_wanted.push(pg.col);
                }
            }
            for c in dict_wanted {
                let d = u.dict_bytes.get(c).copied().unwrap_or(0);
                if d > 0 {
                    bytes_pages_touched += d;
                    page_requests += 1;
                }
            }
            // The same pages, chosen the only way a reader can choose them.
            for pg in &u.pages {
                let (a, b) = (
                    u64::from(src[pg.row_start]),
                    u64::from(src[pg.row_start + pg.rows - 1]),
                );
                if ((a >> 12) as usize..=(b >> 12) as usize)
                    .any(|k| selected_tile.get(k).copied().unwrap_or(false))
                {
                    pages_index += 1;
                    bytes_index += pg.bytes;
                }
            }
        }
        prev = sel;
    }
    EdgeCost {
        selected,
        requests: if l.per_file { selected } else { runs },
        bytes,
        rows_fetched: rows,
        rows_drawable: drawable.len(),
        pages_whole,
        pages_touched,
        bytes_pages_touched,
        page_requests,
        pages_index,
        bytes_index,
    }
}

// ── the report ────────────────────────────────────────────────────────────

fn arg(name: &str, default: &str) -> String {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| default.to_string())
}

fn mb(bytes: u64) -> String {
    if bytes < 1_000_000 {
        format!("{:.1} kB", bytes as f64 / 1_000.0)
    } else {
        format!("{:.2} MB", bytes as f64 / 1_000_000.0)
    }
}

fn yesno(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}

/// `--inspect <file.parquet>`: what one file already on disk carries. Exists so
/// the DuckDB half of claim 4 can be checked against a Parquet the real `fossil`
/// binary wrote (bundled DuckDB), not only against the CLI's output.
fn inspect(path: &Path) {
    let f = read_facts(path, 0, true);
    println!("# {}\n", path.display());
    println!("| fact | value |");
    println!("|---|---|");
    println!("| file bytes | {} |", f.file_bytes);
    println!("| footer bytes (thrift + 4 + PAR1) | {} |", f.footer_bytes);
    println!("| row groups | {} |", f.row_groups.len());
    println!("| rows | {} |", f.row_groups.iter().map(|r| r.rows).sum::<usize>());
    println!("| data pages | {} |", f.data_pages);
    println!("| dictionary pages | {} |", f.dict_pages);
    println!("| page index present | {} |", yesno(f.page_index_present));
    println!("| page index bytes | {} |", f.page_index_bytes);
    println!("\nStored bytes by column: {:?}", f.columns);
}

fn main() {
    let inspect_path = arg("--inspect", "");
    if !inspect_path.is_empty() {
        inspect(Path::new(&inspect_path));
        return;
    }
    let n: usize = arg("--rows", "5000000").parse().expect("--rows");
    let full = arg("--schema", "draw") == "full";
    let k: u32 = arg("--clusters", "2048").parse().expect("--clusters");
    let dir = PathBuf::from(arg("--dir", "/tmp/f7-writer-bench"));
    let tile_pages = arg("--tile-pages", "yes") == "yes";
    fs::create_dir_all(&dir).expect("mkdir");
    let tiles = n.div_ceil(TILE_ROWS);

    println!("# F7 · `DuckDB COPY` vs `arrow-rs`, measured\n");
    println!(
        "{n} vertices, {k} clusters, tile = {TILE_ROWS} rows, {tiles} tiles. Schema: {}. \
         `parquet`/`arrow` 58.3.0 (the workspace pin). DuckDB CLI: {}.\n",
        if full {
            "`dense_id`, `x`, `y`, `cluster_id`, `subject`, `community`"
        } else {
            "`dense_id`, `x`, `y`, `cluster_id` (the drawing tile)"
        },
        duckdb_version(),
    );

    let t = Instant::now();
    let corpus = build_corpus(n, k, full);
    println!("Corpus built in {:.1} s.", t.elapsed().as_secs_f64());

    // ── write ────────────────────────────────────────────────────────────
    let b_single = dir.join("arrow_single.parquet");
    let t = Instant::now();
    write_single(
        &b_single,
        &corpus,
        n,
        Some(TILE_ROWS),
        Compression::UNCOMPRESSED,
        true,
    );
    let b_secs = t.elapsed().as_secs_f64();

    let b_snappy = dir.join("arrow_single_snappy.parquet");
    write_single(
        &b_snappy,
        &corpus,
        n,
        Some(TILE_ROWS),
        Compression::SNAPPY,
        true,
    );

    // The byte-parity variant: DuckDB's codec and DuckDB's per-column dictionary
    // choice (off for the near-unique columns), so the size column compares two
    // writers and not two encoding policies.
    let b_plain = dir.join("arrow_single_plain_snappy.parquet");
    write_single(
        &b_plain,
        &corpus,
        n,
        Some(TILE_ROWS),
        Compression::SNAPPY,
        false,
    );

    // The page-index counterfactual: crate-default row groups, so a row group
    // holds many pages and the index has something to skip.
    let b_default_rg = dir.join("arrow_single_default_rg.parquet");
    write_single(&b_default_rg, &corpus, n, None, Compression::SNAPPY, true);

    let b_tiles_dir = dir.join("arrow_tiles");
    let t = Instant::now();
    let b_tile_paths = write_per_tile(&b_tiles_dir, &corpus, n, Compression::UNCOMPRESSED);
    let b_tiles_secs = t.elapsed().as_secs_f64();

    // DuckDB re-encodes the arrow-rs corpus, so both ways see the same rows in
    // the same order. The per-tile statement is `layout.rs`'s, verbatim.
    let duck_tiles_dir = dir.join("duck_tiles");
    fs::create_dir_all(&duck_tiles_dir).expect("mkdir duck tiles");
    let mut sql = format!(
        "CREATE OR REPLACE TEMP TABLE __fossil_enriched AS \
         SELECT * FROM read_parquet('{}') ORDER BY 1;\n",
        b_single.display()
    );
    // A2 — one file, DuckDB's own defaults, nothing said about row groups.
    sql.push_str(&format!(
        "COPY (SELECT * FROM __fossil_enriched ORDER BY dense_id) TO '{}' (FORMAT PARQUET);\n",
        dir.join("duck_single_default.parquet").display()
    ));
    // A3 — one file, the proposal's row-group size, DuckDB's writer.
    sql.push_str(&format!(
        "COPY (SELECT * FROM __fossil_enriched ORDER BY dense_id) TO '{}' \
         (FORMAT PARQUET, ROW_GROUP_SIZE {TILE_ROWS});\n",
        dir.join("duck_single_4096.parquet").display()
    ));
    // A1 — production: one COPY per tile, `(FORMAT PARQUET)` and nothing else.
    for t in 0..tiles {
        let (lo, hi) = (t * TILE_ROWS, (t + 1) * TILE_ROWS);
        sql.push_str(&format!(
            "COPY (SELECT * FROM __fossil_enriched WHERE dense_id >= {lo} AND dense_id < {hi} \
             ORDER BY dense_id) TO '{}' (FORMAT PARQUET);\n",
            duck_tiles_dir.join(format!("chunk{t}.parquet")).display()
        ));
    }
    let t = Instant::now();
    let have_duck = duckdb(&sql, &dir.join("duck.sql"));
    let duck_secs = t.elapsed().as_secs_f64();

    println!(
        "arrow-rs encoded in {b_secs:.1} s (one file) and {b_tiles_secs:.1} s ({tiles} files); \
         DuckDB wrote all four outputs in {duck_secs:.1} s.\n"
    );

    // ── read every byte of metadata back ─────────────────────────────────
    let mut layouts: Vec<Layout> = Vec::new();
    if have_duck {
        layouts.push(Layout::per_tile(
            "A1 · DuckDB COPY, one file per tile",
            "`(FORMAT PARQUET)` — production, `layout.rs:570`".into(),
            (0..tiles)
                .map(|t| {
                    read_facts(
                        &duck_tiles_dir.join(format!("chunk{t}.parquet")),
                        t * TILE_ROWS,
                        tile_pages,
                    )
                })
                .collect(),
            tile_pages,
        ));
        layouts.push(Layout::single(
            "A2 · DuckDB COPY, one file, default row groups",
            "`(FORMAT PARQUET)`".into(),
            read_facts(&dir.join("duck_single_default.parquet"), 0, true),
        ));
        layouts.push(Layout::single(
            "A3 · DuckDB COPY, one file, ROW_GROUP_SIZE 4096",
            "`(FORMAT PARQUET, ROW_GROUP_SIZE 4096)`".into(),
            read_facts(&dir.join("duck_single_4096.parquet"), 0, true),
        ));
    }
    layouts.push(Layout::single(
        "B1 · arrow-rs, one file, 4096-row row groups",
        "`ArrowWriter`, crate defaults + row-group size — the proposal".into(),
        read_facts(&b_single, 0, true),
    ));
    layouts.push(Layout::single(
        "B2 · arrow-rs, one file, 4096 + snappy",
        "B1 with `set_compression(SNAPPY)` — DuckDB's default codec".into(),
        read_facts(&b_snappy, 0, true),
    ));
    layouts.push(Layout::single(
        "B4 · arrow-rs, 4096 + snappy, dictionary off",
        "B2 with `set_dictionary_enabled(false)` — byte parity with DuckDB".into(),
        read_facts(&b_plain, 0, true),
    ));
    layouts.push(Layout::single(
        "B5 · arrow-rs, default row groups + snappy",
        "no row-group size set (1,048,576 rows) — the page-index counterfactual".into(),
        read_facts(&b_default_rg, 0, true),
    ));
    layouts.push(Layout::per_tile(
        "B3 · arrow-rs, one file per tile",
        "`fossil_df::files::batches_to_parquet` shape".into(),
        b_tile_paths
            .iter()
            .enumerate()
            .map(|(t, p)| read_facts(p, t * TILE_ROWS, tile_pages))
            .collect(),
        tile_pages,
    ));

    // ── 1 · footers ──────────────────────────────────────────────────────
    println!("## 1 · Footer bytes, page index and total size\n");
    println!(
        "`footer` is the thrift `FileMetaData` + the 4-byte length + `PAR1`. The page index \
         (`ColumnIndex` + `OffsetIndex`) is NOT inside it and is counted apart.\n"
    );
    println!(
        "| layout | how | files | row groups | footer total | footer/rg | page index | data pages | dict pages | stored |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for l in &layouts {
        println!(
            "| {} | {} | {} | {} | {} B | {} B | {} B | {} | {} | {} |",
            l.name,
            l.how,
            l.files,
            l.row_groups(),
            l.footer_bytes,
            l.footer_bytes / l.row_groups() as u64,
            l.page_index_bytes,
            if l.pages_measured {
                l.data_pages.to_string()
            } else {
                "not measured".into()
            },
            if l.pages_measured {
                l.dict_pages.to_string()
            } else {
                "not measured".into()
            },
            mb(l.file_bytes),
        );
    }

    let base = layouts.first().map_or(1, |l| l.file_bytes.max(1));
    println!("\n| layout | stored | ÷ {} | metadata a reader must hold (footers + page index) |", layouts[0].name);
    println!("|---|---|---|---|");
    for l in &layouts {
        println!(
            "| {} | {} | {:.3}× | {} in {} request(s) |",
            l.name,
            mb(l.file_bytes),
            l.file_bytes as f64 / base as f64,
            mb(l.footer_bytes + l.page_index_bytes),
            l.files,
        );
    }

    println!("\nStored bytes by column:\n");
    let names: Vec<String> = layouts
        .last()
        .expect("a layout")
        .columns
        .keys()
        .cloned()
        .collect();
    println!("| layout | {} |", names.join(" | "));
    println!("|---{}|", "|---".repeat(names.len()));
    for l in &layouts {
        let cells: Vec<String> = names
            .iter()
            .map(|k| mb(l.columns.get(k).copied().unwrap_or(0)))
            .collect();
        println!("| {} | {} |", l.name, cells.join(" | "));
    }

    // ── 4 · the page index ───────────────────────────────────────────────
    println!("\n## 4 · Is the page index written?\n");
    println!("| layout | `ColumnIndex` + `OffsetIndex` present | bytes | pages per row group per column |");
    println!("|---|---|---|---|");
    for l in &layouts {
        let ppc = if l.pages_measured {
            format!(
                "{:.2}",
                l.data_pages as f64 / (l.row_groups() * names.len()) as f64
            )
        } else {
            "not measured".into()
        };
        println!(
            "| {} | **{}** | {} | {} |",
            l.name,
            yesno(l.page_index_present),
            l.page_index_bytes,
            ppc
        );
    }

    // ── 2 & 3 · windows ──────────────────────────────────────────────────
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for i in 0..n {
        min_x = min_x.min(corpus.x[i]);
        min_y = min_y.min(corpus.y[i]);
        max_x = max_x.max(corpus.x[i]);
        max_y = max_y.max(corpus.y[i]);
    }
    let span = (max_x - min_x).max(max_y - min_y);
    let centres: Vec<(f32, f32)> = (0..WINDOWS)
        .map(|w| {
            let i = (w + 1) * n / (WINDOWS + 1);
            (corpus.x[i], corpus.y[i])
        })
        .collect();
    let (cx0, cy0) = centres[WINDOWS / 2];
    let half = size_window(&corpus, cx0, cy0, span);
    let window_hits: Vec<Vec<u32>> = centres
        .iter()
        .map(|&(cx, cy)| hits(&corpus, cx, cy, half))
        .collect();

    println!("\n## 2 & 3 · Requests, bytes and pages per window\n");
    println!(
        "Nine squares of half-width {half:.0} on a canvas {span:.0} across, one centre per ninth \
         of `dense_id` (= Morton order), sized on the middle one to hold {WINDOW_VERTICES} \
         vertices. A request is one file for a per-file layout, one maximal run of consecutive \
         selected row groups otherwise. `needed` is the units holding a drawn vertex; `selected` \
         is what the `x`/`y` boxes admit, which is all a reader with the footer can prune to.\n"
    );

    // Per-window tile arithmetic, off the proposal's layout.
    let b1 = layouts
        .iter()
        .find(|l| l.name.starts_with("B1"))
        .expect("B1 exists");
    println!("| # | drawn | needed tiles | selected tiles | overread |");
    println!("|---|---|---|---|---|");
    let mut sum_need = 0usize;
    let mut sum_sel = 0usize;
    let (mut min_or, mut max_or) = (f64::MAX, 0f64);
    for (w, &(cx, cy)) in centres.iter().enumerate() {
        let c = window_cost(b1, &window_hits[w], cx, cy, half);
        let or = c.selected as f64 / c.needed.max(1) as f64;
        min_or = min_or.min(or);
        max_or = max_or.max(or);
        sum_need += c.needed;
        sum_sel += c.selected;
        println!(
            "| {} | {} | {} | {} | {:.2}× |",
            w + 1,
            c.drawn,
            c.needed,
            c.selected,
            or
        );
    }
    println!(
        "\nOverread {min_or:.2}×–{max_or:.2}×, mean {:.2}× ({sum_sel} selected over {sum_need} \
         needed). The plan's prior figure is 1.05×–1.21×.",
        sum_sel as f64 / sum_need as f64
    );

    println!(
        "\n| layout | requests/window | bytes/window | bytes if only needed | data pages/window | pages holding a drawn vertex | index, once |"
    );
    println!("|---|---|---|---|---|---|---|");
    let mut page_rows: Vec<String> = Vec::new();
    for l in &layouts {
        let (mut req, mut sel, mut need, mut pw, mut pt) = (0usize, 0u64, 0u64, 0usize, 0usize);
        let (mut pb, mut preq) = (0u64, 0usize);
        for (w, &(cx, cy)) in centres.iter().enumerate() {
            let c = window_cost(l, &window_hits[w], cx, cy, half);
            req += c.requests;
            sel += c.bytes_selected;
            need += c.bytes_needed;
            pw += c.pages_whole;
            pt += c.pages_touched;
            pb += c.bytes_pages_touched;
            preq += c.page_requests;
        }
        page_rows.push(if l.pages_measured {
            format!(
                "| {} | {} | {:.1} | {:.1} | {:.1} | {} | {} |",
                l.name,
                yesno(l.page_index_present),
                pw as f64 / WINDOWS as f64,
                pt as f64 / WINDOWS as f64,
                (pw - pt) as f64 / WINDOWS as f64,
                // No `OffsetIndex` means no page byte size to read — and a
                // reader in that file has no way to fetch a page alone either.
                if l.page_bytes_exact {
                    mb(pb / WINDOWS as u64)
                } else {
                    "not measured (no page index)".into()
                },
                if l.page_bytes_exact {
                    format!("{:.1}", preq as f64 / WINDOWS as f64)
                } else {
                    "n/a".into()
                },
            )
        } else {
            format!(
                "| {} | {} | not measured | | | | |",
                l.name,
                yesno(l.page_index_present)
            )
        });
        let w = WINDOWS as u64;
        println!(
            "| {} | {:.1} | {} | {} | {} | {} | {} in {} request(s) |",
            l.name,
            req as f64 / w as f64,
            mb(sel / w),
            mb(need / w),
            if l.pages_measured {
                format!("{:.1}", pw as f64 / w as f64)
            } else {
                "not measured".into()
            },
            if l.pages_measured {
                format!("{:.1}", pt as f64 / w as f64)
            } else {
                "not measured".into()
            },
            mb(l.footer_bytes + l.page_index_bytes),
            l.files,
        );
    }
    println!(
        "\nThe last column is the cost of *having* the boxes at all, which the per-window columns \
         exclude: one file pays it once and keeps it; {tiles} files have no such place, so the \
         equivalent is a footer read per tile or a sidecar index nothing in the tree writes."
    );

    println!("\n## 4b · What the page index would skip, in pages\n");
    println!(
        "`whole` is the data pages inside the selected units; `touched` is the pages holding a \
         drawn vertex, which is what a reader with `OffsetIndex` + `ColumnIndex` can fetch \
         instead. `skipped` is the difference, and it is only spendable where the index exists. \
         Page bodies only — page headers are not counted, so the byte column is a floor.\n"
    );
    println!(
        "| layout | page index | pages/window whole | touched | skipped | bytes if page-pruned | range requests then |"
    );
    println!("|---|---|---|---|---|---|---|");
    for r in &page_rows {
        println!("{r}");
    }

    // ── 5 · the edges ────────────────────────────────────────────────────
    let m: usize = arg("--edges", "20000000").parse().expect("--edges");
    if m == 0 {
        return;
    }
    let intra: f64 = arg("--intra", "0.85").parse().expect("--intra");
    let t = Instant::now();
    let edges = build_edges(&corpus, m, intra, 0x5EED);
    let tiles_e = occupied_tiles(&edges);
    let edge_secs = t.elapsed().as_secs_f64();

    println!("\n---\n\n# The edges\n");
    println!(
        "{m} edges over the same {n} vertices ({:.1} per vertex), {:.0}% of them inside the \
         source's own cluster — **stipulated, not measured**: a real degree distribution is \
         skewed. Sorted `(src_dense, dst_dense)` = CSR. {} occupied source tiles of {tiles}; \
         rows per occupied tile min {} / median {} / max {}. Built in {edge_secs:.1} s.\n",
        m as f64 / n as f64,
        intra * 100.0,
        tiles_e.len(),
        tiles_e.iter().map(|t| t.2 - t.1).min().unwrap_or(0),
        {
            let mut v: Vec<usize> = tiles_e.iter().map(|t| t.2 - t.1).collect();
            v.sort_unstable();
            v[v.len() / 2]
        },
        tiles_e.iter().map(|t| t.2 - t.1).max().unwrap_or(0),
    );

    // arrow-rs side, written first — DuckDB re-encodes this file, so both ways
    // see the same edges in the same order.
    let e_arrow = dir.join("edges_csr_arrow.parquet");
    write_edge_single(&e_arrow, &edges, &tiles_e, Compression::UNCOMPRESSED, true);
    let e_arrow_plain = dir.join("edges_csr_arrow_plain.parquet");
    write_edge_single(
        &e_arrow_plain,
        &edges,
        &tiles_e,
        Compression::SNAPPY,
        false,
    );
    let e_arrow_tiles = write_edge_per_tile(
        &dir.join("edges_arrow_tiles"),
        &edges,
        &tiles_e,
        Compression::UNCOMPRESSED,
        true,
    );

    // DuckDB, the production statements: `layout.rs:632` writes the adjacency
    // itself, then `layout.rs:716` cuts it into tiles, reading the file back.
    let duck_e = dir.join("duck_edges");
    fs::create_dir_all(duck_e.join("by_source")).expect("mkdir");
    let by_source = duck_e.join("by_source.parquet");
    let mut sql = format!(
        "CREATE OR REPLACE TEMP TABLE __fossil_adjacency AS SELECT * FROM read_parquet('{}');\n\
         COPY (SELECT * FROM __fossil_adjacency ORDER BY src_dense, dst_dense) TO '{}' \
         (FORMAT PARQUET);\n\
         COPY (SELECT * FROM __fossil_adjacency ORDER BY dst_dense, src_dense) TO '{}' \
         (FORMAT PARQUET);\n\
         COPY (SELECT * FROM __fossil_adjacency ORDER BY src_dense, dst_dense) TO '{}' \
         (FORMAT PARQUET, ROW_GROUP_SIZE 16384);\n",
        e_arrow.display(),
        by_source.display(),
        duck_e.join("by_target.parquet").display(),
        duck_e.join("by_source_rg16384.parquet").display(),
    );
    for &(k, _, _) in &tiles_e {
        let (lo, hi) = (k << 12, (k + 1) << 12);
        sql.push_str(&format!(
            "COPY (SELECT * FROM read_parquet('{}') WHERE src_dense >= {lo} AND src_dense < {hi} \
             ORDER BY src_dense, dst_dense) TO '{}' (FORMAT PARQUET);\n",
            by_source.display(),
            duck_e.join("by_source").join(format!("tile{k}.parquet")).display(),
        ));
    }
    let t = Instant::now();
    let duck_edges_ok = have_duck && duckdb(&sql, &dir.join("duck_edges.sql"));
    println!(
        "DuckDB wrote the adjacency, its {} tiles, the CSC file and the fixed-row-group file in \
         {:.1} s.\n",
        tiles_e.len(),
        t.elapsed().as_secs_f64()
    );

    let mut elayouts: Vec<Layout> = Vec::new();
    if duck_edges_ok {
        elayouts.push(Layout::per_tile(
            "EA1 · DuckDB COPY, one file per source tile",
            "`(FORMAT PARQUET)` — production, `layout.rs:716`".into(),
            tiles_e
                .iter()
                .map(|&(k, lo, _)| {
                    read_facts(
                        &duck_e.join("by_source").join(format!("tile{k}.parquet")),
                        lo,
                        true,
                    )
                })
                .collect(),
            true,
        ));
        elayouts.push(Layout::single(
            "EA2 · DuckDB COPY, the untiled adjacency",
            "`(FORMAT PARQUET)` — production writes this too, `layout.rs:632`".into(),
            read_facts(&by_source, 0, true),
        ));
        elayouts.push(Layout::single(
            "EA3 · DuckDB COPY, one file, ROW_GROUP_SIZE 16384",
            "`(FORMAT PARQUET, ROW_GROUP_SIZE 16384)`".into(),
            read_facts(&duck_e.join("by_source_rg16384.parquet"), 0, true),
        ));
    }
    elayouts.push(Layout::single(
        "EB1 · arrow-rs, one file, a row group per source tile",
        "`ArrowWriter` + `flush()` at the tile boundary, crate defaults".into(),
        read_facts(&e_arrow, 0, true),
    ));
    elayouts.push(Layout::single(
        "EB2 · arrow-rs, same, snappy + dictionary off",
        "byte parity with DuckDB".into(),
        read_facts(&e_arrow_plain, 0, true),
    ));
    elayouts.push(Layout::per_tile(
        "EB3 · arrow-rs, one file per source tile",
        "`batches_to_parquet` shape".into(),
        e_arrow_tiles
            .iter()
            .map(|(p, lo)| read_facts(p, *lo, true))
            .collect(),
        true,
    ));

    println!("## 5 · Edge footers, page index and size\n");
    println!(
        "| layout | how | files | row groups | footer total | footer/rg | page index | data pages | pages per rg per column | stored |"
    );
    println!("|---|---|---|---|---|---|---|---|---|---|");
    for l in &elayouts {
        let rgs = l.row_groups();
        println!(
            "| {} | {} | {} | {} | {} B | {} B | {} B | {} | {:.2} | {} |",
            l.name,
            l.how,
            l.files,
            rgs,
            l.footer_bytes,
            l.footer_bytes / rgs as u64,
            l.page_index_bytes,
            l.data_pages,
            l.data_pages as f64 / (rgs * 2) as f64,
            mb(l.file_bytes),
        );
    }
    let ebase = elayouts.first().map_or(1, |l| l.file_bytes.max(1));
    println!("\n| layout | stored | ÷ {} | metadata a reader must hold |", elayouts[0].name);
    println!("|---|---|---|---|");
    for l in &elayouts {
        println!(
            "| {} | {} | {:.3}× | {} in {} request(s) |",
            l.name,
            mb(l.file_bytes),
            l.file_bytes as f64 / ebase as f64,
            mb(l.footer_bytes + l.page_index_bytes),
            l.files,
        );
    }

    // ── 6 · the two-stage window ─────────────────────────────────────────
    println!("\n## 6 · The two-stage window: vertices, then their edges\n");
    println!(
        "Stage 1 selects vertex tiles by the `x`/`y` box. Stage 2 needs the edges incident to \
         those vertices, and edge tile `k` IS vertex tile `k` — the mapping is the identity, so \
         stage 2 costs no lookup and no index. `drawable` is the edges with BOTH endpoints inside \
         the window; `fetched` is every row in the tiles a reader must take.\n"
    );

    let b1_units: &Vec<Unit> = &layouts
        .iter()
        .find(|l| l.name.starts_with("B1"))
        .expect("B1")
        .units;
    let mut selected_per_window: Vec<Vec<bool>> = Vec::new();
    let mut drawable_per_window: Vec<Vec<u32>> = Vec::new();
    let mut missed_in_edges: Vec<usize> = Vec::new();
    let mut inside = vec![false; n];
    for (w, &(cx, cy)) in centres.iter().enumerate() {
        let (x0, x1, y0, y1) = (cx - half, cx + half, cy - half, cy + half);
        let sel: Vec<bool> = b1_units
            .iter()
            .map(|u| {
                let (xmin, xmax, ymin, ymax) = u.bbox;
                xmin <= x1 && xmax >= x0 && ymin <= y1 && ymax >= y0
            })
            .collect();
        inside.iter_mut().for_each(|v| *v = false);
        for &r in &window_hits[w] {
            inside[r as usize] = true;
        }
        let mut drawable = Vec::new();
        let mut missed = 0usize;
        for i in 0..edges.src.len() {
            let (sv, dv) = (edges.src[i] as usize, edges.dst[i] as usize);
            if inside[sv] && inside[dv] {
                drawable.push(i as u32);
            }
            // An in-edge whose source tile the window never selects: CSR cannot
            // reach it, and only a by_target tile could.
            if inside[dv] && !sel.get(sv >> 12).copied().unwrap_or(false) {
                missed += 1;
            }
        }
        drawable_per_window.push(drawable);
        missed_in_edges.push(missed);
        selected_per_window.push(sel);
    }

    println!("| # | vertex tiles selected | edge tiles selected (EB1) | edges fetched | drawable | over-read |");
    println!("|---|---|---|---|---|---|");
    let eb1 = elayouts.iter().find(|l| l.name.starts_with("EB1")).expect("EB1");
    for w in 0..WINDOWS {
        let c = edge_window_cost(eb1, &selected_per_window[w], &drawable_per_window[w], &edges.src);
        println!(
            "| {} | {} | {} | {} | {} | {:.2}× |",
            w + 1,
            selected_per_window[w].iter().filter(|&&b| b).count(),
            c.selected,
            c.rows_fetched,
            c.rows_drawable,
            c.rows_fetched as f64 / c.rows_drawable.max(1) as f64
        );
    }

    println!(
        "\n| layout | requests/window | bytes/window | data pages/window | pages a `ColumnIndex` reader fetches | bytes then | pages holding a drawable edge (oracle) | oracle bytes |"
    );
    println!("|---|---|---|---|---|---|---|---|");
    let mut edge_cost_by_name: BTreeMap<String, (f64, u64)> = BTreeMap::new();
    for l in &elayouts {
        let (mut req, mut by, mut pw, mut pt) = (0usize, 0u64, 0usize, 0usize);
        let (mut pb, mut preq) = (0u64, 0usize);
        let (mut pi, mut bi) = (0usize, 0u64);
        let _ = preq;
        for w in 0..WINDOWS {
            let c = edge_window_cost(l, &selected_per_window[w], &drawable_per_window[w], &edges.src);
            req += c.requests;
            by += c.bytes;
            pw += c.pages_whole;
            pt += c.pages_touched;
            pb += c.bytes_pages_touched;
            preq += c.page_requests;
            pi += c.pages_index;
            bi += c.bytes_index;
        }
        let w = WINDOWS as u64;
        edge_cost_by_name.insert(l.name.to_string(), (req as f64 / w as f64, by / w));
        println!(
            "| {} | {:.1} | {} | {:.1} | {} | {} | {:.1} | {} |",
            l.name,
            req as f64 / w as f64,
            mb(by / w),
            pw as f64 / WINDOWS as f64,
            if l.page_bytes_exact {
                format!("{:.1}", pi as f64 / WINDOWS as f64)
            } else {
                "n/a (no page index)".into()
            },
            if l.page_bytes_exact {
                mb(bi / w)
            } else {
                "not measured".into()
            },
            pt as f64 / WINDOWS as f64,
            if l.page_bytes_exact {
                mb(pb / w)
            } else {
                "not measured".into()
            },
        );
    }

    // The whole dataset: stage 1 + stage 2, per pairing of writers.
    println!("\n### The window, both stages together\n");
    println!("| dataset | vertex requests | vertex bytes | edge requests | edge bytes | TOTAL requests | TOTAL bytes |");
    println!("|---|---|---|---|---|---|---|");
    for (vname, ename, label) in [
        ("A1", "EA1", "production today (DuckDB, a file per tile)"),
        ("A3", "EA3", "DuckDB, one file each, ROW_GROUP_SIZE set"),
        ("B1", "EB1", "arrow-rs, one file each, crate defaults"),
        ("B4", "EB2", "arrow-rs, one file each, snappy + dictionary off"),
    ] {
        let Some(v) = layouts.iter().find(|l| l.name.starts_with(vname)) else {
            continue;
        };
        let (mut vreq, mut vby) = (0usize, 0u64);
        for (w, &(cx, cy)) in centres.iter().enumerate() {
            let c = window_cost(v, &window_hits[w], cx, cy, half);
            vreq += c.requests;
            vby += c.bytes_selected;
        }
        let vreq = vreq as f64 / WINDOWS as f64;
        let vby = vby / WINDOWS as u64;
        let Some((ereq, eby)) = elayouts
            .iter()
            .find(|l| l.name.starts_with(ename))
            .and_then(|l| edge_cost_by_name.get(l.name).copied())
        else {
            continue;
        };
        println!(
            "| {label} | {vreq:.1} | {} | {ereq:.1} | {} | **{:.1}** | **{}** |",
            mb(vby),
            mb(eby),
            vreq + ereq,
            mb(vby + eby),
        );
    }

    // ── 7 · CSR and CSC ──────────────────────────────────────────────────
    println!("\n## 7 · CSR and CSC — does a window need both?\n");
    let csc_duck = duck_e.join("by_target.parquet");
    if duck_edges_ok {
        let f = read_facts(&csc_duck, 0, false);
        println!(
            "`by_target.parquet` (DuckDB, CSC): {} in {} row groups, footer {} B, page index {} B, \
             **and no tiles** — `layout.rs` skips every adjacency whose `ordered_by` is not `Src`, \
             so `by_target/tile{{k}}.parquet` is never written. Confirmed on the artefact below.\n",
            mb(f.file_bytes),
            f.row_groups.len(),
            f.footer_bytes,
            f.page_index_bytes
        );
    }
    let miss_min = missed_in_edges.iter().copied().min().unwrap_or(0);
    let miss_max = missed_in_edges.iter().copied().max().unwrap_or(0);
    let draw_avg =
        drawable_per_window.iter().map(Vec::len).sum::<usize>() as f64 / WINDOWS as f64;
    println!(
        "Per window, edges whose destination is drawn but whose SOURCE tile the window never \
         selects: {miss_min}–{miss_max} (mean {:.0}), against {draw_avg:.0} drawable edges. Those \
         are the edges CSR alone cannot reach.",
        missed_in_edges.iter().sum::<usize>() as f64 / WINDOWS as f64,
    );
}
