//! Where the layout pass gets its bytes, and where it puts them.
//!
//! The pass itself is arithmetic over Arrow — Louvain, a Morton renumbering, a
//! gather, one sort. **None of it is about files.** What was about files was a
//! handful of `std::fs` helpers at the bottom of [`crate::layout`], and that was
//! the single reason a crate declaring `[package.metadata.fossil] wasm = true`
//! could not actually run in a browser: it *compiled* for `wasm32` and then had
//! nothing to open.
//!
//! So the seam is here, and it is deliberately the narrowest one that works:
//! **open a URL for reading, create a URL for writing, ensure a prefix exists.**
//! Everything above them is unchanged and unaware.
//!
//! # Why a trait and not bytes in, bytes out
//!
//! The obvious alternative — hand the pass a `Vec<(String, Vec<u8>)>` and take
//! one back — was rejected because it makes the *native* path worse to buy the
//! browser path nothing. Natively the vertex read is already the memory peak of
//! the whole write path (`/docs/design/streaming`: 4.80 G at one million
//! vertices and ten million edges, and it is the vertex read and the gather, not
//! the edge sort). A byte-slice interface would force every input file resident
//! before the pass starts and every output file resident until it ends, on a
//! host that has a filesystem and does not need either. [`LocalFs`] streams
//! through `File` exactly as the code did before this module existed, and the
//! browser pays the in-memory cost because in a browser there is nothing else to
//! pay.
//!
//! # Why enum dispatch below the trait
//!
//! `LayoutIo` is a `&dyn` parameter — the pass takes one reference and calls it
//! a few dozen times, so the vtable is free and the alternative is making
//! `enrich_layout` generic over a type parameter that would then appear in every
//! helper signature it threads through. But the *reader* and the *writer* it
//! hands back are enums, not
//! boxes, because `parquet` puts them in hot loops: [`Source`] is what every
//! column chunk is decoded through and [`Sink`] is what every row group is
//! encoded into. That is the split `CLAUDE.md` asks for — `&dyn` at the seam,
//! concrete types under it.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use parquet::errors::{ParquetError, Result as ParquetResult};
use parquet::file::reader::{ChunkReader, Length};

use crate::layout::LayoutError;

/// Strip the one scheme this pass dereferences and refuse the rest.
///
/// Shared by both filesystems on purpose: a URL that names a file to
/// [`LocalFs`] must name the same key to [`MemoryFs`], or the browser and the
/// CLI would disagree about what a target *is* before either of them reads a
/// byte. `file://` is stripped, a bare path passes through, anything carrying
/// another `://` is [`LayoutError::Remote`] — an object store is a registration
/// rather than a string, and neither of these two is one.
pub(crate) fn normalise(url: &str) -> Result<&str, LayoutError> {
    let path = url.strip_prefix("file://").unwrap_or(url);
    if path.contains("://") {
        return Err(LayoutError::Remote {
            url: url.to_string(),
        });
    }
    Ok(path)
}

/// Read a Parquet, write a Parquet, make room for one.
///
/// Implemented twice: [`LocalFs`] for the native host and [`MemoryFs`] for the
/// browser. There is no third, and a third would be an object store, which is
/// a different kind of thing — see [`normalise`].
pub trait LayoutIo {
    /// Open `url` for reading. The result is what `parquet` decodes through.
    ///
    /// # Errors
    /// [`LayoutError::Io`] if it is not there, [`LayoutError::Remote`] if the
    /// URL names a scheme this pass does not dereference.
    fn open(&self, url: &str) -> Result<Source, LayoutError>;

    /// Create `url` for writing, truncating whatever was there.
    ///
    /// # Errors
    /// [`LayoutError::Io`] if it cannot be created, [`LayoutError::Remote`] as
    /// [`Self::open`].
    fn create(&self, url: &str) -> Result<Sink, LayoutError>;

