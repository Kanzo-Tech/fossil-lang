//! **The boundary** — a [`Failure`] as the JavaScript `FossilError` every wasm
//! crate throws, and a JavaScript rejection as a [`Foreign`] cause.
//!
//! One function per direction, and the three wasm crates call these and
//! nothing else: `/docs/design/errors#the-boundary-one-shape-and-a-real-error`.
//! The error is built from the wire object ([`Failure`]'s `Serialize`), the
//! same object `FossilError.from` in `@fossil-lang/types` rebuilds one from, so
//! the two sides cannot build different errors.
//!
//! Every function here calls wasm-bindgen intrinsics, which panic on a native
//! target.

use js_sys::{Error, JSON, Object, Reflect};
use wasm_bindgen::{JsCast, JsValue};

use crate::{Failure, Foreign, Problem};

/// `failure` as a thrown value: an `Error` whose `name` is `FossilError` and
/// whose `message` is the detail, carrying `code`, `data`, `title`, `help`,
/// `related`, `problem` — the whole wire object, as plain data — and `cause`:
/// a nested `FossilError` for a cause fossil raised, an `Error` of the cause's
/// own name — with its own `code` and `data` when it carried them — for one it
/// did not.
#[must_use]
pub fn to_js(failure: &Failure) -> JsValue {
    from_wire(&to_wire(failure)).into()
}

/// `failure`'s wire object as plain data — what a call that answers a failure
/// rather than throwing it (`storageRead`'s `{ ok: false, problem }`) hands
/// over, and what `FossilError.from` rebuilds the error from.
#[must_use]
pub fn to_wire(failure: &Failure) -> JsValue {
    // Serialising a failure fossil built cannot fail: every field is a string,
    // a number, a list of those or a struct of those, and every map key a
    // string. `JSON.parse` of what `serde_json` wrote cannot fail either.
    let parse = |failure: &Failure| {
        serde_json::to_string(failure)
            .ok()
            .and_then(|text| JSON::parse(&text).ok())
    };
    parse(failure)
        .or_else(|| {
            parse(&Failure::new(Problem::Bug {
                what: format!("the failure `{}` did not serialise", failure.problem.code()),
            }))
        })
        .unwrap_or(JsValue::NULL)
}

impl From<Failure> for JsValue {
    fn from(failure: Failure) -> Self {
        to_js(&failure)
    }
}

/// A JavaScript rejection as a cause: its own `name` and `message`, and — off
/// any object, an `Error` or not — its own `code` when that is `area/kind`, with
/// its `data` when that is a JSON object. A `JsValue` is not `Send`, so the
/// object itself cannot ride the cause; what a host needs back is copied.
#[must_use]
pub fn foreign(error: &JsValue) -> Foreign {
    let named = error.dyn_ref::<Error>().map_or_else(
        || {
            Foreign::named(
                "Error",
                error.as_string().unwrap_or_else(|| format!("{error:?}")),
            )
        },
        |e| Foreign::named(String::from(e.name()), String::from(e.message())),
    );
    if !error.is_object() {
        return named;
    }
    match get(error, "code").as_string() {
        Some(code) => named.coded(code, data(&get(error, "data"))),
        None => named,
    }
}

/// `value` as a JSON object, or nothing: an array, a primitive, or an object
/// `JSON.stringify` refuses (a cycle, a `BigInt`) is not kept.
fn data(value: &JsValue) -> Option<serde_json::Map<String, serde_json::Value>> {
    if !value.is_object() || js_sys::Array::is_array(value) {
        return None;
    }
    let text = JSON::stringify(value).ok()?.as_string()?;
    serde_json::from_str(&text).ok()
}

fn get(object: &JsValue, key: &str) -> JsValue {
    Reflect::get(object, &JsValue::from_str(key)).unwrap_or(JsValue::UNDEFINED)
}

/// Own properties on an `Error` this module just made. `Reflect.set` on a
/// fresh, extensible object has nothing to refuse, so its answer is not read.
fn put(object: &Object, key: &str, value: &JsValue) {
    if !value.is_undefined() {
        let _ = Reflect::set(object, &JsValue::from_str(key), value);
    }
}

fn from_wire(problem: &JsValue) -> Error {
    let error = Error::new(&get(problem, "detail").as_string().unwrap_or_default());
    error.set_name("FossilError");
    for key in ["code", "data", "title", "severity", "help", "related"] {
        put(&error, key, &get(problem, key));
    }
    put(&error, "problem", problem);
    let cause = get(problem, "cause");
    if cause.is_object() {
        // A foreign cause always has a `name` and a problem never does: a host's
        // own `code` — even one that spells a fossil code — keeps it foreign.
        let cause: JsValue = get(&cause, "name").as_string().map_or_else(
            || from_wire(&cause).into(),
            |name| {
                let foreign = Error::new(&get(&cause, "detail").as_string().unwrap_or_default());
                foreign.set_name(&name);
                put(&foreign, "code", &get(&cause, "code"));
                put(&foreign, "data", &get(&cause, "data"));
                foreign.into()
            },
        );
        put(&error, "cause", &cause);
    }
    error
}

/// A value fossil built that JavaScript would not take — a serialiser or a
/// `Reflect.set` refusing it — as `internal/bug`, the refusal as its cause.
#[must_use]
pub fn bug(what: impl Into<String>, error: impl Into<JsValue>) -> Failure {
    Failure::new(Problem::Bug { what: what.into() }).caused_by(foreign(&error.into()))
}

/// An argument a caller passed that is not what the call takes, as
/// `api/invalid-argument`, with JavaScript's own refusal as the cause when
/// there is one.
#[must_use]
pub fn invalid_argument(
    argument: impl Into<String>,
    expected: impl Into<String>,
    error: Option<JsValue>,
) -> Failure {
    let failure = Failure::new(Problem::InvalidArgument {
        argument: argument.into(),
        expected: expected.into(),
    });
    match error {
        Some(error) => failure.caused_by(foreign(&error)),
        None => failure,
    }
}
