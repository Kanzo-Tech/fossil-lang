//! `fossil-df-wasm` — [`fossil_df::Executor`] exposed to JS.
//!
//! The executor is `fossil-df`'s: compile, register the documents a program
//! names, read its sources through a [`Storage`], execute on `DataFusion`, and
//! write the `fossil/1` corpus. This crate only marshals JS values around it,
//! and it is the one host that writes a corpus — in the browser, and in Node
//! through `@fossil-lang/executor`.

// The executor is single-threaded by construction (the browser has no threads),
// so its futures need not be `Send` — and DataFusion's execution futures aren't.
#![allow(clippy::future_not_send)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use fossil_df::Executor;
use fossil_storage::{Access, JsHost, Scope, Storage};
use futures::TryStreamExt;
use object_store::memory::InMemory;
use object_store::{ObjectStore, ObjectStoreExt};
use url::Url;
use wasm_bindgen::prelude::*;

// ───────────────────────── wasm-bindgen surface ─────────────────────────────

/// The JS-facing executor — `FossilExecutor` on the JS side, a `RefCell`
/// around the Rust [`Executor`], for the reason `fossil-wasm`'s
/// `WasmWorkspace` gives: a `&mut self` export borrows wasm-bindgen's cell
/// exclusively, a failed borrow panics, and on `wasm32` that poisons the object
/// for good. Every export takes `&self`; a register while a run is in flight
/// returns a catchable `Error` instead.
#[wasm_bindgen(js_name = FossilExecutor)]
#[derive(Debug)]
pub struct FossilExecutor {
    inner: RefCell<Executor>,
}

#[wasm_bindgen(js_class = FossilExecutor)]
impl FossilExecutor {
    /// Compile `program`, installing the panic hook so a Rust panic surfaces as
    /// a `console.error` stack trace instead of an opaque `unreachable`.
    // `wasm_bindgen` dictates these by value: it owns the JS→Rust conversion
    // and cannot hand out borrows across the boundary.
    ///
    /// `path` is where the program lives — a URL, e.g. `file:///…/hello.fossil`
    /// — so the relative sources and documents it names resolve beside it.
    /// Without one, only `@conn/…` and absolute URLs resolve.
    #[wasm_bindgen(constructor)]
    #[must_use]
    #[allow(clippy::needless_pass_by_value)]
    pub fn new(program: String, path: Option<String>) -> Self {
        console_error_panic_hook::set_once();
        let executor = path.map_or_else(
            || Executor::new(&program),
            |path| Executor::at(&program, &path),
        );
        Self {
            inner: RefCell::new(executor),
        }
    }

    /// `{ name: baseUrl }` — what `@name/…` expands against.
    /// `@fossil-lang/storage`'s `resolveDocuments` sets it from the host.
    ///
    /// # Errors
    /// A JS `Error` if `connections` is not an object of strings, or a run is
    /// in flight.
    #[wasm_bindgen(js_name = setConnections)]
    pub fn set_connections(&self, connections: &JsValue) -> Result<(), JsError> {
        let connections = parse_connections(connections).map_err(|e| JsError::new(&e))?;
        self.borrow_mut("setConnections")?
            .set_connections(connections);
        Ok(())
    }

    /// `[{ key, locator, connection? }]` — the documents the program names and
    /// the executor does not hold. `@fossil-lang/storage`'s `resolveDocuments`
    /// reads them.
    ///
    /// # Errors
    /// A JS `Error` if a register is in flight.
    #[wasm_bindgen(js_name = missingDocuments)]
    pub fn missing_documents(&self) -> Result<JsValue, JsError> {
        let arr = js_sys::Array::new();
        for missing in self.borrow()?.missing_documents() {
            let obj = js_sys::Object::new();
            set(&obj, "key", &JsValue::from_str(&missing.key)).map_err(|e| JsError::new(&e))?;
            set(&obj, "locator", &JsValue::from_str(&missing.locator))
                .map_err(|e| JsError::new(&e))?;
            if let Some(connection) = &missing.connection {
                set(&obj, "connection", &JsValue::from_str(connection))
                    .map_err(|e| JsError::new(&e))?;
            }
            arr.push(&obj);
        }
        Ok(arr.into())
    }

    /// Register a fetched document under the `key` `missingDocuments` gave it.
    ///
    /// # Errors
    /// A JS `Error` if a run is in flight.
    #[wasm_bindgen(js_name = registerDocument)]
    #[allow(clippy::needless_pass_by_value)]
    pub fn register_document(&self, key: String, text: String) -> Result<(), JsError> {
        self.borrow_mut("registerDocument")?
            .register_document(&key, &text);
        Ok(())
    }

