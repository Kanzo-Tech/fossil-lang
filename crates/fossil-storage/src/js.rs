//! [`JsHost`] — a JS `Host` as a [`Host`], for every wasm32 binding that
//! reaches storage.
//!
//! A store needs its credential source to be `Send + Sync`, and a `JsValue` is
//! neither, so the host is owned by a task on the page's event loop and asked
//! over a channel: the pattern `object_store` itself uses to drive `fetch` from
//! wasm32.
//!
//! **Every answer has a deadline**, [`HOST_MS`]: a host whose promise never
//! settles is [`HostError::Silent`], not a run that never ends.

use crate::credential::{Access, StorageCredential};
use fossil_graph_schema::Foreign;
use fossil_graph_schema::js::foreign;
use std::collections::HashMap;

use crate::store::{Host, HostError, Scope};
use futures::channel::{mpsc, oneshot};
use futures::future::{BoxFuture, Either, select};
use futures::{FutureExt, StreamExt};
use js_sys::{Function, Promise, Reflect};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, spawn_local};

/// How long a host may take to answer — `/docs/design/failure`'s G1 table, and
/// `@fossil-lang/storage`'s figure for the same wait.
const HOST_MS: u64 = 30_000;

enum Request {
    Connections(oneshot::Sender<Result<HashMap<String, String>, HostError>>),
    Credentials(
        Scope,
        Access,
        oneshot::Sender<Result<Vec<StorageCredential>, HostError>>,
    ),
}

#[derive(Debug, Clone)]
pub struct JsHost {
    requests: mpsc::UnboundedSender<Request>,
}

impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Connections(_) => "connections",
            Self::Credentials(..) => "credentials",
        })
    }
}

impl JsHost {
    /// Serve `host.credentials` until the last clone is dropped. `host` is the
    /// `@fossil-lang/types` `Host`.
    #[must_use]
    pub fn new(host: JsValue) -> Self {
        let (requests, mut incoming) = mpsc::unbounded::<Request>();
        spawn_local(async move {
            while let Some(request) = incoming.next().await {
                let host = host.clone();
                spawn_local(async move {
                    match request {
                        // A send fails only when the asker stopped waiting, and then
                        // nobody is left to tell.
                        Request::Connections(reply) => {
                            let _ = reply.send(connections(&host).await); // the asker is gone
                        }
                        Request::Credentials(scope, access, reply) => {
                            let _ = reply.send(credentials(&host, &scope, access).await); // the asker is gone
                        }
                    }
                });
            }
        });
        Self { requests }
    }

    fn ask<T: Send + 'static>(
        &self,
        request: impl FnOnce(oneshot::Sender<Result<T, HostError>>) -> Request,
    ) -> BoxFuture<'static, Result<T, HostError>> {
        let (reply, answer) = oneshot::channel();
        let sent = self.requests.unbounded_send(request(reply));
        Box::pin(async move {
            sent.map_err(|_| Foreign::named("Error", "the host is gone"))?;
            answer
                .await
                .map_err(|_| Foreign::named("Error", "the host dropped the request"))?
        })
    }
}

impl Host for JsHost {
    fn connections(&self) -> BoxFuture<'static, Result<HashMap<String, String>, HostError>> {
        self.ask(Request::Connections)
    }

    fn credentials(
        &self,
        scope: &Scope,
        access: Access,
    ) -> BoxFuture<'static, Result<Vec<StorageCredential>, HostError>> {
        let scope = scope.clone();
        self.ask(move |reply| Request::Credentials(scope, access, reply))
    }
}

fn method(host: &JsValue, name: &str) -> Result<Function, Foreign> {
    Reflect::get(host, &JsValue::from_str(name))
        .ok()
        .and_then(|f| f.dyn_into().ok())
        .ok_or_else(|| Foreign::named("TypeError", format!("the host has no `{name}`")))
}

// These run on the page's event loop, which is the point: the host never
// leaves it.

