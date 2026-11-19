//! Proof of concept: a graph larger than the engine's budget, written as Apache GraphAr in K
//! range partitions — the engine partitions, the writer only writes.
//!
//! - **Ids** are GraphAr Spark's `IndexGenerator`: a count per partition, exclusive prefix sums,
//!   and `id = offset + rank` within the partition sorted by key. Partitions are key ranges, so
//!   the ids are the ones one global `ORDER BY key` gives, for any K.
//! - **Edges** are scattered once by the destination's partition and, after `dst → id`, once by
//!   the source's (a Grace hash join's partition phase, here by range). Each pass reads every edge
//!   once; a partition is then joined and sorted on its own, under the budget.
//! - **The writer is clustered**: partitions are emitted in key order, so the edges reach
//!   `graphar_parquet::EdgeWriter` already in `(src, dst)` order across partitions, and it writes
//!   each chunk as soon as it is full.
//! - **K = 1 is the same code with no spill**: every partition stays in memory.
//!
//! The spill goes through the host ([`Io`]): in the browser, OPFS files the page writes and reads.
//! The sources are generated, so nothing but the run's own state is held.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use datafusion::arrow::array::{
    Array, ArrayRef, AsArray, Int64Array, RecordBatch, StringArray, UInt32Array,
};
use datafusion::arrow::buffer::Buffer;
use datafusion::arrow::compute::{cast, take_record_batch};
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::arrow::ipc::reader::StreamDecoder;
use datafusion::arrow::ipc::writer::StreamWriter;
use datafusion::common::JoinType;
use datafusion::prelude::{SessionContext, col, ident};
use fossil_exec::memory::Budget;
use fossil_exec::session::session_within;
use futures::StreamExt;
use graphar_info::{
    AdjListType, AdjacentList, Cardinality, DataType as Gar, EdgeInfo, FileType, GraphInfo,
    InfoVersion, Property, PropertyGroup, VertexInfo,
};
use graphar_parquet::{DST_INDEX_COL, EdgeWriter, Options, SRC_INDEX_COL, VertexWriter};
use object_store::memory::InMemory;
use object_store::path::Path;
use object_store::{ObjectStore, ObjectStoreExt};
use serde::Serialize;
use web_time::Instant;

pub type Error = Box<dyn std::error::Error>;
type Result<T> = std::result::Result<T, Error>;

/// What a run is asked to do.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Params {
    pub n: u64,
    pub m: u64,
    /// The number of partitions; `None` chooses it from the budget.
    pub k: Option<usize>,
    /// The engine's pool, in MiB.
    pub budget_mb: usize,
    pub vertex_chunk: u64,
    pub edge_chunk: u64,
}

/// What a run answers.
#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub k: usize,
    pub estimated_mb: u64,
    pub phases: Vec<(String, f64)>,
    pub vertices: u64,
    pub edges: u64,
    /// Order-independent sums over `(id, key)` and over `(src, dst)`.
    pub vertex_print: String,
    pub edge_print: String,
    pub spilled_mb: u64,
    pub written_mb: u64,
    pub files: u64,
}

/// Where the spill and the graph go: the host. `Memory` keeps both (the native tests);
/// `Js` hands them to the page.
pub enum Io {
    Memory(RefCell<HashMap<String, Vec<u8>>>),
    Js {
        append: js_sys::Function,
        read: js_sys::Function,
        output: js_sys::Function,
    },
}

impl Io {
    async fn call(
        f: &js_sys::Function,
        a: &str,
        b: Option<&[u8]>,
    ) -> Result<wasm_bindgen::JsValue> {
        let a = wasm_bindgen::JsValue::from_str(a);
        let promise = match b {
            Some(b) => f.call2(
                &wasm_bindgen::JsValue::NULL,
                &a,
                &js_sys::Uint8Array::from(b),
            ),
            None => f.call1(&wasm_bindgen::JsValue::NULL, &a),
        }
        .map_err(|e| format!("{e:?}"))?;
        wasm_bindgen_futures::JsFuture::from(js_sys::Promise::from(promise))
            .await
            .map_err(|e| format!("{e:?}").into())
    }

