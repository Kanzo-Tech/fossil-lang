//! [`DescriptorCache`] — the one table of host-introspected input schemas.
//!
//! Three crates used to carry a private `Mutex<HashMap<_, InferredDescriptor>>`
//! and a pair of `System` trait methods to reach it. They are one table now,
//! and it is reached through a single accessor.
//!
//! Two properties are load-bearing:
//!
//! - **It is ambient, never a query key.** A [`DescriptorCache`] is reached
//!   through the context (`db.system().descriptors()`); no Salsa query is
//!   keyed by it, interns it, or hashes it. Reading it from inside a tracked
//!   query registers no dependency, exactly as `read_file` does.
//! - **It holds data, not behaviour.** There is nothing to dispatch here, so
//!   there is no trait object and no `dyn` anything — the table is a concrete
//!   struct and the accessor hands out a `&`.
//!
//! The key is the source **URI as written in the program** — the string inside
//! `io.csv("…")` — and not the binding name. Two bindings over one file
//! introspect once; renaming a binding does not throw the entry away; and the
//! entry's freshness is a property of the file, which only the URI names.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use smol_str::SmolStr;

use crate::InferredDescriptor;

/// The introspected-schema table, keyed by source URI.
///
/// Interior mutability: registration takes `&self` because hosts hold the
/// cache behind a shared `System`. `Mutex` and not `RwLock` — writes happen
/// once per source per compile, reads are per-mapping and cheap, and the
/// read-side of an `RwLock` is not free enough to pay for that shape.
///
/// A poisoned lock is treated as an empty table rather than propagated: every
/// consumer already has a "no descriptor" path (forward propagation is
/// disabled for that source), and a panic in an unrelated thread is not a
/// reason to fail a compile.
#[derive(Debug, Default)]
pub struct DescriptorCache {
    table: Mutex<HashMap<SmolStr, InferredDescriptor>>,
    /// How many descriptors have been *inserted* over this cache's life.
    ///
    /// This is the introspection counter: a host inserts only after it has
    /// actually gone and read the source, so a call that the freshness check
    /// skipped does not move it. It is what makes "did that re-introspect?" a
    /// question with a number for an answer instead of a stopwatch.
    registrations: AtomicU64,
}

impl DescriptorCache {
    /// An empty cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The descriptor registered for `uri`, if any. Cloned: the table cannot
    /// lend a borrow across the lock guard, and a descriptor is a short `Vec`
    /// of `(SmolStr, Primitive)` pairs.
    #[must_use]
    pub fn get(&self, uri: &str) -> Option<InferredDescriptor> {
        self.table.lock().ok()?.get(uri).cloned()
    }

    /// Register `descriptor` under its own [`InferredDescriptor::uri`],
    /// replacing any previous entry, and count the introspection that
    /// produced it.
    pub fn insert(&self, descriptor: InferredDescriptor) {
        if let Ok(mut table) = self.table.lock() {
            table.insert(descriptor.uri.clone(), descriptor);
            self.registrations.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Does the cache already hold an entry for `uri` whose freshness token
    /// equals `token`? If it does, introspecting `uri` again would produce the
    /// same descriptor and the host should skip the read.
    ///
    /// An empty `token` is never fresh. A host that cannot cheaply establish
    /// whether a source changed says so by handing over an empty token, and
    /// then it re-introspects every time — the honest answer, and the safe one.
    #[must_use]
    pub fn is_fresh(&self, uri: &str, token: &str) -> bool {
        if token.is_empty() {
            return false;
        }
        self.table
            .lock()
            .is_ok_and(|t| t.get(uri).is_some_and(|d| d.freshness_token == token))
    }

    /// Number of descriptors inserted since this cache was created. A skipped
    /// re-introspection does not increment it — that is the whole point.
    #[must_use]
    pub fn registrations(&self) -> u64 {
        self.registrations.load(Ordering::Relaxed)
    }

    /// Number of URIs currently held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.table.lock().map_or(0, |t| t.len())
    }

    /// Is the table empty?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::InferredColumn;
    use fossil_graph_schema::Primitive;

    fn descriptor(uri: &str, token: &str, cols: &[&str]) -> InferredDescriptor {
        InferredDescriptor {
            uri: uri.into(),
            columns: cols
                .iter()
                .map(|c| InferredColumn {
                    name: (*c).into(),
                    primitive: Primitive::String,
                })
                .collect(),
            freshness_token: token.to_string(),
        }
    }

    #[test]
    fn a_descriptor_is_found_under_its_uri_not_under_a_binding_name() {
        let cache = DescriptorCache::new();
        cache.insert(descriptor("examples/users.csv", "t1", &["id"]));
        assert!(
            cache.get("users").is_none(),
            "the binding name is not a key"
        );
        assert_eq!(
            cache.get("examples/users.csv").expect("present").uri,
            "examples/users.csv"
        );
    }

    #[test]
    fn re_registering_a_uri_replaces_the_entry_and_counts_the_read() {
        let cache = DescriptorCache::new();
        cache.insert(descriptor("u.csv", "t1", &["id"]));
        cache.insert(descriptor("u.csv", "t2", &["id", "name"]));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.registrations(), 2);
        assert_eq!(cache.get("u.csv").expect("present").columns.len(), 2);
    }

    #[test]
    fn freshness_is_the_token_and_an_empty_token_is_never_fresh() {
        let cache = DescriptorCache::new();
        cache.insert(descriptor("u.csv", "t1", &["id"]));
        assert!(cache.is_fresh("u.csv", "t1"));
        assert!(!cache.is_fresh("u.csv", "t2"), "a moved token is stale");
        assert!(
            !cache.is_fresh("other.csv", "t1"),
            "an unknown URI is stale"
        );

        cache.insert(descriptor("blind.csv", "", &["id"]));
        assert!(
            !cache.is_fresh("blind.csv", ""),
            "a host that cannot tell must re-read"
        );
    }
}
