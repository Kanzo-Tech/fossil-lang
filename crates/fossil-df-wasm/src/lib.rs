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
// `result_large_err`: the helpers here refuse with `fossil_graph_schema::Failure`
// (144 bytes, over clippy's 128) on the path that ends the call, where the copy
// costs nothing measurable — `fossil-df`'s crate root says the same.
#![allow(clippy::result_large_err)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use fossil_df::Executor;
use fossil_graph_schema::js::{bug, invalid_argument};
use fossil_graph_schema::{Failure, Foreign, Problem};
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
/// throws a catchable `FossilError`, `api/busy`, instead.
///
/// Everything thrown here is a `FossilError` built by
/// `fossil_graph_schema::js::to_js` — a [`Failure`] with a code.
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
    /// `api/invalid-argument` if `connections` is not an object of strings,
    /// `api/busy` if a run is in flight.
    #[wasm_bindgen(js_name = setConnections)]
    pub fn set_connections(
        &self,
        #[wasm_bindgen(unchecked_param_type = "Record<string, string>")] connections: JsValue,
    ) -> Result<(), JsValue> {
        let connections: HashMap<String, String> = serde_wasm_bindgen::from_value(connections)
            .map_err(|e| {
                invalid_argument(
                    "connections",
                    "an object of { name: baseUrl }",
                    Some(e.into()),
                )
            })?;
        self.borrow_mut("setConnections")?
            .set_connections(connections);
        Ok(())
    }

    /// `[{ key, locator, connection? }]` — the documents the program names and
    /// the executor does not hold. `@fossil-lang/storage`'s `resolveDocuments`
    /// reads them.
    ///
    /// # Errors
    /// `api/busy` if a register is in flight.
    #[wasm_bindgen(js_name = missingDocuments)]
    pub fn missing_documents(&self) -> Result<JsValue, JsValue> {
        Ok(to_value(
            "the missing documents",
            &self.borrow("missingDocuments")?.missing_documents(),
        )?)
    }

    /// Register a fetched document under the `key` `missingDocuments` gave it.
    ///
    /// # Errors
    /// `api/busy` if a run is in flight.
    #[wasm_bindgen(js_name = registerDocument)]
    #[allow(clippy::needless_pass_by_value)]
    pub fn register_document(&self, key: String, text: String) -> Result<(), JsValue> {
        self.borrow_mut("registerDocument")?
            .register_document(&key, &text);
        Ok(())
    }

    /// The sources to read — `ProgramSource`, the list every host introspects.
    ///
    /// # Errors
    /// `api/busy` if a register is in flight.
    pub fn sources(&self) -> Result<JsValue, JsValue> {
        Ok(to_value("the sources", &self.borrow("sources")?.sources())?)
    }

    /// Run with the storage `host` vends: read each source through its
    /// connection's credential, write under the one prefix `host` vends `write`
    /// on for `job`, and answer the [`fossil_df::RunReport`].
    ///
    /// # Errors
    /// `storage/ambiguous-prefix` when the host vends more than one prefix to
    /// write under, the storage failure when it vends none, or the run's own
    /// failure — `run/over-budget` when it needed more memory than the
    /// executor's budget, which is raised before anything is written.
    ///
    /// `signal`, an `AbortSignal`, stops the run: it rejects with the signal's
    /// reason and the run is dropped where it stands. `fossil.json` is written
    /// last, so a stopped run leaves no corpus — only the files it had already
    /// written under the prefix.
    // The shared borrow is held across the run on purpose: it is what makes a
    // `registerDocument` issued mid-run fail instead of changing the program
    // under it.
    #[allow(clippy::await_holding_refcell_ref, clippy::future_not_send)]
    pub async fn run(
        &self,
        host: JsValue,
        job: String,
        signal: Option<js_sys::Object>,
    ) -> Result<JsValue, JsValue> {
        let Some(signal) = signal else {
            return self.run_until(host, job).await;
        };
        let stopped = stopped(&signal)?;
        let run = std::pin::pin!(self.run_until(host, job));
        match futures::future::select(run, stopped).await {
            futures::future::Either::Left((done, _)) => done,
            futures::future::Either::Right(_) => {
                Err(js_sys::Reflect::get(&signal, &JsValue::from_str("reason"))
                    .unwrap_or(JsValue::UNDEFINED))
            }
        }
    }

    #[allow(clippy::await_holding_refcell_ref, clippy::future_not_send)]
    async fn run_until(&self, host: JsValue, job: String) -> Result<JsValue, JsValue> {
        let mut storage = Storage::new(Arc::new(JsHost::new(host)));
        let scope = Scope::Job(job);
        let dest = match storage.grant(scope.clone(), Access::Write).await {
            Ok([prefix]) => prefix.clone(),
            Ok(prefixes) => {
                return Err(Failure::new(Problem::AmbiguousPrefix {
                    scope: scope.to_string(),
                    count: prefixes.len() as u64,
                })
                .with_help("a run writes under exactly one prefix")
                .into());
            }
            Err(e) => return Err(Failure::from(e).into()),
        };
        let exec = self.borrow("run")?;
        let report = exec.execute(&mut storage, &dest).await?;
        Ok(to_value("the run report", &report)?)
    }

    /// Run over files held in memory, for a host with no storage: `sources`
    /// maps each locator the program reads to its bytes, and the output stays
    /// in memory under `dest`. Answers `{ files: [{ path, bytes }], report }`.
    ///
    /// # Errors
    /// `api/invalid-argument` when `sources` is not an object of `Uint8Array`s
    /// keyed by URL, or the run's own failure, as [`Self::run`]'s is.
    #[allow(clippy::await_holding_refcell_ref, clippy::future_not_send)]
    #[wasm_bindgen(js_name = runInMemory)]
    pub async fn run_in_memory(&self, sources: JsValue, dest: String) -> Result<JsValue, JsValue> {
        let dest = format!("{}/", dest.trim_end_matches('/'));
        let mut storage = Storage::new(Arc::new(NoHost));
        let mut held: HashMap<String, Arc<InMemory>> = HashMap::new();
        let mut memory = |storage: &mut Storage, root: &str| -> Result<Arc<InMemory>, Failure> {
            if let Some(store) = held.get(root) {
                return Ok(Arc::clone(store));
            }
            let store = Arc::new(InMemory::new());
            storage.with_store(root, Arc::clone(&store) as Arc<dyn ObjectStore>)?;
            held.insert(root.to_string(), Arc::clone(&store));
            Ok(store)
        };
        let shape = |error: Option<JsValue>| {
            invalid_argument("sources", "an object of { [url]: Uint8Array }", error)
        };
        let obj: &js_sys::Object = sources.dyn_ref().ok_or_else(|| shape(None))?;
        for entry in js_sys::Object::entries(obj).iter() {
            let pair: js_sys::Array = entry.into();
            let locator = pair.get(0).as_string().ok_or_else(|| shape(None))?;
            let bytes: js_sys::Uint8Array = pair.get(1).dyn_into().map_err(|_| shape(None))?;
            let url = Url::parse(&locator).map_err(|e| {
                invalid_argument("sources", "an object keyed by URL", None).caused_by(e)
            })?;
            let root = format!("{}://{}/", url.scheme(), url.authority());
            let store = memory(&mut storage, &root)?;
            store
                .put(
                    &object_store::path::Path::from(url.path().trim_start_matches('/')),
                    bytes.to_vec().into(),
                )
                .await
                .map_err(|e| unreachable(&locator, e))?;
        }
        let out = Arc::new(InMemory::new());
        storage
            .with_store(&dest, Arc::clone(&out) as Arc<dyn ObjectStore>)
            .map_err(Failure::from)?;

        let exec = self.borrow("runInMemory")?;
        let report = exec.execute(&mut storage, &dest).await?;

        let key = Url::parse(&dest)
            .map_err(|e| invalid_argument("dest", "a URL", None).caused_by(e))?
            .path()
            .trim_start_matches('/')
            .to_string();
        let files = js_sys::Array::new();
        let listed: Vec<_> = out
            .list(None)
            .try_collect()
            .await
            .map_err(|e| unreachable(&dest, e))?;
        for meta in listed {
            let bytes = out
                .get(&meta.location)
                .await
                .map_err(|e| unreachable(&dest, e))?
                .bytes()
                .await
                .map_err(|e| unreachable(&dest, e))?;
            let location = meta.location.as_ref();
            let path = location.strip_prefix(&key).unwrap_or(location);
            let file = js_sys::Object::new();
            set(&file, "path", &JsValue::from_str(path))?;
            set(&file, "bytes", &js_sys::Uint8Array::from(bytes.as_ref()))?;
            files.push(&file);
        }
        let result = js_sys::Object::new();
        set(&result, "files", &files)?;
        set(&result, "report", &to_value("the run report", &report)?)?;
        Ok(result.into())
    }
}

