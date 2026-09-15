//! Where the layout pass puts its bytes.
//!
//! The pass itself is arithmetic over Arrow — Louvain, a Morton renumbering, a
//! gather, one sort. **None of it is about files.** What was about files was a
//! handful of `std::fs` helpers at the bottom of [`crate::layout`], and that was
//! the single reason a crate declaring `[package.metadata.fossil] wasm = true`
//! could not actually run in a browser: it *compiled* for `wasm32` and then had
//! nothing to open.
//!
//! So the seam is here, and it is deliberately the narrowest one that works:
//! **create a URL for writing, ensure a prefix exists.** Everything above them
//! is unchanged and unaware.
//!
//! # It had a reader, and the reader is gone
//!
//! `open(url) -> Source` was the third method, and the pass opened the Parquet
//! the writer had staged. The rows arrive as `RecordBatch`es now — see
//! [`crate::layout::VertexLayoutTarget`] — so there is nothing to open, and the
//! `ChunkReader` switch that let one `parquet` call site decode either a `File`
//! or a `Bytes` went with it. **This seam is one-way.**
//!
//! What that removed, besides code: on the browser path a staged file was
//! resident as bytes in this map *and* as the Arrow the executor was still
//! holding, and then again as the decoded copy the pass made of it. Three
//! copies, and the crate said so in this comment.
//!
//! # Why a trait and not bytes out
//!
//! The obvious alternative — have the pass return a `Vec<(String, Vec<u8>)>` —
//! makes the *native* path worse to buy the browser path nothing: it would force
//! every output file resident until the pass ends, on a host that has a
//! filesystem and does not need that. [`LocalFs`] streams a row group at a time
//! through `File`, and the browser pays the in-memory cost because in a browser
//! there is nothing else to pay.
//!
//! # Why enum dispatch below the trait
//!
//! `LayoutIo` is a `&dyn` parameter — the pass takes one reference and calls it
//! a few dozen times, so the vtable is free and the alternative is making
//! `enrich_layout` generic over a type parameter that would then appear in every
//! helper signature it threads through. But the *writer* it hands back is an
//! enum and not a box, because `parquet` puts it in a hot loop: [`Sink`] is what
//! every row group is encoded into. That is the split `CLAUDE.md` asks for —
//! `&dyn` at the seam, concrete types under it.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bytes::Bytes;

use crate::layout::LayoutError;

/// Strip the one scheme this pass dereferences and refuse the rest.
///
/// Shared by both filesystems on purpose: a URL that names a file to
/// [`LocalFs`] must name the same key to [`MemoryFs`], or the browser and the
/// CLI would disagree about what a destination *is* before either of them writes
/// a byte. `file://` is stripped, a bare path passes through, anything carrying
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

/// Write a Parquet, make room for one.
///
/// Implemented twice: [`LocalFs`] for the native host and [`MemoryFs`] for the
/// browser. There is no third, and a third would be an object store, which is
/// a different kind of thing — see [`normalise`].
pub trait LayoutIo {
    /// Create `url` for writing, truncating whatever was there.
    ///
    /// # Errors
    /// [`LayoutError::Io`] if it cannot be created, [`LayoutError::Remote`] if
    /// the URL names a scheme this pass does not dereference.
    fn create(&self, url: &str) -> Result<Sink, LayoutError>;

    /// Make `prefix` a place a [`Self::create`] can land.
    ///
    /// # Errors
    /// [`LayoutError::Prefix`] if it cannot be made.
    fn ensure_prefix(&self, prefix: &str) -> Result<(), LayoutError>;
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
/// One resident copy of every file the pass writes, for as long as the handle
/// lives. That is strictly more than the native path, which holds one row group,
/// and it is the price of not having a filesystem — see the module header for
/// why the native side is not made to pay it too.
///
/// It used to be more than that: the staged vertex and adjacency Parquet went in
/// here too, and were held beside the tiles until the caller removed them. That
/// is not a smaller map now, it is a map holding only outputs.
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

    /// Both filesystems answer the same question about a URL, which is what
    /// lets one set of targets drive either of them.
    #[test]
    fn both_filesystems_refuse_the_same_schemes() {
        let memory = MemoryFs::new();
        for url in ["s3://bucket/vertex/", "https://host/vertex/"] {
            assert!(matches!(
                memory.create(url),
                Err(LayoutError::Remote { .. })
            ));
            assert!(matches!(
                LocalFs.create(url),
                Err(LayoutError::Remote { .. })
            ));
            assert!(matches!(
                memory.ensure_prefix(url),
                Err(LayoutError::Remote { .. })
            ));
            assert!(matches!(
                LocalFs.ensure_prefix(url),
                Err(LayoutError::Remote { .. })
            ));
        }
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
