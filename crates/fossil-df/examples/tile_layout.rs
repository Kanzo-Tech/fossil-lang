//! What a camera window costs, in footers, requests and bytes, under the two
//! Parquet layouts — one file per tile, or one file with 4,096-row row groups.
//!
//! ADR-0046 §9 asserts three things about the second layout: a 4,096-row tile is
//! one row group, its footer is 710 bytes, and over nine windows the `x`/`y`
//! boxes select 14–23 tiles where 13–21 are needed (overread 1.05×–1.21×). Two
//! of those are arithmetic over a corpus that no longer exists on disk. This
//! example writes both layouts and observes all three, plus the number ADR-0046
//! never took: **how many requests a window costs**, which is where the two
//! layouts actually differ.
//!
//! # What it measures
//!
//! 1. **Footer bytes** — the thrift `FileMetaData` plus its 8-byte trailer, per
//!    file and in total, and separately the **page index** (`ColumnIndex` +
//!    `OffsetIndex`) bytes, which live outside that blob.
//! 2. **Requests per window.** A tile that is its own file is its own request:
//!    the count is the number of selected tiles. Row groups inside one file
//!    coalesce — a run of consecutive selected row groups is one HTTP range
//!    request — so the count is the number of maximal runs.
//! 3. **Bytes per window**, and overread as selected/needed, in tiles and in
//!    bytes. `needed` is the set of tiles holding at least one vertex inside
//!    the window rectangle; `selected` is the set whose `x`/`y` box intersects
//!    it, which is what a reader with the footer in hand can prune to.
//! 4. **Whether the page index is written**, by `arrow-rs` and by `DuckDB`, in
//!    the versions in this tree — read back off the bytes, not assumed.
//!
//! # What it does NOT measure
//!
//! - **No edges.** Vertices only. §3.2 of ADR-0042 measured edge placement and
//!   this changes nothing about it: an edge tile is keyed by its source's
//!   `dense_id` range either way, so the layout question is the same question
//!   and the vertex file is where it is cheapest to ask.
//! - **No network.** A "request" here is a range that a reader would have to
//!   ask for; nothing is served over HTTP, so latency and connection reuse are
//!   out of scope. The request *counts* are what ADR-0042 §3.1's table counted,
//!   and they are comparable to it; the milliseconds are not measured at all.
//! - **The community structure is stipulated, not computed.** `community_hierarchy`
//!   is not run — the corpus is `k` equal-sized clusters laid out by the same
//!   `cluster_layout` phyllotaxis-on-a-Z-grid the writer uses, then renumbered by
//!   the same Morton rank. What the measurement depends on is Morton order over a
//!   clumpy 2-D placement, and that is reproduced exactly; what is not reproduced
//!   is a skewed community-size distribution, which would change the *needed*
//!   counts and is untested here.
//! - **Compression is off**, because `batches_to_parquet` leaves it off. Byte
//!   figures are therefore uncompressed-page figures and are a **ceiling**; the
//!   ratios between the two layouts are what survives turning it on, not the
//!   absolute megabytes.
//!
//! # Running it
//!
//! Release only — 5M rows in a debug build measures the absence of inlining.
//!
//! ```text
//! cargo run --release -p fossil-df --example tile_layout -- --rows 5000000 --dir /tmp/f7
//! ```
//!
//! To add the `DuckDB` COPY comparison, run this once, then the `DuckDB` CLI
//! over the corpus it left behind, then this again — it picks the files up:
//!
//! ```text
//! duckdb -c "
//!   COPY (SELECT * FROM read_parquet('/tmp/f7/corpus.parquet') ORDER BY dense_id)
//!     TO '/tmp/f7/duck_single.parquet' (FORMAT PARQUET, ROW_GROUP_SIZE 4096);
//!   COPY (SELECT *, (dense_id >> 12) AS tile FROM read_parquet('/tmp/f7/corpus.parquet'))
//!     TO '/tmp/f7/duck_tiles' (FORMAT PARQUET, PARTITION_BY (tile), OVERWRITE_OR_IGNORE);"
//! ```