/// A future that resolves when `signal` aborts — at once if it already has.
fn stopped(signal: &js_sys::Object) -> Result<wasm_bindgen_futures::JsFuture, Failure> {
    let aborted = js_sys::Reflect::get(signal, &JsValue::from_str("aborted"))
        .map_err(|e| invalid_argument("signal", "an AbortSignal", Some(e)))?;
    let listen: js_sys::Function =
        js_sys::Reflect::get(signal, &JsValue::from_str("addEventListener"))
            .ok()
            .and_then(|f| f.dyn_into().ok())
            .ok_or_else(|| invalid_argument("signal", "an AbortSignal", None))?;
    let once = js_sys::Object::new();
    js_sys::Reflect::set(&once, &JsValue::from_str("once"), &JsValue::TRUE)
        .map_err(|e| bug("setting `once` on a fresh object", e))?;
    let mut failed = None;
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        if aborted.is_truthy() {
            failed = resolve.call0(&JsValue::UNDEFINED).err();
            return;
        }
        failed = listen
            .call3(signal, &JsValue::from_str("abort"), &resolve, &once)
            .err();
    });
    if let Some(e) = failed {
        return Err(invalid_argument("signal", "an AbortSignal", Some(e)));
    }
    Ok(wasm_bindgen_futures::JsFuture::from(promise))
}