    async fn append(&self, name: &str, bytes: &[u8]) -> Result<()> {
        match self {
            Self::Memory(m) => m
                .borrow_mut()
                .entry(name.to_owned())
                .or_default()
                .extend_from_slice(bytes),
            Self::Js { append, .. } => {
                Self::call(append, name, Some(bytes)).await?;
            }
        }
        Ok(())
    }

    async fn read(&self, name: &str) -> Result<Vec<u8>> {
        match self {
            Self::Memory(m) => Ok(m.borrow_mut().remove(name).unwrap_or_default()),
            Self::Js { read, .. } => {
                Ok(js_sys::Uint8Array::new(&Self::call(read, name, None).await?).to_vec())
            }
        }
    }

    async fn output(&self, path: &str, bytes: &[u8]) -> Result<()> {
        match self {
            Self::Memory(m) => {
                m.borrow_mut().insert(format!("out:{path}"), bytes.to_vec());
            }
            Self::Js { output, .. } => {
                Self::call(output, path, Some(bytes)).await?;
            }
        }
        Ok(())
    }
}

/// The graph's files as the writers put them, handed to the host and dropped as they come.
struct Outbox {
    store: InMemory,
    bytes: Cell<u64>,
    files: Cell<u64>,
}

impl Outbox {
    async fn drain(&self, io: &Io) -> Result<()> {
        let listed: Vec<_> = self.store.list(None).collect().await;
        for meta in listed {
            let path = meta?.location;
            let bytes = self.store.get(&path).await?.bytes().await?;
            io.output(path.as_ref(), &bytes).await?;
            self.store.delete(&path).await?;
            self.bytes.set(self.bytes.get() + bytes.len() as u64);
            self.files.set(self.files.get() + 1);
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// The sources, generated: the same IRIs as E13 (`http://example.org/person/` || md5(i)).
// ---------------------------------------------------------------------------------------------

const BATCH: u64 = 65_536;

fn key(i: u64) -> String {
    format!(
        "http://example.org/person/{:x}",
        md5::compute(i.to_string())
    )
}

fn vertex_batch(lo: u64, hi: u64) -> RecordBatch {
    let keys: StringArray = (lo..hi).map(|i| Some(key(i))).collect();
    let names: StringArray = (lo..hi).map(|i| Some(format!("name {i}"))).collect();
    RecordBatch::try_from_iter([
        ("key", Arc::new(keys) as ArrayRef),
        ("name", Arc::new(names)),
    ])
    .unwrap()
}

fn edge_batch(lo: u64, hi: u64, n: u64) -> RecordBatch {
    let src: StringArray = (lo..hi)
        .map(|j| Some(key((j.wrapping_mul(2_654_435_761) + 12_345) % n)))
        .collect();
    let dst: StringArray = (lo..hi)
        .map(|j| Some(key((j.wrapping_mul(40_503) + 7) % n)))
        .collect();
    RecordBatch::try_from_iter([("src", Arc::new(src) as ArrayRef), ("dst", Arc::new(dst))])
        .unwrap()
}

fn batches(count: u64) -> impl Iterator<Item = (u64, u64)> {
    (0..count.div_ceil(BATCH)).map(move |b| (b * BATCH, ((b + 1) * BATCH).min(count)))
}

// ---------------------------------------------------------------------------------------------
// The scatter: a stream in, K partitions out, each in memory (K = 1) or spilled as Arrow IPC.
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Default)]
struct Buf(Arc<Mutex<Vec<u8>>>);
impl std::io::Write for Buf {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

enum Part {
    Memory(Vec<RecordBatch>),
    Spill {
        name: String,
        writer: Option<StreamWriter<Buf>>,
        buf: Buf,
    },
}

/// A partition after its scatter: what [`load`] reads back.
enum Done {
    Memory(Vec<RecordBatch>),
    Spilled(String),
    Empty,
}

const FLUSH: usize = 1 << 20;

struct Scatter {
    key: &'static str,
    bounds: Arc<Vec<String>>,
    parts: Vec<Part>,
    rows: Vec<u64>,
}

impl Scatter {
    fn new(prefix: &str, key: &'static str, bounds: Arc<Vec<String>>) -> Self {
        let k = bounds.len() + 1;
        let parts = (0..k)
            .map(|b| {
                if k == 1 {
                    Part::Memory(Vec::new())
                } else {
                    Part::Spill {
                        name: format!("{prefix}{b}.arrow"),
                        writer: None,
                        buf: Buf::default(),
                    }
                }
            })
            .collect();
        Self {
            key,
            bounds,
            parts,
            rows: vec![0; k],
        }
    }