// Deliberate numeric code: dense ids and counts cast to `f32`/`f64` for the
// layout arithmetic and for ratios in the report. Same declension `layout.rs`
// makes for the same reason.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_lossless
)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use datafusion::arrow::array::{Float32Array, StringArray, UInt32Array};
use datafusion::arrow::datatypes::{DataType, Field, Schema};
use datafusion::arrow::record_batch::RecordBatch;
use fossil_df::files::batches_to_parquet;
use parquet::arrow::ArrowWriter;
use parquet::file::metadata::{PageIndexPolicy, ParquetMetaData, ParquetMetaDataReader};
use parquet::file::properties::WriterProperties;
use parquet::file::statistics::Statistics;
use parquet::schema::types::ColumnPath;

/// Which columns get `parquet`'s dictionary encoding.
#[derive(Clone, Copy)]
enum Dict {
    /// What `batches_to_parquet` does today: the crate default, dictionary on
    /// for everything.
    Default,
    /// Dictionary off for everything.
    Off,
    /// Off for the near-unique columns, on for the repeated one.
    PerColumn,
}

/// Rows per tile — `fossil_sinks::DEFAULT_CHUNK_SIZE`, restated here because the
/// example must not depend on the crate that owns the manifest.
const TILE_ROWS: usize = 4_096;
/// How many windows to pan across. Nine, so the overread band is comparable to
/// the nine ADR-0046 §9 reports.
const WINDOWS: usize = 9;
/// Vertices a window is sized to contain, matching the ADR-0042 harness.
const WINDOW_VERTICES: usize = 20_000;
/// The `parquet` crate version this measurement is about. There is no runtime
/// way to ask, so it is the workspace pin restated; if `Cargo.toml` moves and
/// this does not, the page-index verdict is attributed to the wrong version.
const PARQUET_PIN: &str = "58 (workspace pin)";

// ──────────────────────────────────────────────────────────────────────────
// The corpus
// ──────────────────────────────────────────────────────────────────────────

/// The four drawing columns, in `dense_id` order — row `i` **is** `dense_id` `i`
/// (ADR-0042 §3.5: the drawing tile carries geometry and nothing else).
struct Corpus {
    x: Vec<f32>,
    y: Vec<f32>,
    cluster: Vec<u32>,
    /// The pre-Morton id, which is what the subject IRI is built from — a
    /// stable identity that the renumbering scrambles (ADR-0042 §3.4).
    orig: Vec<u32>,
    /// Carry `subject` and `community` as well as the four drawing columns.
    /// ADR-0042 §3.5 decided the drawing tile carries geometry only; the wide
    /// shape is here because the footer figure this measurement is checked
    /// against was taken on a corpus that had them.
    full: bool,
}

/// Golden angle, cluster spacing and packing radius — copied from
/// `fossil_runtime::layout`, which owns `DuckDB` and so cannot be depended on
/// from here. If those constants move, this example measures a corpus the
/// writer no longer produces.
const GOLDEN_ANGLE: f32 = 2.399_963_2;
const CLUSTER_SPACING: f32 = 100.0;
const INTRA_CLUSTER_RADIUS: f32 = 12.0;

/// `fossil_runtime::layout::cluster_layout`, verbatim in behaviour: clusters on
/// a Z-order grid, phyllotaxis within the cell, uniform pitch sized by the
/// largest disc.
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

/// Inverse of [`morton2`] — the grid cell a cluster id occupies.
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

/// Interleave the low 16 bits of `x` and `y` — `fossil_runtime::layout::morton2`.
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

/// `k` equal clusters over `n` vertices, placed and then renumbered into Morton
/// order — which is what `enrich_layout` does, minus the community detection
/// that decides the cluster ids.
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

    // The new `dense_id` is the Morton rank, so emit the rows in that order.
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