/// An in-memory store that answered with an error, as the storage failure a
/// real store's would be.
fn unreachable(locator: &str, error: object_store::Error) -> Failure {
    Failure::new(Problem::Unreachable {
        locator: locator.to_string(),
    })
    .caused_by(error)
}

/// Plain data fossil just made, as a JS value.
fn to_value<T: serde::Serialize + ?Sized>(what: &str, value: &T) -> Result<JsValue, Failure> {
    serde_wasm_bindgen::to_value(value).map_err(|e| bug(format!("serialising {what}"), e))
}

/// The host of a run with no storage: it has no connections and vends nothing,
/// so a source that is not in memory is refused by name.
#[derive(Debug)]
struct NoHost;

impl fossil_storage::Host for NoHost {
    fn connections(
        &self,
    ) -> futures::future::BoxFuture<
        'static,
        Result<HashMap<String, String>, fossil_storage::HostError>,
    > {
        Box::pin(async { Ok(HashMap::new()) })
    }

    fn credentials(
        &self,
        scope: &Scope,
        _access: Access,
    ) -> futures::future::BoxFuture<
        'static,
        Result<Vec<fossil_storage::StorageCredential>, fossil_storage::HostError>,
    > {
        let refused = Foreign::named(
            "Error",
            format!("a run in memory has no storage to vend {scope} from"),
        );
        Box::pin(async move { Err(refused.into()) })
    }
}

impl FossilExecutor {
    fn borrow(&self, call: &str) -> Result<std::cell::Ref<'_, Executor>, Failure> {
        self.inner.try_borrow().map_err(|_| busy(call))
    }

    fn borrow_mut(&self, call: &str) -> Result<std::cell::RefMut<'_, Executor>, Failure> {
        self.inner.try_borrow_mut().map_err(|_| busy(call))
    }
}

fn busy(call: &str) -> Failure {
    Failure::new(Problem::Busy {
        call: call.to_string(),
    })
}

fn set(obj: &js_sys::Object, key: &str, value: &JsValue) -> Result<(), Failure> {
    js_sys::Reflect::set(obj, &JsValue::from_str(key), value)
        .map(|_| ())
        .map_err(|e| bug(format!("setting `{key}` on a fresh object"), e))
}