    async fn push(&mut self, batch: &RecordBatch, io: &Io, spilled: &Cell<u64>) -> Result<()> {
        if batch.num_rows() == 0 {
            return Ok(());
        }
        let k = self.parts.len();
        if k == 1 {
            self.rows[0] += batch.num_rows() as u64;
            if let Part::Memory(v) = &mut self.parts[0] {
                v.push(batch.clone());
            }
            return Ok(());
        }
        let keys = batch.column_by_name(self.key).ok_or("no key column")?;
        let keys = cast(keys, &DataType::Utf8)?;
        let keys = keys.as_string::<i32>();
        let mut index: Vec<Vec<u32>> = vec![Vec::new(); k];
        for i in 0..keys.len() {
            let v = keys.value(i).as_bytes();
            let b = self.bounds.partition_point(|x| x.as_bytes() <= v);
            index[b].push(i as u32);
        }
        for (b, rows) in index.into_iter().enumerate() {
            if rows.is_empty() {
                continue;
            }
            self.rows[b] += rows.len() as u64;
            let part = take_record_batch(batch, &UInt32Array::from(rows))?;
            if let Part::Spill { name, writer, buf } = &mut self.parts[b] {
                if writer.is_none() {
                    *writer = Some(StreamWriter::try_new(buf.clone(), &part.schema())?);
                }
                writer.as_mut().unwrap().write(&part)?;
                let bytes = {
                    let mut v = buf.0.lock().unwrap();
                    if v.len() < FLUSH {
                        continue;
                    }
                    std::mem::take(&mut *v)
                };
                spilled.set(spilled.get() + bytes.len() as u64);
                io.append(name, &bytes).await?;
            }
        }
        Ok(())
    }