/// What a host call is handed — `@fossil-lang/types`' `HostCall`: a signal
/// that aborts when fossil stops waiting, and the controller that aborts it.
fn call() -> Result<(JsValue, JsValue), Foreign> {
    let global = js_sys::global();
    let controller = Reflect::get(&global, &JsValue::from_str("AbortController"))
        .ok()
        .and_then(|c| c.dyn_into::<Function>().ok())
        .and_then(|c| Reflect::construct(&c, &js_sys::Array::new()).ok())
        .ok_or_else(|| Foreign::named("TypeError", "this runtime has no AbortController"))?;
    let signal =
        Reflect::get(&controller, &JsValue::from_str("signal")).map_err(|e| foreign(&e))?;
    let options = js_sys::Object::new();
    Reflect::set(&options, &JsValue::from_str("signal"), &signal).map_err(|e| foreign(&e))?;
    Ok((options.into(), controller))
}

#[allow(clippy::future_not_send)]
async fn settle(
    returned: Result<JsValue, JsValue>,
    controller: &JsValue,
    name: &str,
) -> Result<JsValue, HostError> {
    let promise: Promise = returned.map_err(|e| foreign(&e))?.dyn_into().map_err(|_| {
        Foreign::named(
            "TypeError",
            format!("`{name}` returned something other than a promise"),
        )
    })?;
    match select(JsFuture::from(promise), elapse(HOST_MS)).await {
        Either::Left((answer, _)) => Ok(answer.map_err(|e| foreign(&e))?),
        Either::Right(((), _)) => {
            // Tell the host to stop: its late answer is no longer read either way.
            if let Ok(abort) = method(controller, "abort") {
                abort.call0(controller).map_err(|e| foreign(&e))?;
            }
            Err(HostError::Silent { after: HOST_MS })
        }
    }
}

#[allow(clippy::future_not_send)]
async fn connections(host: &JsValue) -> Result<HashMap<String, String>, HostError> {
    let (options, controller) = call()?;
    let returned = method(host, "connections")?.call1(host, &options);
    let map = settle(returned, &controller, "connections").await?;
    Ok(serde_wasm_bindgen::from_value(map).map_err(|e| shape(&e))?)
}

#[allow(clippy::future_not_send)]
async fn credentials(
    host: &JsValue,
    scope: &Scope,
    access: Access,
) -> Result<Vec<StorageCredential>, HostError> {
    let scope = serde_wasm_bindgen::to_value(scope).map_err(|e| shape(&e))?;
    let access = serde_wasm_bindgen::to_value(&access).map_err(|e| shape(&e))?;
    let (options, controller) = call()?;
    let returned = method(host, "credentials")?.call3(host, &scope, &access, &options);
    let vended = settle(returned, &controller, "credentials").await?;
    Ok(serde_wasm_bindgen::from_value(vended).map_err(|e| shape(&e))?)
}

/// Resolves once `ms` have passed, by the page's own `setTimeout` — so a
/// test's fake clock drives it — and is `Send`, so a store's future can race it.
///
/// The timer is `unref`'d where the runtime has that (Node), so a deadline that
/// lost its race keeps no process alive; it is not cleared, because the id is
/// a `JsValue` and the future must stay `Send`.
// `pub(crate)` for `store.rs`'s request deadline: `unreachable_pub` refuses a
// bare `pub` in this private module and `redundant_pub_crate` the other.
#[allow(clippy::redundant_pub_crate)]
pub(crate) fn elapse(ms: u64) -> impl std::future::Future<Output = ()> + Send + Unpin {
    let (fired, done) = oneshot::channel::<()>();
    let callback = Closure::once_into_js(move || {
        // A deadline whose race was already won has no receiver left.
        let _ = fired.send(());
    });
    let global = js_sys::global();
    #[allow(clippy::cast_precision_loss)] // a deadline is seconds, far below 2^53 ms
    let delay = JsValue::from_f64(ms as f64);
    if let Ok(timer) = method(&global, "setTimeout").and_then(|set| {
        set.call2(&global, &callback, &delay)
            .map_err(|e| foreign(&e))
    }) && let Ok(unref) = method(&timer, "unref")
    {
        // `unref` returns the timer; a runtime whose timer has none is a browser.
        let _ = unref.call0(&timer);
    }
    // Cancelled only if the callback is dropped unfired, which JS never does to
    // a closure it holds; either way the deadline has nothing left to wait for.
    done.map(|_cancelled| ())
}

/// A value the host handed back that is not the shape `Host` declares.
fn shape(error: &serde_wasm_bindgen::Error) -> Foreign {
    Foreign::named("TypeError", error.to_string())
}
