//! [`JsHost`] — a JS `Host` as a [`Host`], for every wasm32 binding that
//! reaches storage.
//!
//! A store needs its credential source to be `Send + Sync`, and a `JsValue` is
//! neither, so the host is owned by a task on the page's event loop and asked
//! over a channel: the pattern `object_store` itself uses to drive `fetch` from
//! wasm32.

use crate::credential::{Access, StorageCredential};
use fossil_graph_schema::Foreign;
use fossil_graph_schema::js::foreign;
use std::collections::HashMap;

use crate::store::{Host, Scope};
use futures::StreamExt;
use futures::channel::{mpsc, oneshot};
use futures::future::BoxFuture;
use js_sys::{Function, Promise, Reflect};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, spawn_local};

enum Request {
    Connections(oneshot::Sender<Result<HashMap<String, String>, Foreign>>),
    Credentials(
        Scope,
        Access,
        oneshot::Sender<Result<Vec<StorageCredential>, Foreign>>,
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
                        Request::Connections(reply) => {
                            let _ = reply.send(connections(&host).await);
                        }
                        Request::Credentials(scope, access, reply) => {
                            let _ = reply.send(credentials(&host, &scope, access).await);
                        }
                    }
                });
            }
        });
        Self { requests }
    }

    fn ask<T: Send + 'static>(
        &self,
        request: impl FnOnce(oneshot::Sender<Result<T, Foreign>>) -> Request,
    ) -> BoxFuture<'static, Result<T, Foreign>> {
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
    fn connections(&self) -> BoxFuture<'static, Result<HashMap<String, String>, Foreign>> {
        self.ask(Request::Connections)
    }

    fn credentials(
        &self,
        scope: &Scope,
        access: Access,
    ) -> BoxFuture<'static, Result<Vec<StorageCredential>, Foreign>> {
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

// These three run on the page's event loop, which is the point: the host never
// leaves it.
#[allow(clippy::future_not_send)]
async fn settle(returned: Result<JsValue, JsValue>, name: &str) -> Result<JsValue, Foreign> {
    let promise: Promise = returned.map_err(|e| foreign(&e))?.dyn_into().map_err(|_| {
        Foreign::named(
            "TypeError",
            format!("`{name}` returned something other than a promise"),
        )
    })?;
    JsFuture::from(promise).await.map_err(|e| foreign(&e))
}

#[allow(clippy::future_not_send)]
async fn connections(host: &JsValue) -> Result<HashMap<String, String>, Foreign> {
    let returned = method(host, "connections")?.call0(host);
    let map = settle(returned, "connections").await?;
    serde_wasm_bindgen::from_value(map).map_err(|e| shape(&e))
}

#[allow(clippy::future_not_send)]
async fn credentials(
    host: &JsValue,
    scope: &Scope,
    access: Access,
) -> Result<Vec<StorageCredential>, Foreign> {
    let scope = serde_wasm_bindgen::to_value(scope).map_err(|e| shape(&e))?;
    let access = serde_wasm_bindgen::to_value(&access).map_err(|e| shape(&e))?;
    let returned = method(host, "credentials")?.call2(host, &scope, &access);
    let vended = settle(returned, "credentials").await?;
    serde_wasm_bindgen::from_value(vended).map_err(|e| shape(&e))
}

/// A value the host handed back that is not the shape `Host` declares.
fn shape(error: &serde_wasm_bindgen::Error) -> Foreign {
    Foreign::named("TypeError", error.to_string())
}