    async fn finish(self, io: &Io, spilled: &Cell<u64>) -> Result<(Vec<Done>, Vec<u64>)> {
        let mut done = Vec::new();
        for (part, &rows) in self.parts.into_iter().zip(&self.rows) {
            done.push(match part {
                Part::Memory(v) if rows > 0 => Done::Memory(v),
                Part::Spill {
                    name,
                    writer: Some(mut w),
                    buf,
                } => {
                    w.finish()?;
                    let bytes = std::mem::take(&mut *buf.0.lock().unwrap());
                    spilled.set(spilled.get() + bytes.len() as u64);
                    io.append(&name, &bytes).await?;
                    Done::Spilled(name)
                }
                _ => Done::Empty,
            });
        }
        Ok((done, self.rows))
    }
}

async fn load(done: &mut Done, io: &Io) -> Result<Vec<RecordBatch>> {
    match std::mem::replace(done, Done::Empty) {
        Done::Memory(v) => Ok(v),
        Done::Empty => Ok(Vec::new()),
        Done::Spilled(name) => {
            let mut buffer = Buffer::from_vec(io.read(&name).await?);
            let mut decoder = StreamDecoder::new();
            let mut out = Vec::new();
            while !buffer.is_empty() {
                if let Some(b) = decoder.decode(&mut buffer)? {
                    out.push(b);
                }
            }
            decoder.finish()?;
            Ok(out)
        }
    }
}

async fn collect(
    ctx: &SessionContext,
    df: datafusion::dataframe::DataFrame,
) -> Result<Vec<RecordBatch>> {
    let _ = ctx;
    Ok(df.collect().await?)
}

// ---------------------------------------------------------------------------------------------
// The graph's information: one vertex type, one edge type, chunks bounded.
// ---------------------------------------------------------------------------------------------

fn infos(p: &Params) -> (VertexInfo, EdgeInfo, GraphInfo) {
    let prop = |name: &str, primary| Property {
        name: name.into(),
        data_type: Gar::String,
        is_primary: primary,
        is_nullable: false,
        cardinality: Cardinality::Single,
    };
    let vertex = VertexInfo {
        r#type: "person".into(),
        chunk_size: p.vertex_chunk,
        prefix: "vertex/person/".into(),
        labels: Vec::new(),
        property_groups: vec![PropertyGroup {
            properties: vec![prop("key", true), prop("name", false)],
            file_type: FileType::Parquet,
            prefix: "key_name/".into(),
        }],
        version: InfoVersion::default(),
    };
    let edge = EdgeInfo {
        src_type: "person".into(),
        edge_type: "knows".into(),
        dst_type: "person".into(),
        chunk_size: p.edge_chunk,
        src_chunk_size: p.vertex_chunk,
        dst_chunk_size: p.vertex_chunk,
        directed: true,
        prefix: "edge/person_knows_person/".into(),
        adj_lists: vec![AdjacentList {
            r#type: AdjListType::OrderedBySource,
            file_type: FileType::Parquet,
            prefix: "ordered_by_source/".into(),
        }],
        property_groups: Vec::new(),
        version: InfoVersion::default(),
    };
    let graph = GraphInfo {
        name: "scale".into(),
        prefix: None,
        vertices: vec![vertex.clone()],
        edges: vec![edge.clone()],
        labels: Vec::new(),
        version: InfoVersion::default(),
        extra_info: Vec::new(),
    };
    (vertex, edge, graph)
}

fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn string_print(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    })
}

/// The number of partitions the budget asks for: the largest step holds one partition's edges
/// twice (the scattered input and the join's output) and its vertices' hash table, and the
/// spill buffers sit beside the pool, so a partition is sized to a third of it.
fn partitions(p: &Params, key_bytes: u64) -> (usize, u64) {
    let edge = 2 * (key_bytes + 4) + 16;
    let vertex = key_bytes + 4 + 16 + 24;
    let estimated = p.m * edge * 2 + p.n * vertex * 2;
    let budget = (p.budget_mb as u64) << 20;
    let k = estimated.div_ceil(budget / 3).max(1) as usize;
    (k, estimated >> 20)
}