/// The drawing schema — `dense_id`, `x`, `y`, `cluster_id` (ADR-0042 §3.5) —
/// or that plus `subject` and `community`, the six columns ADR-0042 §3.4
/// weighed.
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

/// One `RecordBatch` over rows `[lo, hi)` of the corpus.
fn batch(c: &Corpus, lo: usize, hi: usize) -> RecordBatch {
    let mut cols: Vec<datafusion::arrow::array::ArrayRef> = vec![
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
        // `community` is the coarse level of the hierarchy — eight groups, the
        // shape ADR-0042 §3 measured at five million.
        cols.push(Arc::new(UInt32Array::from_iter_values(
            c.cluster[lo..hi].iter().map(|c| c % 8),
        )));
    }
    RecordBatch::try_new(schema(c.full), cols).expect("corpus columns are the schema")
}

// ──────────────────────────────────────────────────────────────────────────
// The two writers
// ──────────────────────────────────────────────────────────────────────────

/// Layout A — one file per tile, through `fossil_df::files::batches_to_parquet`,
/// which is the encoder that ships. Returns the file paths in tile order.
fn write_per_tile(dir: &Path, c: &Corpus, n: usize) -> Vec<PathBuf> {
    fs::create_dir_all(dir).expect("mkdir tiles");
    let tiles = n.div_ceil(TILE_ROWS);
    let mut paths = Vec::with_capacity(tiles);
    for t in 0..tiles {
        let lo = t * TILE_ROWS;
        let hi = ((t + 1) * TILE_ROWS).min(n);
        let bytes = batches_to_parquet(&[batch(c, lo, hi)])
            .expect("encode tile")
            .expect("a tile is never empty");
        let path = dir.join(format!("chunk{t}.parquet"));
        fs::write(&path, bytes).expect("write tile");
        paths.push(path);
    }
    paths
}

/// Layout B — one file, row groups of [`TILE_ROWS`]. This encoder does not
/// exist in the tree; it is `batches_to_parquet` with the row-group size set,
/// which is the whole of ADR-0046 §9's proposal §2. `dictionary` is the
/// `parquet` default (on) unless a caller says otherwise — it is a parameter
/// because on these four columns the default is not free, and §1 shows what it
/// costs.
fn write_single(path: &Path, c: &Corpus, n: usize, dictionary: Dict) {
    let mut b = WriterProperties::builder().set_max_row_group_row_count(Some(TILE_ROWS));
    b = match dictionary {
        Dict::Default => b,
        Dict::Off => b.set_dictionary_enabled(false),
        // Off where the column is near-unique inside a tile, on where it is a
        // handful of repeated values. This is the choice DuckDB makes per
        // column and `parquet` does not.
        Dict::PerColumn => ["dense_id", "x", "y"].into_iter().fold(b, |b, c| {
            b.set_column_dictionary_enabled(ColumnPath::from(c), false)
        }),
    };
    let props = b.build();
    let file = fs::File::create(path).expect("create single");
    let mut w = ArrowWriter::try_new(file, schema(c.full), Some(props)).expect("writer");
    // Feed it a tile at a time so the row-group boundaries are exact rather
    // than a consequence of how big the batches happened to be.
    for t in 0..n.div_ceil(TILE_ROWS) {
        let lo = t * TILE_ROWS;
        let hi = ((t + 1) * TILE_ROWS).min(n);
        w.write(&batch(c, lo, hi)).expect("write row group");
    }
    w.close().expect("close single");
}

// ──────────────────────────────────────────────────────────────────────────
// Reading the bytes back
// ──────────────────────────────────────────────────────────────────────────

/// What one file's metadata costs, and what the reader can learn from it.
struct FileFacts {
    file_bytes: u64,
    /// Thrift `FileMetaData` plus the 4-byte length and the `PAR1` trailer —
    /// what ADR-0046 calls "the footer".
    footer_bytes: u64,
    /// `ColumnIndex` + `OffsetIndex`, which sit before the footer and are what
    /// "arrow-rs writes the page index by default" is a claim about.
    page_index_bytes: u64,
    page_index_present: bool,
    row_groups: usize,
    /// Per row group: `(xmin, xmax, ymin, ymax, compressed bytes)`.
    boxes: Vec<(f32, f32, f32, f32, u64)>,
    /// Stored bytes by column name — where the difference between two encoders
    /// actually lives.
    columns: BTreeMap<String, u64>,
}

