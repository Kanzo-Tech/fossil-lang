//! `fossil-storage-wasm` — [`fossil_storage`] exposed to JS.
//!
//! The `DuckDB` half is stateless: every call takes the credential as the host
//! vended it and parses it again, so nothing secret outlives the call on this
//! side of the boundary. The bytes half ([`storage_read`]) asks the JS `Host`
//! itself, through [`JsHost`], and reaches storage through
//! `object_store`.

// `result_large_err`: the helpers here refuse with `fossil_graph_schema::Failure`
// (144 bytes, over clippy's 128) on the path that ends the call, where the copy
// costs nothing measurable — `fossil-df`'s crate root says the same.
#![allow(clippy::result_large_err)]

use std::sync::Arc;

use fossil_graph_schema::Failure;
use fossil_graph_schema::js::{bug, invalid_argument, to_wire};
use fossil_storage::{Access, Grant, JsHost, Scope, Storage, StorageCredential};
use js_sys::{Array, Object, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;

/// Install the panic hook when the module boots, so a panic's message and trace
/// reach the console beside the `RuntimeError` that `@fossil-lang/storage`
/// reports as `internal/bug`.
#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

fn grant(credential: JsValue) -> Result<Grant, Failure> {
    let credential: StorageCredential = serde_wasm_bindgen::from_value(credential)
        .map_err(|e| invalid_argument("credential", "a StorageCredential", Some(e.into())))?;
    Ok(Grant::try_from(credential)?)
}

fn parse_access(access: &str) -> Result<Access, Failure> {
    match access {
        "read" => Ok(Access::Read),
        "write" => Ok(Access::Write),
        _ => Err(invalid_argument("access", "`read` or `write`", None)),
    }
}

fn object(fields: &[(&str, JsValue)]) -> Result<JsValue, Failure> {
    let obj = Object::new();
    for (key, value) in fields {
        Reflect::set(&obj, &JsValue::from_str(key), value)
            .map_err(|e| bug(format!("setting `{key}` on a fresh object"), e))?;
    }
    Ok(obj.into())
}

fn opt(value: Option<String>) -> JsValue {
    value.map_or(JsValue::NULL, |v| JsValue::from_str(&v))
}

/// `{ prefix, install, uninstall, expiresAtMs }` — the statements that put the
/// credential in the engine and take it out (`null` for a store the engine is
/// lent file by file), and when it stops working (`null` when unsaid).
///
/// # Errors
/// A `FossilError`: `api/invalid-argument` for an unknown `access` or a
/// credential that is not one, the credential's own code for one fossil does not read.
#[wasm_bindgen(js_name = storageGrant)]
pub fn storage_grant(credential: JsValue, access: &str) -> Result<JsValue, JsValue> {
    let (grant, access) = (grant(credential)?, parse_access(access)?);
    #[allow(clippy::cast_precision_loss)]
    // milliseconds since 1970 fit in 2^53 until the year 287396
    let expires = grant
        .expires_at_ms()
        .map_or(JsValue::NULL, |ms| JsValue::from_f64(ms as f64));
    Ok(object(&[
        ("prefix", JsValue::from_str(grant.prefix())),
        ("install", opt(grant.install_sql(access))),
        ("uninstall", opt(grant.uninstall_sql(access))),
        ("expiresAtMs", expires),
    ])?)
}

/// `{ name, lend }` — what SQL calls `locator`, and the URL the engine lends
/// that name to (`null` when the name is readable as it is).
///
/// # Errors
/// A `FossilError` — `storage/outside-prefix` when `locator` lies outside the
/// credential's prefix.
#[wasm_bindgen(js_name = storageName)]
pub fn storage_name(credential: JsValue, locator: &str) -> Result<JsValue, JsValue> {
    let grant = grant(credential)?;
    let name = grant.name(locator).map_err(Failure::from)?;
    let lend = grant.lend(locator).map_err(Failure::from)?;
    Ok(object(&[
        ("name", JsValue::from_str(&name)),
        ("lend", opt(lend)),
    ])?)
}

/// `[{ ok: true, bytes } | { ok: false, problem }]` — each target's bytes, in
/// order, read with the credentials `host` vends: one `credentials` call per
/// connection named, not per file. A target with no connection is read only
/// when it is a public `http(s)` URL. A failure is the target's own answer and
/// never the batch's, and `problem` is the wire object a thrown `FossilError`
/// carries.
///
/// # Errors
/// A `FossilError`, `api/invalid-argument`, when `targets` is not an array of
/// `{ locator, connection? }`.
#[wasm_bindgen(js_name = storageRead)]
pub async fn storage_read(host: JsValue, targets: JsValue) -> Result<JsValue, JsValue> {
    #[derive(serde::Deserialize)]
    struct Target {
        locator: String,
        connection: Option<String>,
    }
    let targets: Vec<Target> = serde_wasm_bindgen::from_value(targets).map_err(|e| {
        invalid_argument(
            "targets",
            "an array of { locator, connection? }",
            Some(e.into()),
        )
    })?;
    let mut storage = Storage::new(Arc::new(JsHost::new(host)));
    let mut refused = Vec::with_capacity(targets.len());
    for target in &targets {
        refused.push(match &target.connection {
            Some(connection) => storage
                .grant(Scope::Connection(connection.clone()), Access::Read)
                .await
                .err()
                .map(Failure::from),
            None => storage.public(&target.locator).err().map(Failure::from),
        });
    }
    let out = Array::new();
    for (target, refused) in targets.iter().zip(refused) {
        let result = match refused {
            Some(failure) => Err(failure),
            None => storage.get(&target.locator).await.map_err(Failure::from),
        };
        out.push(&match result {
            Ok(bytes) => object(&[
                ("ok", JsValue::TRUE),
                ("bytes", Uint8Array::from(bytes.as_ref()).into()),
            ])?,
            Err(failure) => object(&[("ok", JsValue::FALSE), ("problem", to_wire(&failure))])?,
        });
    }
    Ok(out.into())
}