/// Runs the whole write.
pub async fn run(p: &Params, io: &Io) -> Result<Report> {
    let mut report = Report::default();
    let mut clock = Instant::now();
    let mut phase = |report: &mut Report, name: &str| {
        let now = Instant::now();
        report
            .phases
            .push((name.into(), (now - clock).as_secs_f64()));
        clock = now;
    };
    let spilled = Cell::new(0u64);
    let budget = Arc::new(Budget::new(p.budget_mb << 20));
    let ctx = session_within(budget.clone(), None);
    let (vinfo, einfo, ginfo) = infos(p);
    let options = Options::default();
    let out = Outbox {
        store: InMemory::new(),
        bytes: Cell::new(0),
        files: Cell::new(0),
    };
    let root = Path::from("");

    // 1. K, and the boundaries: a systematic sample of the keys (the sources' statistics would
    //    give the sizes; the sample gives the boundaries).
    let step = (p.n / 100_000).max(1);
    let mut sample: Vec<String> = (0..p.n).step_by(step as usize).map(key).collect();
    sample.sort();
    let key_bytes = sample.iter().map(|s| s.len() as u64).sum::<u64>() / sample.len().max(1) as u64;
    let (auto, estimated) = partitions(p, key_bytes);
    let k = p.k.unwrap_or(auto);
    report.k = k;
    report.estimated_mb = estimated;
    let bounds: Arc<Vec<String>> = Arc::new(
        (1..k)
            .map(|i| sample[sample.len() * i / k].clone())
            .collect(),
    );
    phase(&mut report, "bounds");

    // 2. Vertices scattered by key; counts, then offsets.
    let mut sv = Scatter::new("v", "key", bounds.clone());
    for (lo, hi) in batches(p.n) {
        sv.push(&vertex_batch(lo, hi), io, &spilled).await?;
    }
    let (mut vparts, counts) = sv.finish(io, &spilled).await?;
    let offsets: Vec<u64> = counts
        .iter()
        .scan(0, |acc, &c| {
            let o = *acc;
            *acc += c;
            Some(o)
        })
        .collect();
    phase(&mut report, "vertices_scatter");

    // 3. Each partition sorted by key and numbered from its offset; written in order, and kept
    //    as the partition's map `key -> id` (spilled when K > 1).
    let id_schema: SchemaRef = Arc::new(Schema::new(vec![
        Field::new("key", DataType::Utf8, false),
        Field::new("id", DataType::Int64, false),
    ]));
    let mut maps: Vec<Done> = Vec::new();
    let mut vprint = 0u64;
    {
        let mut writer = VertexWriter::new(&out.store, &root, &vinfo, &options)?;
        for (b, part) in vparts.iter_mut().enumerate() {
            let rows = load(part, io).await?;
            if rows.is_empty() {
                maps.push(Done::Empty);
                continue;
            }
            let sorted = collect(
                &ctx,
                ctx.read_batches(rows)?
                    .sort(vec![col("key").sort(true, false)])?,
            )
            .await?;
            let mut next = offsets[b];
            let mut map = Scatter::new(&format!("m{b}_"), "key", Arc::new(Vec::new()));
            if k > 1 {
                map.parts = vec![Part::Spill {
                    name: format!("m{b}.arrow"),
                    writer: None,
                    buf: Buf::default(),
                }];
            }
            for batch in sorted {
                let batch = RecordBatch::try_new(
                    batch.schema(),
                    vec![
                        cast(batch.column(0), &DataType::Utf8)?,
                        cast(batch.column(1), &DataType::Utf8)?,
                    ],
                )?;
                writer.write(&batch).await?;
                let ids = Int64Array::from_iter_values(
                    (next..next + batch.num_rows() as u64).map(|i| i as i64),
                );
                let keys = batch.column(0).as_string::<i32>();
                for (i, id) in ids.values().iter().enumerate() {
                    vprint = vprint.wrapping_add(mix(*id as u64 ^ string_print(keys.value(i))));
                }
                next += batch.num_rows() as u64;
                let m = RecordBatch::try_new(
                    id_schema.clone(),
                    vec![batch.column(0).clone(), Arc::new(ids)],
                )?;
                if k > 1 {
                    map.push_one(&m, io, &spilled).await?;
                } else if let Part::Memory(v) = &mut map.parts[0] {
                    v.push(m);
                    map.rows[0] += 1;
                }
            }
            maps.push(map.finish(io, &spilled).await?.0.remove(0));
            out.drain(io).await?;
        }
        report.vertices = writer.finish().await?;
        out.drain(io).await?;
    }
    report.vertex_print = format!("{vprint:016x}");
    phase(&mut report, "vertices_number_write");

    // 4. Edges scattered by destination: one pass.
    let mut sd = Scatter::new("d", "dst", bounds.clone());
    for (lo, hi) in batches(p.m) {
        sd.push(&edge_batch(lo, hi, p.n), io, &spilled).await?;
    }
    let (mut dparts, _) = sd.finish(io, &spilled).await?;
    phase(&mut report, "edges_by_dst");

    // 5. Per destination partition, dst -> id, the result scattered by source: one pass more.
    let mut ss = Scatter::new("s", "src", bounds.clone());
    let mut kept: Vec<Vec<RecordBatch>> = Vec::new();
    for b in 0..k {
        let edges = load(&mut dparts[b], io).await?;
        let map = if k > 1 {
            load(&mut maps[b], io).await?
        } else {
            match &maps[b] {
                Done::Memory(v) => v.clone(),
                _ => Vec::new(),
            }
        };
        if k > 1 {
            kept.push(Vec::new());
        } else {
            kept.push(map.clone());
        }
        if edges.is_empty() || map.is_empty() {
            if k > 1 {
                maps[b] = respill(&format!("m{b}r.arrow"), map, io, &spilled).await?;
            }
            continue;
        }
        let joined = ctx
            .read_batches(map.clone())?
            .join(
                ctx.read_batches(edges)?,
                JoinType::Inner,
                &["key"],
                &["dst"],
                None,
            )?
            .select(vec![col("src"), col("id").alias("dst")])?;
        let mut stream = joined.execute_stream().await?;
        while let Some(batch) = stream.next().await {
            ss.push(&batch?, io, &spilled).await?;
        }
        if k > 1 {
            maps[b] = respill(&format!("m{b}r.arrow"), map, io, &spilled).await?;
        }
    }
    let (mut sparts, _) = ss.finish(io, &spilled).await?;
    phase(&mut report, "edges_dst_to_src");

    // 6. Per source partition, in order: src -> id, sorted, to the clustered writer.
    let mut eprint = 0u64;
    {
        let mut writer = EdgeWriter::new(
            &out.store,
            &root,
            &einfo,
            AdjListType::OrderedBySource,
            report.vertices,
            &options,
        )?;
        for b in 0..k {
            let edges = load(&mut sparts[b], io).await?;
            let map = if k > 1 {
                load(&mut maps[b], io).await?
            } else {
                std::mem::take(&mut kept[b])
            };
            if edges.is_empty() || map.is_empty() {
                continue;
            }
            let sorted = ctx
                .read_batches(map)?
                .join(
                    ctx.read_batches(edges)?,
                    JoinType::Inner,
                    &["key"],
                    &["src"],
                    None,
                )?
                .select(vec![
                    col("id").alias(SRC_INDEX_COL),
                    col("dst").alias(DST_INDEX_COL),
                ])?
                .sort(vec![
                    ident(SRC_INDEX_COL).sort(true, false),
                    ident(DST_INDEX_COL).sort(true, false),
                ])?;
            let mut stream = sorted.execute_stream().await?;
            while let Some(batch) = stream.next().await {
                let batch = batch?;
                let (s, d) = (
                    batch
                        .column(0)
                        .as_primitive::<datafusion::arrow::datatypes::Int64Type>(),
                    batch
                        .column(1)
                        .as_primitive::<datafusion::arrow::datatypes::Int64Type>(),
                );
                for i in 0..batch.num_rows() {
                    eprint =
                        eprint.wrapping_add(mix((s.value(i) as u64) << 32 ^ d.value(i) as u64));
                }
                writer.write(&batch).await?;
                out.drain(io).await?;
            }
        }
        report.edges = writer.finish().await?;
        out.drain(io).await?;
    }
    report.edge_print = format!("{eprint:016x}");
    graphar_parquet::write_info(&out.store, &root, &ginfo).await?;
    out.drain(io).await?;
    phase(&mut report, "edges_by_src_write");
    report.spilled_mb = spilled.get() >> 20;
    report.written_mb = out.bytes.get() >> 20;
    report.files = out.files.get();
    let _ = budget;
    Ok(report)
}

