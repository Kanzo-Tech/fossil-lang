//! `fossil-storage-wasm` — [`fossil_storage`] exposed to JS.
//!
//! The `DuckDB` half is stateless: every call takes the credential as the host
//! vended it and parses it again, so nothing secret outlives the call on this
//! side of the boundary. The bytes half ([`storage_read`], [`storage_write`])
//! asks the JS `Host` itself, through [`JsHost`], and reaches storage through
//! `object_store`.

use std::sync::Arc;

use fossil_storage::{Access, Grant, JsHost, Scope, Storage, StorageCredential};
use js_sys::{Array, Object, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;

fn grant(credential: JsValue) -> Result<Grant, JsError> {
    let credential: StorageCredential = serde_wasm_bindgen::from_value(credential)?;
    Grant::try_from(credential).map_err(|e| JsError::new(&e.to_string()))
}

fn parse_access(access: &str) -> Result<Access, JsError> {
    match access {
        "read" => Ok(Access::Read),
        "write" => Ok(Access::Write),
        other => Err(JsError::new(&format!(
            "access is `read` or `write`, not {other:?}"
        ))),
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
pub fn storage_grant(credential: JsValue, access: &str) -> Result<JsValue, JsError> {
    let (grant, access) = (grant(credential)?, parse_access(access)?);
    #[allow(clippy::cast_precision_loss)]
    // milliseconds since 1970 fit in 2^53 until the year 287396
    let expires = grant
        .expires_at_ms()
        .map_or(JsValue::NULL, |ms| JsValue::from_f64(ms as f64));
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
    let name = grant
        .name(locator)
        .map_err(|e| JsError::new(&e.to_string()))?;
    let lend = grant
        .lend(locator)
        .map_err(|e| JsError::new(&e.to_string()))?;
    object(&[("name", JsValue::from_str(&name)), ("lend", opt(lend))])
}

/// `[{ ok: true, bytes } | { ok: false, reason }]` — each target's bytes, in
/// order, read with the credentials `host` vends: one `credentials` call per
/// connection named, not per file. A target with no connection is read only
/// when it is a public `http(s)` URL. A failure is the target's own answer and
/// never the batch's.
///
/// # Errors
/// A JS `Error` when `targets` is not an array of `{ locator, connection? }`.
#[wasm_bindgen(js_name = storageRead)]
pub async fn storage_read(host: JsValue, targets: JsValue) -> Result<JsValue, JsError> {
    #[derive(serde::Deserialize)]
    struct Target {
        locator: String,
        connection: Option<String>,
    }
    let targets: Vec<Target> = serde_wasm_bindgen::from_value(targets)?;
    let mut storage = Storage::new(Arc::new(JsHost::new(host)));
    let mut refused = Vec::with_capacity(targets.len());
    for target in &targets {
        refused.push(match &target.connection {
            Some(connection) => storage
                .grant(Scope::Connection(connection.clone()), Access::Read)
                .await
                .err()
                .map(|e| e.to_string()),
            None => storage
                .public(&target.locator)
                .err()
                .map(|_| "it names no connection and is not a public URL".to_string()),
        });
    }
    let out = Array::new();
    for (target, refused) in targets.iter().zip(refused) {
        let result = match refused {
            Some(reason) => Err(reason),
            None => storage
                .get(&target.locator)
                .await
                .map_err(|e| e.to_string()),
        };
        out.push(&match result {
            Ok(bytes) => object(&[
                ("ok", JsValue::TRUE),
                ("bytes", Uint8Array::from(bytes.as_ref()).into()),
            ])?,
            Err(reason) => object(&[
                ("ok", JsValue::FALSE),
                ("reason", JsValue::from_str(&reason)),
            ])?,
        });
    }
    Ok(out.into())
}

/// Write each `{ path, bytes }` under the one prefix `host` vends `write` on
/// for `scope`, and answer the prefix. A scope that vends several prefixes has
/// no single place to write to, and is refused.
///
/// # Errors
/// A JS `Error` when the host vends no single prefix, or a write fails.
#[wasm_bindgen(js_name = storageWrite)]
pub async fn storage_write(host: JsValue, scope: JsValue, files: Array) -> Result<String, JsError> {
    let scope: Scope = serde_wasm_bindgen::from_value(scope)?;
    let mut storage = Storage::new(Arc::new(JsHost::new(host)));
    let prefix = match storage.grant(scope.clone(), Access::Write).await {
        Ok([prefix]) => prefix.clone(),
        Ok(prefixes) => {
            return Err(JsError::new(&format!(
                "the host vended {} write credentials for {scope}; a write needs exactly one prefix",
                prefixes.len()
            )));
        }
        Err(e) => return Err(JsError::new(&e.to_string())),
    };
    for file in files.iter() {
        let path = Reflect::get(&file, &JsValue::from_str("path"))
            .ok()
            .and_then(|p| p.as_string())
            .ok_or_else(|| JsError::new("a file is `{ path: string, bytes: Uint8Array }`"))?;
        let bytes = Reflect::get(&file, &JsValue::from_str("bytes"))
            .ok()
            .and_then(|b| b.dyn_into::<Uint8Array>().ok())
            .ok_or_else(|| JsError::new("a file is `{ path: string, bytes: Uint8Array }`"))?;
        storage
            .put(&format!("{prefix}{path}"), bytes.to_vec().into())
            .await
            .map_err(|e| JsError::new(&e.to_string()))?;
    }
    Ok(prefix)
}