fn read_facts(path: &Path) -> FileFacts {
    let file = fs::File::open(path).expect("open");
    let file_bytes = file.metadata().expect("stat").len();
    let meta: ParquetMetaData = ParquetMetaDataReader::new()
        .with_page_index_policy(PageIndexPolicy::Optional)
        .parse_and_finish(&file)
        .expect("parse metadata");

    // The footer length is the 4 bytes before `PAR1`; the blob itself is that
    // many bytes further back. Read it off the file rather than re-serialising.
    let tail = read_tail(path, 8);
    let meta_len = u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]) as u64;
    let footer_bytes = meta_len + 8;

    let mut page_index_bytes = 0u64;
    let mut columns: BTreeMap<String, u64> = BTreeMap::new();
    let mut boxes = Vec::with_capacity(meta.num_row_groups());
    for rg in meta.row_groups() {
        let (mut xmin, mut xmax, mut ymin, mut ymax) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for col in rg.columns() {
            page_index_bytes += col.column_index_length().unwrap_or(0).max(0) as u64;
            page_index_bytes += col.offset_index_length().unwrap_or(0).max(0) as u64;
            let name = col.column_descr().name().to_string();
            *columns.entry(name.clone()).or_default() += col.compressed_size().max(0) as u64;
            if let Some(Statistics::Float(v)) = col.statistics() {
                let (lo, hi) = (v.min_opt().copied(), v.max_opt().copied());
                if let (Some(lo), Some(hi)) = (lo, hi) {
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
        boxes.push((xmin, xmax, ymin, ymax, rg.compressed_size().max(0) as u64));
    }
    FileFacts {
        file_bytes,
        footer_bytes,
        page_index_bytes,
        page_index_present: meta.column_index().is_some() && page_index_bytes > 0,
        row_groups: meta.num_row_groups(),
        boxes,
        columns,
    }
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

// ──────────────────────────────────────────────────────────────────────────
// The windows
// ──────────────────────────────────────────────────────────────────────────

/// A square window, and what it costs under each layout.
struct WindowCost {
    inside: usize,
    needed: usize,
    selected: usize,
    /// Maximal runs of consecutive selected units — one HTTP range request each
    /// inside one file; meaningless across files, where every unit is its own.
    runs: usize,
    bytes_needed: u64,
    bytes_selected: u64,
}

/// Half-width that puts about [`WINDOW_VERTICES`] vertices in a square centred
/// on `(cx, cy)` — binary search, because the density is not uniform.
fn size_window(c: &Corpus, cx: f32, cy: f32, span: f32) -> f32 {
    let (mut lo, mut hi) = (span * 1e-4, span);
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if count_inside(c, cx, cy, mid) < WINDOW_VERTICES {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    hi
}

fn count_inside(c: &Corpus, cx: f32, cy: f32, h: f32) -> usize {
    let (x0, x1, y0, y1) = (cx - h, cx + h, cy - h, cy + h);
    (0..c.x.len())
        .filter(|&i| c.x[i] >= x0 && c.x[i] <= x1 && c.y[i] >= y0 && c.y[i] <= y1)
        .count()
}

/// Cost of one window over a unit list — `boxes[u]` is unit `u`'s `x`/`y` box
/// and its byte size, and unit `u` holds rows `[u·4096, (u+1)·4096)`.
fn window_cost(
    c: &Corpus,
    boxes: &[(f32, f32, f32, f32, u64)],
    cx: f32,
    cy: f32,
    h: f32,
) -> WindowCost {
    let (x0, x1, y0, y1) = (cx - h, cx + h, cy - h, cy + h);
    let mut needed = vec![false; boxes.len()];
    let mut inside = 0usize;
    for i in 0..c.x.len() {
        if c.x[i] >= x0 && c.x[i] <= x1 && c.y[i] >= y0 && c.y[i] <= y1 {
            inside += 1;
            needed[i / TILE_ROWS] = true;
        }
    }
    let mut selected = vec![false; boxes.len()];
    for (u, b) in boxes.iter().enumerate() {
        selected[u] = b.0 <= x1 && b.1 >= x0 && b.2 <= y1 && b.3 >= y0;
    }
    // Every needed unit must be selected, or the boxes are lying.
    for u in 0..boxes.len() {
        assert!(
            !needed[u] || selected[u],
            "unit {u} needed but not selected"
        );
    }

    let mut runs = 0usize;
    let mut prev = false;
    for &s in &selected {
        if s && !prev {
            runs += 1;
        }
        prev = s;
    }
    WindowCost {
        inside,
        needed: needed.iter().filter(|&&b| b).count(),
        selected: selected.iter().filter(|&&b| b).count(),
        runs,
        bytes_needed: (0..boxes.len())
            .filter(|&u| needed[u])
            .map(|u| boxes[u].4)
            .sum(),
        bytes_selected: (0..boxes.len())
            .filter(|&u| selected[u])
            .map(|u| boxes[u].4)
            .sum(),
    }
}

// ──────────────────────────────────────────────────────────────────────────
// A layout is a set of files and a rule for what one request can fetch
// ──────────────────────────────────────────────────────────────────────────

/// One way of laying 5M vertices out on an origin, and everything a reader can
/// learn from it without downloading a page.
struct Layout {
    name: String,
    /// Files in tile order. A one-file layout has one entry.
    files: Vec<FileFacts>,
    /// Tiles are separate files, so a file boundary is a request boundary and
    /// consecutive tiles cannot be coalesced into one range request.
    per_file: bool,
    /// Per tile: its `x`/`y` box and the bytes one request for it costs.
    units: Vec<(f32, f32, f32, f32, u64)>,
}

impl Layout {
    /// `per_file`: one row group per file, and the request cost is the whole
    /// file — a reader that wants the data must read the footer first, so
    /// fetching 60 kB in one request is cheaper than two ranges.
    fn new(name: String, files: Vec<FileFacts>, per_file: bool) -> Self {
        let units = if per_file {
            files
                .iter()
                .map(|f| {
                    assert_eq!(f.row_groups, 1, "{name}: a tile file is not one row group");
                    let b = f.boxes[0];
                    (b.0, b.1, b.2, b.3, f.file_bytes)
                })
                .collect()
        } else {
            files[0].boxes.clone()
        };
        Self {
            name,
            files,
            per_file,
            units,
        }
    }

    fn footer_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.footer_bytes).sum()
    }
    fn page_index_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.page_index_bytes).sum()
    }
    fn stored_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.file_bytes).sum()
    }
    fn page_index_present(&self) -> bool {
        self.files.iter().all(|f| f.page_index_present)
    }
    fn requests(&self, w: &WindowCost) -> usize {
        if self.per_file { w.selected } else { w.runs }
    }
}

