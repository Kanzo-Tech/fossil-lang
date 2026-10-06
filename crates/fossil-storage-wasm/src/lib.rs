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
use fossil_storage::{Access, Grant, JsHost, Storage, StorageCredential};
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

fn access(access: JsValue) -> Result<Access, Failure> {
    serde_wasm_bindgen::from_value(access)
        .map_err(|e| invalid_argument("access", "`read` or `write`", Some(e.into())))
}

fn object(fields: &[(&str, JsValue)]) -> Result<JsValue, Failure> {
    let obj = Object::new();
    for (key, value) in fields {
        Reflect::set(&obj, &JsValue::from_str(key), value)
            .map_err(|e| bug(format!("setting `{key}` on a fresh object"), e))?;
    }
    Ok(obj.into())
}

// The types the signatures below name, from the one place they are declared.
#[wasm_bindgen(typescript_custom_section)]
const WIRE_TYPES: &str =
    "import type { Access, GrantPlan, LocatorName } from '@fossil-lang/types';";

fn to_js(value: &impl serde::Serialize) -> Result<JsValue, Failure> {
    serde_wasm_bindgen::to_value(value).map_err(|e| bug("serialising an answer", e))
}

/// The credential's [`fossil_storage::GrantPlan`] for `access`.
///
/// # Errors
/// A `FossilError`: `api/invalid-argument` for an unknown `access` or a
/// credential that is not one, the credential's own code for one fossil does not read.
#[wasm_bindgen(js_name = storageGrant, unchecked_return_type = "GrantPlan")]
pub fn storage_grant(
    credential: JsValue,
    #[wasm_bindgen(unchecked_param_type = "Access")] access: JsValue,
) -> Result<JsValue, JsValue> {
    let (grant, access) = (grant(credential)?, self::access(access)?);
    Ok(to_js(&grant.plan(access))?)
}

/// The [`fossil_storage::LocatorName`] of `locator` under the credential.
///
/// # Errors
/// A `FossilError` — `storage/outside-prefix` when `locator` lies outside the
/// credential's prefix.
#[wasm_bindgen(js_name = storageName, unchecked_return_type = "LocatorName")]
pub fn storage_name(credential: JsValue, locator: &str) -> Result<JsValue, JsValue> {
    let named = grant(credential)?
        .locator_name(locator)
        .map_err(Failure::from)?;
    Ok(to_js(&named)?)
}

/// The prefix, of `prefixes`, whose credential covers `locator` — the longest one
/// covering it, by [`fossil_storage::covering`]; `undefined` when none does.
#[wasm_bindgen(js_name = storageCovering)]
#[must_use]
pub fn storage_covering(prefixes: Vec<String>, locator: &str) -> Option<String> {
    fossil_storage::covering(prefixes, String::as_str, locator)
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
        refused.push(
            storage
                .route(&target.locator, target.connection.as_deref())
                .await
                .err()
                .map(Failure::from),
        );
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
