//! [`JsHost`] — a JS `Host` as a [`Host`], for every wasm32 binding that
//! reaches storage.
//!
//! A store needs its credential source to be `Send + Sync`, and a `JsValue` is
//! neither, so the host is owned by a task on the page's event loop and asked
//! over a channel: the pattern `object_store` itself uses to drive `fetch` from
//! wasm32.

use crate::credential::{Access, StorageCredential};
use std::collections::HashMap;

use crate::store::{Host, Scope};
use futures::StreamExt;
use futures::channel::{mpsc, oneshot};
use futures::future::BoxFuture;
use js_sys::{Function, Promise, Reflect};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, spawn_local};

enum Request {
    Connections(oneshot::Sender<Result<HashMap<String, String>, String>>),
    Credentials(
        Scope,
        Access,
        oneshot::Sender<Result<Vec<StorageCredential>, String>>,
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
        request: impl FnOnce(oneshot::Sender<Result<T, String>>) -> Request,
    ) -> BoxFuture<'static, Result<T, String>> {
        let (reply, answer) = oneshot::channel();
        let sent = self.requests.unbounded_send(request(reply));
        Box::pin(async move {
            sent.map_err(|_| "the host is gone".to_string())?;
            answer
                .await
                .map_err(|_| "the host dropped the request".to_string())?
        })
    }
}

impl Host for JsHost {
    fn connections(&self) -> BoxFuture<'static, Result<HashMap<String, String>, String>> {
        self.ask(Request::Connections)
    }

    fn credentials(
        &self,
        scope: &Scope,
        access: Access,
    ) -> BoxFuture<'static, Result<Vec<StorageCredential>, String>> {
        let scope = scope.clone();
        self.ask(move |reply| Request::Credentials(scope, access, reply))
    }
}

fn method(host: &JsValue, name: &str) -> Result<Function, String> {
    Reflect::get(host, &JsValue::from_str(name))
        .ok()
        .and_then(|f| f.dyn_into().ok())
        .ok_or_else(|| format!("the host has no `{name}`"))
}

// These three run on the page's event loop, which is the point: the host never
// leaves it.
#[allow(clippy::future_not_send)]
async fn settle(returned: Result<JsValue, JsValue>, name: &str) -> Result<JsValue, String> {
    let promise: Promise = returned
        .map_err(|e| describe(&e))?
        .dyn_into()
        .map_err(|_| format!("`{name}` returned something other than a promise"))?;
    JsFuture::from(promise).await.map_err(|e| describe(&e))
}

#[allow(clippy::future_not_send)]
async fn connections(host: &JsValue) -> Result<HashMap<String, String>, String> {
    let returned = method(host, "connections")?.call0(host);
    let map = settle(returned, "connections").await?;
    serde_wasm_bindgen::from_value(map).map_err(|e| e.to_string())
}

#[allow(clippy::future_not_send)]
async fn credentials(
    host: &JsValue,
    scope: &Scope,
    access: Access,
) -> Result<Vec<StorageCredential>, String> {
    let scope = serde_wasm_bindgen::to_value(scope).map_err(|e| e.to_string())?;
    let access = serde_wasm_bindgen::to_value(&access).map_err(|e| e.to_string())?;
    let returned = method(host, "credentials")?.call2(host, &scope, &access);
    let vended = settle(returned, "credentials").await?;
    serde_wasm_bindgen::from_value(vended).map_err(|e| e.to_string())
}

fn describe(error: &JsValue) -> String {
    error
        .dyn_ref::<js_sys::Error>()
        .map_or_else(|| format!("{error:?}"), |e| String::from(e.message()))
}