    /// Make `prefix` a place a [`Self::create`] can land.
    ///
    /// # Errors
    /// [`LayoutError::Prefix`] if it cannot be made.
    fn ensure_prefix(&self, prefix: &str) -> Result<(), LayoutError>;
}

// ──────────────────────────────────────────────────────────────────────────
// The reader
// ──────────────────────────────────────────────────────────────────────────

/// A Parquet the pass reads: a file on a disk, or bytes in a map.
///
/// `parquet` wants a [`ChunkReader`], and both halves already are one — `File`
/// and `Bytes` each carry an impl in `parquet` itself. This is the two-way
/// switch that lets one call site accept either, and every method below is a
/// delegation to whichever impl is underneath.
#[derive(Debug)]
pub enum Source {
    /// A file, read with seeks. The decoder pulls the footer and then only the
    /// column chunks it was projected onto, so the file never becomes resident.
    File(File),
    /// Bytes already in memory. Slicing one is a refcount, not a copy, which is
    /// what makes the browser path a single resident copy per file rather than
    /// one per read.
    Memory(Bytes),
}

impl Length for Source {
    fn len(&self) -> u64 {
        match self {
            Self::File(file) => Length::len(file),
            Self::Memory(bytes) => bytes.len() as u64,
        }
    }
}

impl ChunkReader for Source {
    type T = SourceRead;

    fn get_read(&self, start: u64) -> ParquetResult<Self::T> {
        match self {
            Self::File(file) => Ok(SourceRead::File(ChunkReader::get_read(file, start)?)),
            Self::Memory(bytes) => {
                let start = usize::try_from(start).map_err(|_| too_far(start))?;
                if start > bytes.len() {
                    return Err(too_far(start as u64));
                }
                Ok(SourceRead::Memory(Cursor::new(bytes.slice(start..))))
            }
        }
    }

    fn get_bytes(&self, start: u64, length: usize) -> ParquetResult<Bytes> {
        match self {
            Self::File(file) => ChunkReader::get_bytes(file, start, length),
            Self::Memory(bytes) => {
                let start = usize::try_from(start).map_err(|_| too_far(start))?;
                let end = start
                    .checked_add(length)
                    .ok_or_else(|| too_far(start as u64))?;
                if end > bytes.len() {
                    return Err(ParquetError::EOF(format!(
                        "read of {length} bytes at {start} runs past the end of a {}-byte buffer",
                        bytes.len()
                    )));
                }
                Ok(bytes.slice(start..end))
            }
        }
    }
}

fn too_far(start: u64) -> ParquetError {
    ParquetError::EOF(format!("offset {start} is past the end of the buffer"))
}

/// The [`Read`] a [`Source`] hands out. One variant per variant of it.
#[derive(Debug)]
pub enum SourceRead {
    /// A handle on the same file, per `ChunkReader`'s `File::try_clone` model.
    ///
    /// `BufReader` and not `File`, because that is what `parquet`'s own
    /// `impl ChunkReader for File` returns and this variant is a delegation to
    /// it — the buffering is that impl's decision, not one taken here.
    File(std::io::BufReader<File>),
    /// A cursor over a refcounted slice — no copy of the underlying bytes.
    Memory(Cursor<Bytes>),
}

impl Read for SourceRead {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::File(file) => file.read(buf),
            Self::Memory(cursor) => cursor.read(buf),
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// The writer
// ──────────────────────────────────────────────────────────────────────────

/// A Parquet the pass writes: straight to a file, or into a map.
///
/// `TileWriter<W: Write + Send>` is what consumes this, one row group at a
/// time, so a `Sink` is written to incrementally and never holds more than the
/// row group being encoded — on the native side. On the memory side it
/// accumulates, because there is nowhere for it to go until it is finished.
#[derive(Debug)]
pub enum Sink {
    /// Streamed to disk. Peak cost is one row group, as before this module.
    File(File),
    /// Accumulated in a `Vec` and published to the map when it is dropped.
    Memory(MemSink),
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::File(file) => file.write(buf),
            Self::Memory(sink) => sink.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::File(file) => file.flush(),
            Self::Memory(sink) => sink.flush(),
        }
    }
}

