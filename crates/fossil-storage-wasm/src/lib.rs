//! `fossil-storage-wasm` — [`fossil_storage::Grant`] exposed to JS, stateless:
//! every call takes the credential as the host vended it and parses it again,
//! so nothing secret outlives the call on this side of the boundary.

use fossil_storage::{Access, Grant, StorageCredential};
use js_sys::{Object, Reflect};
use wasm_bindgen::prelude::*;

fn grant(credential: JsValue) -> Result<Grant, JsError> {
    let credential: StorageCredential = serde_wasm_bindgen::from_value(credential)?;
    Grant::try_from(credential).map_err(|e| JsError::new(&e.to_string()))
}

fn access(access: &str) -> Result<Access, JsError> {
    match access {
        "read" => Ok(Access::Read),
        "write" => Ok(Access::Write),
        other => Err(JsError::new(&format!("access is `read` or `write`, not {other:?}"))),
    }
}

fn object(fields: &[(&str, JsValue)]) -> Result<JsValue, JsError> {
    let obj = Object::new();
    for (key, value) in fields {
        Reflect::set(&obj, &JsValue::from_str(key), value)
            .map_err(|_| JsError::new("setting a field on a fresh object"))?;
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
/// A JS `Error` for a credential fossil does not read, or an unknown `access`.
#[wasm_bindgen(js_name = storageGrant)]
pub fn storage_grant(credential: JsValue, access_: &str) -> Result<JsValue, JsError> {
    let (grant, access) = (grant(credential)?, access(access_)?);
    #[allow(clippy::cast_precision_loss)] // milliseconds since 1970 fit in 2^53 until the year 287396
    let expires = grant.expires_at_ms().map_or(JsValue::NULL, |ms| JsValue::from_f64(ms as f64));
    object(&[
        ("prefix", JsValue::from_str(grant.prefix())),
        ("install", opt(grant.install_sql(access))),
        ("uninstall", opt(grant.uninstall_sql(access))),
        ("expiresAtMs", expires),
    ])
}

/// `{ name, lend }` — what SQL calls `locator`, and the URL the engine lends
/// that name to (`null` when the name is readable as it is).
///
/// # Errors
/// A JS `Error` when `locator` lies outside the credential's prefix.
#[wasm_bindgen(js_name = storageName)]
pub fn storage_name(credential: JsValue, locator: &str) -> Result<JsValue, JsError> {
    let grant = grant(credential)?;
    let name = grant.name(locator).map_err(|e| JsError::new(&e.to_string()))?;
    let lend = grant.lend(locator).map_err(|e| JsError::new(&e.to_string()))?;
    object(&[("name", JsValue::from_str(&name)), ("lend", opt(lend))])
}

/// `{ url, headers }` — one `method` request on `locator`, signed at `now_ms`.
///
/// # Errors
/// A JS `Error` when `locator` lies outside the credential's prefix.
#[wasm_bindgen(js_name = storageSign)]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // `Date.now()` is a non-negative integer
pub fn storage_sign(
    credential: JsValue,
    method: &str,
    locator: &str,
    now_ms: f64,
) -> Result<JsValue, JsError> {
    let signed = grant(credential)?
        .sign(method, locator, now_ms as u64)
        .map_err(|e| JsError::new(&e.to_string()))?;
    let headers = Object::new();
    for (key, value) in &signed.headers {
        Reflect::set(&headers, &JsValue::from_str(key), &JsValue::from_str(value))
            .map_err(|_| JsError::new("setting a header on a fresh object"))?;
    }
    object(&[("url", JsValue::from_str(&signed.url)), ("headers", headers.into())])
}