/// Every `*.parquet` under a `PARTITION_BY` tree, ordered by the `tile=<k>` key
/// so unit `k` is tile `k` — `read_dir` order is not that.
fn duck_tile_paths(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut keyed: Vec<(u64, PathBuf)> = Vec::new();
    for e in entries.flatten() {
        let dir = e.path();
        let Some(k) = dir
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_prefix("tile="))
            .and_then(|s| s.parse::<u64>().ok())
        else {
            continue;
        };
        if let Ok(inner) = fs::read_dir(&dir) {
            for f in inner.flatten() {
                if f.path().extension().is_some_and(|x| x == "parquet") {
                    keyed.push((k, f.path()));
                }
            }
        }
    }
    keyed.sort_unstable_by_key(|(k, _)| *k);
    keyed.into_iter().map(|(_, p)| p).collect()
}

fn arg(name: &str, default: &str) -> String {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| default.to_string())
}

fn main() {
    let n: usize = arg("--rows", "5000000")
        .parse()
        .expect("--rows is a number");
    let full = arg("--schema", "draw") == "full";
    let k: u32 = arg("--clusters", "2048")
        .parse()
        .expect("--clusters is a number");
    let dir = PathBuf::from(arg("--dir", "/tmp/f7-tile-layout"));
    fs::create_dir_all(&dir).expect("mkdir workdir");
    let tiles = n.div_ceil(TILE_ROWS);

    println!("# F7 · what a camera window costs, by Parquet layout\n");
    println!(
        "{n} vertices, {k} clusters, tile = {TILE_ROWS} rows, {tiles} tiles. Uncompressed pages; \
         schema: {}.",
        if full {
            "`dense_id`, `x`, `y`, `cluster_id`, `subject`, `community` (six columns)"
        } else {
            "`dense_id`, `x`, `y`, `cluster_id` (the drawing tile)"
        }
    );

    let t = Instant::now();
    let corpus = build_corpus(n, k, full);
    let corpus_secs = t.elapsed().as_secs_f64();

    // ── Write, then read every byte of metadata back off the disk ─────────
    let tiles_dir = dir.join("tiles");
    let t = Instant::now();
    let tile_paths = write_per_tile(&tiles_dir, &corpus, n);
    let a_secs = t.elapsed().as_secs_f64();

    let single = dir.join("single.parquet");
    let t = Instant::now();
    write_single(&single, &corpus, n, Dict::Default);
    let b_secs = t.elapsed().as_secs_f64();

    // The same layout with `parquet`'s dictionary encoding off. It is a layout
    // and not a footnote because §1 measures the default costing more than
    // PLAIN on three of these four columns.
    let single_plain = dir.join("single_plain.parquet");
    write_single(&single_plain, &corpus, n, Dict::Off);
    let single_tuned = dir.join("single_tuned.parquet");
    write_single(&single_tuned, &corpus, n, Dict::PerColumn);

    // The corpus as one file, so the DuckDB comparison reads the same rows in
    // the same order. Left alone once written — re-running must not invalidate
    // the DuckDB output sitting beside it.
    let corpus_path = dir.join("corpus.parquet");
    if !corpus_path.exists() {
        write_single(&corpus_path, &corpus, n, Dict::Default);
    }

    let mut layouts = vec![
        Layout::new(
            "A · arrow-rs, one file per tile".to_string(),
            tile_paths.iter().map(|p| read_facts(p)).collect(),
            true,
        ),
        Layout::new(
            format!("B · arrow-rs, one file, {TILE_ROWS}-row row groups"),
            vec![read_facts(&single)],
            false,
        ),
        Layout::new(
            "C · arrow-rs, one file, dictionary off".to_string(),
            vec![read_facts(&single_plain)],
            false,
        ),
        Layout::new(
            "D · arrow-rs, one file, dictionary per column".to_string(),
            vec![read_facts(&single_tuned)],
            false,
        ),
    ];
    let duck_single = dir.join("duck_single.parquet");
    if duck_single.exists() {
        layouts.push(Layout::new(
            format!("B′ · DuckDB COPY, one file, ROW_GROUP_SIZE {TILE_ROWS}"),
            vec![read_facts(&duck_single)],
            false,
        ));
    }
    let duck_tiles = duck_tile_paths(&dir.join("duck_tiles"));
    if !duck_tiles.is_empty() {
        layouts.push(Layout::new(
            "A′ · DuckDB COPY, PARTITION_BY tile".to_string(),
            duck_tiles.iter().map(|p| read_facts(p)).collect(),
            true,
        ));
    }
    println!(
        "\nCorpus built in {corpus_secs:.1} s; encoded in {a_secs:.1} s (A) and {b_secs:.1} s (B). \
         The DuckDB rows are the same rows — it re-encodes `corpus.parquet`, it does not \
         regenerate anything."
    );

    // ── 1 · Footers ───────────────────────────────────────────────────────
    println!("\n## 1 · Footer bytes\n");
    println!(
        "`footer` is the thrift `FileMetaData` plus the 4-byte length and `PAR1`. The page index \
         is not inside it — it is a separate block, and it is counted separately.\n"
    );
    println!(
        "| layout | files | row groups | footer total | per tile | page index | per tile | stored |"
    );
    println!("|---|---|---|---|---|---|---|---|");
    for l in &layouts {
        let rgs: usize = l.files.iter().map(|f| f.row_groups).sum();
        println!(
            "| {} | {} | {} | {} B | {} B | {} B | {} B | {} |",
            l.name,
            l.files.len(),
            rgs,
            l.footer_bytes(),
            l.footer_bytes() / rgs as u64,
            l.page_index_bytes(),
            l.page_index_bytes() / rgs as u64,
            mb(l.stored_bytes()),
        );
    }
    println!("\nStored bytes by column — where an encoder choice actually shows up:\n");
    let names: Vec<String> = layouts[1].files[0].columns.keys().cloned().collect();
    println!("| layout | {} |", names.join(" | "));
    println!("|---{}|", "|---".repeat(names.len()));
    for l in &layouts {
        let cells: Vec<String> = names
            .iter()
            .map(|k| {
                mb(l.files
                    .iter()
                    .map(|f| f.columns.get(k).copied().unwrap_or(0))
                    .sum())
            })
            .collect();
        println!("| {} | {} |", l.name, cells.join(" | "));
    }

    let rg_b = layouts[1].files[0].row_groups;
    println!(
        "\nA row group per tile: {}. Footer, A over B: {:.2}×. The metadata a reader must hold to \
         address every tile at all — every footer plus every page index — is {} for A and {} for B.",
        if rg_b == tiles {
            format!("confirmed ({rg_b} row groups, {tiles} tiles)")
        } else {
            format!("CONTRADICTED ({rg_b} row groups, {tiles} tiles)")
        },
        layouts[0].footer_bytes() as f64 / layouts[1].footer_bytes() as f64,
        mb(layouts[0].footer_bytes() + layouts[0].page_index_bytes()),
        mb(layouts[1].footer_bytes() + layouts[1].page_index_bytes()),
    );

    // ── 4 · The page index ────────────────────────────────────────────────
    println!("\n## 4 · Is the page index written?\n");
    println!(
        "Read back off the bytes with `PageIndexPolicy::Optional`, which returns what is there.\n"
    );
    println!("| writer | `ColumnIndex` + `OffsetIndex` present | bytes |");
    println!("|---|---|---|");
    for l in &layouts {
        println!(
            "| {} | **{}** | {} |",
            l.name,
            yesno(l.page_index_present()),
            l.page_index_bytes()
        );
    }
    println!(
        "\n`parquet` crate: {PARQUET_PIN}. DuckDB rows are absent unless the CLI step above was run."
    );

    // ── 2 & 3 · Windows ───────────────────────────────────────────────────
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for i in 0..n {
        min_x = min_x.min(corpus.x[i]);
        min_y = min_y.min(corpus.y[i]);
        max_x = max_x.max(corpus.x[i]);
        max_y = max_y.max(corpus.y[i]);
    }
    let span = (max_x - min_x).max(max_y - min_y);
    // Nine centres spread through `dense_id`, which is Morton order, so they
    // are spread over the canvas and every one lands on a vertex. A literal pan
    // — one centre, nine steps of a half-width — walks off the edge of a corpus
    // this shape and measures three empty windows, which is what the first run
    // of this example did.
    let centres: Vec<(f32, f32)> = (0..WINDOWS)
        .map(|w| {
            let i = (w + 1) * n / (WINDOWS + 1);
            (corpus.x[i], corpus.y[i])
        })
        .collect();
    let (cx0, cy0) = centres[WINDOWS / 2];
    let h = size_window(&corpus, cx0, cy0, span);

    println!("\n## 2 & 3 · Requests and bytes per window\n");
    println!(
        "Nine squares of half-width {h:.0} on a canvas {span:.0} across — one per ninth of \
         `dense_id`, sized on the middle one to hold {WINDOW_VERTICES} vertices. `needed` is the \
         tiles holding a drawn vertex; `selected` is what the `x`/`y` boxes admit, which is all a \
         reader can prune to. A request is one file for a per-file layout and one maximal run of \
         consecutive row groups otherwise.\n"
    );

    // The needed/selected/overread columns are a property of the *tiling*, not
    // of the encoder, so they are reported once — off layout B, and asserted
    // equal on every other layout below.
    let costs: Vec<WindowCost> = centres
        .iter()
        .map(|&(cx, cy)| window_cost(&corpus, &layouts[1].units, cx, cy, h))
        .collect();
    println!("| # | drawn | needed | selected | overread |");
    println!("|---|---|---|---|---|");
    for (w, c) in costs.iter().enumerate() {
        println!(
            "| {} | {} | {} | {} | {:.2}× |",
            w + 1,
            c.inside,
            c.needed,
            c.selected,
            c.selected as f64 / c.needed.max(1) as f64
        );
    }
    let sum_needed: usize = costs.iter().map(|c| c.needed).sum();
    let sum_selected: usize = costs.iter().map(|c| c.selected).sum();
    println!(
        "\nSelected {}–{} tiles where {}–{} were needed; overread {:.2}×–{:.2}×, mean {:.2}×.",
        costs.iter().map(|c| c.selected).min().unwrap_or(0),
        costs.iter().map(|c| c.selected).max().unwrap_or(0),
        costs.iter().map(|c| c.needed).min().unwrap_or(0),
        costs.iter().map(|c| c.needed).max().unwrap_or(0),
        costs
            .iter()
            .map(|c| c.selected as f64 / c.needed.max(1) as f64)
            .fold(f64::MAX, f64::min),
        costs
            .iter()
            .map(|c| c.selected as f64 / c.needed.max(1) as f64)
            .fold(0.0, f64::max),
        sum_selected as f64 / sum_needed as f64,
    );

    println!(
        "\n| layout | requests/window | bytes/window | bytes if only the needed tiles | index, once |"
    );
    println!("|---|---|---|---|---|");
    for l in &layouts {
        let mut req = 0usize;
        let mut sel = 0u64;
        let mut need = 0u64;
        for (wi, &(cx, cy)) in centres.iter().enumerate() {
            let c = window_cost(&corpus, &l.units, cx, cy, h);
            assert_eq!(
                (c.needed, c.selected),
                (costs[wi].needed, costs[wi].selected),
                "{}: window {} selects a different tile set than layout B",
                l.name,
                wi + 1
            );
            req += l.requests(&c);
            sel += c.bytes_selected;
            need += c.bytes_needed;
        }
        let w = WINDOWS as u64;
        println!(
            "| {} | {:.1} | {} | {} | {} |",
            l.name,
            req as f64 / w as f64,
            mb(sel / w),
            mb(need / w),
            if l.per_file {
                format!(
                    "{} ({} footers)",
                    mb(l.footer_bytes() + l.page_index_bytes()),
                    l.files.len()
                )
            } else {
                format!("{} (1 footer)", mb(l.footer_bytes() + l.page_index_bytes()))
            },
        );
    }
    println!(
        "\nThe last column is the cost of *having* the boxes at all, which the per-window columns \
         exclude. A one-file layout pays it in one request and keeps it; a per-file layout has no \
         such file — the equivalent is a footer read per tile, or a sidecar index that nothing in \
         the tree writes today."
    );
}

fn mb(bytes: u64) -> String {
    if bytes < 1_000_000 {
        format!("{:.1} kB", bytes as f64 / 1_000.0)
    } else {
        format!("{:.2} MB", bytes as f64 / 1_000_000.0)
    }
}

fn yesno(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}