    /// `[{ uri, format, connection? }]` — the sources to read, `uri` being the
    /// locator fossil resolved, `format` the catalogue row's name and
    /// `connection` the one a credential is vended for.
    ///
    /// # Errors
    /// A JS `Error` if the output shape document is unregistered or does not
    /// decode.
    pub fn sources(&self) -> Result<JsValue, JsError> {
        let srcs = self.borrow()?.sources().map_err(|e| JsError::new(&e))?;
        let arr = js_sys::Array::new();
        for (uri, format, connection) in srcs {
            let obj = js_sys::Object::new();
            set(&obj, "uri", &JsValue::from_str(&uri)).map_err(|e| JsError::new(&e))?;
            set(&obj, "format", &JsValue::from_str(&format)).map_err(|e| JsError::new(&e))?;
            if let Some(connection) = &connection {
                set(&obj, "connection", &JsValue::from_str(connection))
                    .map_err(|e| JsError::new(&e))?;
            }
            arr.push(&obj);
        }
        Ok(arr.into())
    }

    /// Run with the storage `host` vends: read each source through its
    /// connection's credential, write under the one prefix `host` vends `write`
    /// on for `job`, and answer the [`fossil_df::RunReport`].
    ///
    /// # Errors
    /// A JS `Error` when the host vends no single prefix to write under, or the
    /// run fails — named `OverBudget` when it needed more memory than the
    /// executor's budget, which is raised before anything is written.
    // The shared borrow is held across the run on purpose: it is what makes a
    // `registerDocument` issued mid-run fail instead of changing the program
    // under it.
    #[allow(clippy::await_holding_refcell_ref, clippy::future_not_send)]
    pub async fn run(&self, host: JsValue, job: String) -> Result<JsValue, JsValue> {
        let mut storage = Storage::new(Arc::new(JsHost::new(host)));
        let scope = Scope::Job(job);
        let dest = match storage.grant(scope.clone(), Access::Write).await {
            Ok([prefix]) => prefix.clone(),
            Ok(prefixes) => {
                return Err(JsError::new(&format!(
                    "the host vended {} write credentials for {scope}; a run writes under exactly one prefix",
                    prefixes.len()
                ))
                .into());
            }
            Err(e) => return Err(JsError::new(&e.to_string()).into()),
        };
        let exec = self.borrow()?;
        let report = exec
            .execute(&mut storage, &dest)
            .await
            .map_err(|e| run_error(&e))?;
        Ok(serde_wasm_bindgen::to_value(&report)?)
    }

    /// Run over files held in memory, for a host with no storage: `sources`
    /// maps each locator the program reads to its bytes, and the output stays
    /// in memory under `dest`. Answers `{ files: [{ path, bytes }], report }`.
    ///
    /// # Errors
    /// A JS `Error` when `sources` is not an object of `Uint8Array`s, or the run
    /// fails — named `OverBudget` as [`Self::run`]'s is.
    #[allow(clippy::await_holding_refcell_ref, clippy::future_not_send)]
    #[wasm_bindgen(js_name = runInMemory)]
    pub async fn run_in_memory(&self, sources: JsValue, dest: String) -> Result<JsValue, JsValue> {
        let dest = format!("{}/", dest.trim_end_matches('/'));
        let mut storage = Storage::new(Arc::new(NoHost));
        let mut held: HashMap<String, Arc<InMemory>> = HashMap::new();
        let mut memory = |storage: &mut Storage, root: &str| -> Result<Arc<InMemory>, JsError> {
            if let Some(store) = held.get(root) {
                return Ok(Arc::clone(store));
            }
            let store = Arc::new(InMemory::new());
            storage
                .with_store(root, Arc::clone(&store) as Arc<dyn ObjectStore>)
                .map_err(|e| JsError::new(&e.to_string()))?;
            held.insert(root.to_string(), Arc::clone(&store));
            Ok(store)
        };
        let obj: &js_sys::Object = sources
            .dyn_ref()
            .ok_or_else(|| JsError::new("`sources` is `{ [locator]: Uint8Array }`"))?;
        for entry in js_sys::Object::entries(obj).iter() {
            let pair: js_sys::Array = entry.into();
            let locator = pair
                .get(0)
                .as_string()
                .ok_or_else(|| JsError::new("a source locator is a string"))?;
            let bytes: js_sys::Uint8Array = pair
                .get(1)
                .dyn_into()
                .map_err(|_| JsError::new("a source's bytes are a Uint8Array"))?;
            let url = Url::parse(&locator).map_err(|e| JsError::new(&format!("{locator}: {e}")))?;
            let root = format!("{}://{}/", url.scheme(), url.authority());
            let store = memory(&mut storage, &root)?;
            store
                .put(
                    &object_store::path::Path::from(url.path().trim_start_matches('/')),
                    bytes.to_vec().into(),
                )
                .await
                .map_err(|e| JsError::new(&e.to_string()))?;
        }
        let out = Arc::new(InMemory::new());
        storage
            .with_store(&dest, Arc::clone(&out) as Arc<dyn ObjectStore>)
            .map_err(|e| JsError::new(&e.to_string()))?;

        let exec = self.borrow()?;
        let report = exec
            .execute(&mut storage, &dest)
            .await
            .map_err(|e| run_error(&e))?;

        let key = Url::parse(&dest)
            .map_err(|e| JsError::new(&e.to_string()))?
            .path()
            .trim_start_matches('/')
            .to_string();
        let files = js_sys::Array::new();
        let listed: Vec<_> = out
            .list(None)
            .try_collect()
            .await
            .map_err(|e| JsError::new(&e.to_string()))?;
        for meta in listed {
            let bytes = out
                .get(&meta.location)
                .await
                .map_err(|e| JsError::new(&e.to_string()))?
                .bytes()
                .await
                .map_err(|e| JsError::new(&e.to_string()))?;
            let location = meta.location.as_ref();
            let path = location.strip_prefix(&key).unwrap_or(location);
            let file = js_sys::Object::new();
            set(&file, "path", &JsValue::from_str(path)).map_err(|e| JsError::new(&e))?;
            set(&file, "bytes", &js_sys::Uint8Array::from(bytes.as_ref()))
                .map_err(|e| JsError::new(&e))?;
            files.push(&file);
        }
        let result = js_sys::Object::new();
        set(&result, "files", &files).map_err(|e| JsError::new(&e))?;
        set(&result, "report", &serde_wasm_bindgen::to_value(&report)?)
            .map_err(|e| JsError::new(&e))?;
        Ok(result.into())
    }
}