/// A `Vec<u8>` that files itself under its URL when it goes out of scope.
///
/// **Publishing on `Drop` and not on an explicit `finish`** because the thing
/// that closes it is `TileWriter::finish`, which consumes the `TileWriter` and
/// drops the sink inside `ArrowWriter::close` — after the footer is written and
/// flushed. There is no seam between "the bytes are complete" and "the sink is
/// dropped" to put a call in, short of changing `fossil-df`'s writer signature
/// for the benefit of one of its two callers.
#[derive(Debug)]
pub struct MemSink {
    url: String,
    buf: Vec<u8>,
    into: Arc<Mutex<BTreeMap<String, Bytes>>>,
}

impl Write for MemSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for MemSink {
    fn drop(&mut self) {
        let bytes = Bytes::from(std::mem::take(&mut self.buf));
        if let Ok(mut map) = self.into.lock() {
            map.insert(std::mem::take(&mut self.url), bytes);
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────
// The two filesystems
// ──────────────────────────────────────────────────────────────────────────

/// `std::fs`. What the native host has always used, unchanged.
#[derive(Debug, Clone, Copy, Default)]
pub struct LocalFs;

impl LocalFs {
    fn path(url: &str) -> Result<PathBuf, LayoutError> {
        normalise(url).map(PathBuf::from)
    }
}

impl LayoutIo for LocalFs {
    fn open(&self, url: &str) -> Result<Source, LayoutError> {
        File::open(Self::path(url)?)
            .map(Source::File)
            .map_err(|source| LayoutError::Io {
                target: url.to_string(),
                source,
            })
    }

    fn create(&self, url: &str) -> Result<Sink, LayoutError> {
        File::create(Self::path(url)?)
            .map(Sink::File)
            .map_err(|source| LayoutError::Io {
                target: url.to_string(),
                source,
            })
    }

    fn ensure_prefix(&self, prefix: &str) -> Result<(), LayoutError> {
        std::fs::create_dir_all(Self::path(prefix)?).map_err(|source| LayoutError::Prefix {
            prefix: prefix.to_string(),
            source,
        })
    }
}

/// A map from URL to bytes. What the browser has instead of a disk.
///
/// Cheap to clone — every clone shares one map — so the caller keeps a handle,
/// hands a reference to the pass, and reads the outputs back out afterwards.
///
/// # What it costs
///
/// One resident copy of every file the pass touches, for as long as the handle
/// lives: the staged vertex and adjacency Parquet the executor produced go in,
/// the tiles come out, and until [`Self::remove`] takes the staged ones away
/// both are held at once. Reads off it are refcounted slices and add nothing.
/// That is strictly more than the native path, which holds one row group, and
/// it is the price of not having a filesystem — see the module header for why
/// the native side is not made to pay it too.
#[derive(Debug, Clone, Default)]
pub struct MemoryFs {
    files: Arc<Mutex<BTreeMap<String, Bytes>>>,
}

impl MemoryFs {
    /// An empty filesystem.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Put `bytes` at `url`, replacing whatever was there.
    ///
    /// # Panics
    /// If the shared map was poisoned by a panic in another holder.
    pub fn insert(&self, url: impl Into<String>, bytes: impl Into<Bytes>) {
        self.files
            .lock()
            .expect("layout memory filesystem poisoned")
            .insert(url.into(), bytes.into());
    }

    /// Drop `url` if it is there. What the caller uses to delete the staged
    /// single-file vertex Parquet, which `exactly-once` fails a corpus for
    /// leaving behind.
    ///
    /// # Panics
    /// If the shared map was poisoned by a panic in another holder.
    pub fn remove(&self, url: &str) {
        self.files
            .lock()
            .expect("layout memory filesystem poisoned")
            .remove(url);
    }

    /// Every file, in URL order, leaving the filesystem empty.
    ///
    /// # Panics
    /// If the shared map was poisoned by a panic in another holder.
    #[must_use]
    pub fn drain(&self) -> Vec<(String, Bytes)> {
        std::mem::take(
            &mut *self
                .files
                .lock()
                .expect("layout memory filesystem poisoned"),
        )
        .into_iter()
        .collect()
    }

    /// Whether `url` is there.
    ///
    /// # Panics
    /// If the shared map was poisoned by a panic in another holder.
    #[must_use]
    pub fn contains(&self, url: &str) -> bool {
        self.files
            .lock()
            .expect("layout memory filesystem poisoned")
            .contains_key(url)
    }
}

impl LayoutIo for MemoryFs {
    fn open(&self, url: &str) -> Result<Source, LayoutError> {
        let key = normalise(url)?;
        self.files
            .lock()
            .expect("layout memory filesystem poisoned")
            .get(key)
            .cloned()
            .map(Source::Memory)
            .ok_or_else(|| LayoutError::Io {
                target: url.to_string(),
                source: std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "not in the layout memory filesystem",
                ),
            })
    }

    fn create(&self, url: &str) -> Result<Sink, LayoutError> {
        Ok(Sink::Memory(MemSink {
            url: normalise(url)?.to_string(),
            buf: Vec::new(),
            into: Arc::clone(&self.files),
        }))
    }

    /// Nothing to do: a map has no directories. It still validates the URL, so
    /// a scheme this pass cannot dereference is refused here rather than at the
    /// first write into it.
    fn ensure_prefix(&self, prefix: &str) -> Result<(), LayoutError> {
        normalise(prefix).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_memory_sink_publishes_when_it_is_dropped() {
        let fs = MemoryFs::new();
        {
            let mut sink = fs.create("vertex/Person/tiles.parquet").unwrap();
            sink.write_all(b"PAR1").unwrap();
        }
        assert!(fs.contains("vertex/Person/tiles.parquet"));
    }

    #[test]
    fn a_memory_source_reads_back_what_was_written() {
        let fs = MemoryFs::new();
        fs.insert("a", Bytes::from_static(b"0123456789"));
        let source = fs.open("a").unwrap();
        assert_eq!(Length::len(&source), 10);
        assert_eq!(source.get_bytes(2, 3).unwrap(), Bytes::from_static(b"234"));
        let mut read = String::new();
        source
            .get_read(7)
            .unwrap()
            .read_to_string(&mut read)
            .unwrap();
        assert_eq!(read, "789");
    }

    #[test]
    fn a_read_past_the_end_is_an_error_and_not_a_panic() {
        let fs = MemoryFs::new();
        fs.insert("a", Bytes::from_static(b"0123"));
        let source = fs.open("a").unwrap();
        assert!(source.get_bytes(2, 99).is_err());
        assert!(source.get_read(99).is_err());
    }

    /// Both filesystems answer the same question about a URL, which is what
    /// lets one set of targets drive either of them.
    #[test]
    fn both_filesystems_refuse_the_same_schemes() {
        let memory = MemoryFs::new();
        for url in ["s3://bucket/vertex.parquet", "https://host/v.parquet"] {
            assert!(matches!(memory.open(url), Err(LayoutError::Remote { .. })));
            assert!(matches!(LocalFs.open(url), Err(LayoutError::Remote { .. })));
        }
        // `file://` is stripped by both, so it reaches a not-found rather than
        // a refusal.
        assert!(matches!(
            memory.open("file:///tmp/nope.parquet"),
            Err(LayoutError::Io { .. })
        ));
    }

    #[test]
    fn draining_empties_the_filesystem() {
        let fs = MemoryFs::new();
        fs.insert("b", Bytes::from_static(b"2"));
        fs.insert("a", Bytes::from_static(b"1"));
        let drained = fs.drain();
        assert_eq!(
            drained.iter().map(|(u, _)| u.as_str()).collect::<Vec<_>>(),
            ["a", "b"],
            "URL order, so a caller's output listing is stable"
        );
        assert!(!fs.contains("a"));
    }
}