/// A map read for one step and needed by the next, spilled again.
async fn respill(
    name: &str,
    batches: Vec<RecordBatch>,
    io: &Io,
    spilled: &Cell<u64>,
) -> Result<Done> {
    if batches.is_empty() {
        return Ok(Done::Empty);
    }
    let mut s = Scatter::new("", "key", Arc::new(Vec::new()));
    s.parts = vec![Part::Spill {
        name: name.into(),
        writer: None,
        buf: Buf::default(),
    }];
    for b in &batches {
        s.push_one(b, io, spilled).await?;
    }
    Ok(s.finish(io, spilled).await?.0.remove(0))
}

impl Scatter {
    /// Appends to the one spilled partition, whatever the key.
    async fn push_one(&mut self, batch: &RecordBatch, io: &Io, spilled: &Cell<u64>) -> Result<()> {
        self.rows[0] += batch.num_rows() as u64;
        if let Part::Spill { name, writer, buf } = &mut self.parts[0] {
            if writer.is_none() {
                *writer = Some(StreamWriter::try_new(buf.clone(), &batch.schema())?);
            }
            writer.as_mut().unwrap().write(batch)?;
            let bytes = {
                let mut v = buf.0.lock().unwrap();
                if v.len() < FLUSH {
                    return Ok(());
                }
                std::mem::take(&mut *v)
            };
            spilled.set(spilled.get() + bytes.len() as u64);
            io.append(name, &bytes).await?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// The page's door.
// ---------------------------------------------------------------------------------------------

#[wasm_bindgen::prelude::wasm_bindgen]
pub async fn write_graph(
    params: String,
    append: js_sys::Function,
    read: js_sys::Function,
    output: js_sys::Function,
) -> std::result::Result<String, wasm_bindgen::JsError> {
    console_error_panic_hook::set_once();
    let p: Params =
        serde_json::from_str(&params).map_err(|e| wasm_bindgen::JsError::new(&e.to_string()))?;
    let io = Io::Js {
        append,
        read,
        output,
    };
    let report = run(&p, &io)
        .await
        .map_err(|e| wasm_bindgen::JsError::new(&e.to_string()))?;
    Ok(serde_json::to_string(&report).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(k: Option<usize>) -> Params {
        Params {
            n: 20_000,
            m: 100_000,
            k,
            budget_mb: 512,
            vertex_chunk: 4096,
            edge_chunk: 16_384,
        }
    }

    #[test]
    fn any_k_writes_the_same_graph() {
        let one =
            futures::executor::block_on(run(&params(Some(1)), &Io::Memory(RefCell::default())))
                .unwrap();
        for k in [2, 7, 16] {
            let io = Io::Memory(RefCell::default());
            let r = futures::executor::block_on(run(&params(Some(k)), &io)).unwrap();
            assert_eq!((r.vertices, r.edges), (20_000, 100_000));
            assert_eq!(
                (&r.vertex_print, &r.edge_print, r.files),
                (&one.vertex_print, &one.edge_print, one.files),
                "k = {k}"
            );
            let Io::Memory(m) = io else { unreachable!() };
            let out: Vec<_> = m
                .borrow()
                .keys()
                .filter(|k| !k.starts_with("out:"))
                .cloned()
                .collect();
            assert!(
                out.is_empty(),
                "every spill is read back and dropped: {out:?}"
            );
        }
    }
}

/// Seconds the sources take to generate alone, to subtract from a run.
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn generate_only(n: f64, m: f64) -> f64 {
    let (n, m) = (n as u64, m as u64);
    let t = Instant::now();
    let mut rows = 0;
    for _ in 0..2 {
        for (lo, hi) in batches(n) {
            rows += vertex_batch(lo, hi).num_rows();
        }
    }
    for (lo, hi) in batches(m) {
        rows += edge_batch(lo, hi, n).num_rows();
    }
    std::hint::black_box(rows);
    t.elapsed().as_secs_f64()
}
