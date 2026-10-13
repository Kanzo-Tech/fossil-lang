//! `Profiler` — [`fossil_profile::Profiler`] for a JS host: the plan in wasm,
//! each query run on the page's DuckDB-WASM by the host, and the answers fed
//! back. `@fossil-lang/profile` is the loop around it. And `mappingGrammar`:
//! what a model may write once the sources are profiled — `fossil_ide::constrain`.

use fossil_graph_schema::js::invalid_argument;
use fossil_profile::{Options, Rows, Source};
use wasm_bindgen::prelude::*;

use crate::to_value;

/// The plan over some sources: hand out a query, take its rows, until there
/// are none; then the profile.
#[wasm_bindgen(js_name = Profiler)]
#[derive(Debug)]
pub struct JsProfiler {
    plan: Option<fossil_profile::Profiler>,
}

fn argument<T: serde::de::DeserializeOwned>(
    name: &str,
    expected: &str,
    value: JsValue,
) -> Result<T, JsValue> {
    serde_wasm_bindgen::from_value(value)
        .map_err(|e| invalid_argument(name, expected, Some(e.into())).into())
}

#[wasm_bindgen(js_class = Profiler)]
impl JsProfiler {
    /// A plan over `sources`, as far as `options` says — the whole profile
    /// when it is absent.
    ///
    /// # Errors
    ///
    /// `api/invalid-argument` when `sources` or `options` is not their shape.
    #[wasm_bindgen(constructor)]
    pub fn new(
        #[wasm_bindgen(unchecked_param_type = "ProfileSource[]")] sources: JsValue,
        #[wasm_bindgen(unchecked_param_type = "ProfileOptions | undefined")] options: JsValue,
    ) -> Result<Self, JsValue> {
        let sources: Vec<Source> = argument("sources", "an array of ProfileSource", sources)?;
        let options: Options = if options.is_undefined() {
            Options::default()
        } else {
            argument("options", "ProfileOptions", options)?
        };
        Ok(Self {
            plan: Some(fossil_profile::Profiler::new(sources, options)),
        })
    }

    /// The next query to run, or `undefined` when none is to be handed out now:
    /// the profile is done, or the phase waits on answers still out.
    ///
    /// # Errors
    ///
    /// `internal/bug` after [`Self::finish`].
    #[wasm_bindgen(js_name = nextQuery, unchecked_return_type = "ProfileQuery | undefined")]
    pub fn next_query(&mut self) -> Result<JsValue, JsValue> {
        self.plan_mut()?.next_query().map_or_else(
            || Ok(JsValue::UNDEFINED),
            |query| to_value("the query", &query),
        )
    }

    /// The rows query `id` answered, every cell its text or `null`.
    ///
    /// # Errors
    ///
    /// `api/invalid-argument` when `rows` is not an array of arrays of text or
    /// `null`; `internal/bug` after [`Self::finish`].
    pub fn feed(
        &mut self,
        id: u32,
        #[wasm_bindgen(unchecked_param_type = "(string | null)[][]")] rows: JsValue,
    ) -> Result<(), JsValue> {
        let rows: Rows = argument(
            "rows",
            "an array of rows, each an array of text or null",
            rows,
        )?;
        self.plan_mut()?.feed(id, &rows);
        Ok(())
    }

    /// Query `id` could not be run, and `reason` is what the engine said.
    ///
    /// # Errors
    ///
    /// `internal/bug` after [`Self::finish`].
    pub fn fail(&mut self, id: u32, reason: &str) -> Result<(), JsValue> {
        self.plan_mut()?.fail(id, reason);
        Ok(())
    }

    /// The profile; the plan is spent.
    ///
    /// # Errors
    ///
    /// `internal/bug` when it was already finished.
    #[wasm_bindgen(unchecked_return_type = "Profile")]
    pub fn finish(&mut self) -> Result<JsValue, JsValue> {
        let plan = self.plan.take().ok_or_else(spent)?;
        to_value("the profile", &plan.finish())
    }
}

impl JsProfiler {
    fn plan_mut(&mut self) -> Result<&mut fossil_profile::Profiler, JsValue> {
        self.plan.as_mut().ok_or_else(spent)
    }
}

fn spent() -> JsValue {
    fossil_graph_schema::js::bug("the profiler was used after `finish`", JsValue::UNDEFINED).into()
}

/// What a model may write over `vocabulary`: the `prelude` the request already says (the shapes
/// imported, the sources bound) and the `grammar` of the mappings after it, as the EBNF a
/// constrained decoder takes — every name in it one that exists (`fossil_ide::constrain`).
///
/// # Errors
///
/// `api/invalid-argument` when `vocabulary` is not a `MappingVocabulary`.
#[wasm_bindgen(js_name = mappingGrammar, unchecked_return_type = "{ prelude: string; grammar: string }")]
pub fn mapping_grammar(
    #[wasm_bindgen(unchecked_param_type = "MappingVocabulary")] vocabulary: JsValue,
) -> Result<JsValue, JsValue> {
    #[derive(serde::Serialize)]
    struct Constrained {
        prelude: String,
        grammar: String,
    }
    let vocabulary: fossil_ide::constrain::Vocabulary =
        argument("vocabulary", "a MappingVocabulary", vocabulary)?;
    to_value(
        "the grammar",
        &Constrained {
            prelude: fossil_ide::constrain::prelude(&vocabulary),
            grammar: fossil_ide::constrain::mapping_grammar(&vocabulary).ebnf(),
        },
    )
}