/// A failed run as a JS `Error`. One that needed more memory than the budget
/// is named `OverBudget`, so a host can tell it from a program's own failure
/// without reading the message.
fn run_error(e: &fossil_df::RunError) -> JsValue {
    let error = js_sys::Error::new(&e.to_string());
    if matches!(e, fossil_df::RunError::OverBudget(_)) {
        error.set_name("OverBudget");
    }
    error.into()
}

/// The host of a run with no storage: it has no connections and vends nothing,
/// so a source that is not in memory is refused by name.
#[derive(Debug)]
struct NoHost;

impl fossil_storage::Host for NoHost {
    fn connections(
        &self,
    ) -> futures::future::BoxFuture<'static, Result<HashMap<String, String>, String>> {
        Box::pin(async { Ok(HashMap::new()) })
    }

    fn credentials(
        &self,
        scope: &Scope,
        _access: Access,
    ) -> futures::future::BoxFuture<'static, Result<Vec<fossil_storage::StorageCredential>, String>>
    {
        let refused = format!("a run in memory has no storage to vend {scope} from");
        Box::pin(async move { Err(refused) })
    }
}

impl FossilExecutor {
    fn borrow(&self) -> Result<std::cell::Ref<'_, Executor>, JsError> {
        self.inner
            .try_borrow()
            .map_err(|_| JsError::new("the executor is being changed"))
    }

    fn borrow_mut(&self, call: &str) -> Result<std::cell::RefMut<'_, Executor>, JsError> {
        self.inner
            .try_borrow_mut()
            .map_err(|_| JsError::new(&format!("{call} during a run")))
    }
}

/// Parse the JS `connections` object `{ name: baseUrl }`.
fn parse_connections(connections: &JsValue) -> Result<HashMap<String, String>, String> {
    let obj: &js_sys::Object = connections
        .dyn_ref::<js_sys::Object>()
        .ok_or("`connections` must be an object of { name: baseUrl }")?;
    let mut map = HashMap::new();
    for entry in js_sys::Object::entries(obj).iter() {
        let pair: js_sys::Array = entry.into();
        let name = pair
            .get(0)
            .as_string()
            .ok_or("`connections` keys must be strings")?;
        let url = pair
            .get(1)
            .as_string()
            .ok_or("`connections` values must be strings")?;
        map.insert(name, url);
    }
    Ok(map)
}

fn set(obj: &js_sys::Object, key: &str, value: &JsValue) -> Result<(), String> {
    js_sys::Reflect::set(obj, &JsValue::from_str(key), value)
        .map(|_| ())
        .map_err(|e| js_err(&e))
}

fn js_err(e: &JsValue) -> String {
    e.as_string().unwrap_or_else(|| "JS error".to_string())
}
