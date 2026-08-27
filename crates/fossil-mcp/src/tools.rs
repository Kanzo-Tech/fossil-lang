//! The six typed tools, and the one field that decides whether there are six.
//!
//! # Why six tools and not one
//!
//! This registered **one** tool, `dispatch_verb`, taking `{ verb, params }` as
//! opaque JSON. It was the minimal surface that was still the whole service,
//! and it was honest about being a placeholder. Its cost is paid by the model
//! on the other end: one tool whose input schema is a `oneOf` over six
//! unrelated shapes, so every call has to be assembled by first choosing a
//! discriminant and then reconstructing the params for it, and the six
//! descriptions arrive as one paragraph whether the caller wanted `schema` or
//! `path`.
//!
//! Six tools, one per [`Verb`], put six contracts in front of the caller. A
//! client that filters tools filters verbs. A client that shows descriptions
//! shows the right one. And a malformed `read` is rejected against `read`'s
//! schema at the protocol boundary rather than inside our handler.
//!
//! Nothing here holds a second copy of the surface: the names, the prose and
//! the params schemas all come off [`Verb`], which is
//! `crates/fossil-graph/src/operations/mod.rs` and is the same artefact the
//! snapshot tests publish.
//!
//! # Hiding the escape hatch is now literal
//!
//! `execute_sql` was always documented as a verb "a binding MAY hide behind a
//! permission", and with one generic tool that was not something a binding
//! could actually do — a tool cannot be half-registered. With six, hiding it
//! means not registering it, and [`SqlPolicy`] is where that is decided.
//!
//! **It is one field, and it has two consequences on purpose.** [`Self::tools`]
//! drops `execute_sql` from the list, and [`Self::operation`] passes no
//! [`RawSqlAccess`] — which is what closes `read`'s `where`, because that field
//! carries the same authority (`fossil_graph::operations::raw_sql`). There is
//! no second knob to set inconsistently: a surface that lists the hatch admits
//! the predicate, and one that hides it refuses both.

use std::sync::Arc;

use fossil_graph::{Operation, RawSqlAccess, Verb};
use rmcp::model::{JsonObject, Tool};

/// Whether this server puts a caller's SQL in front of the engine.
///
/// The default is [`Self::Withheld`], and the asymmetry is deliberate: the five
/// bounded verbs cost a function of the answer, and `execute_sql` costs a
/// function of whatever was typed. A deployment that wants the hatch says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SqlPolicy {
    /// No `execute_sql` tool, and `read` refuses a `where`.
    #[default]
    Withheld,
    /// Both doors. The caller reaches the engine.
    Allowed,
}

impl SqlPolicy {
    /// The permission token, or nothing. This is the only place the policy
    /// becomes a token, which is what keeps the two consequences in step.
    const fn access(self) -> Option<RawSqlAccess> {
        match self {
            Self::Allowed => Some(RawSqlAccess::granted()),
            Self::Withheld => None,
        }
    }
}

/// The tool surface: which verbs this server publishes, and how a tool call
/// becomes an [`Operation`].
#[derive(Debug, Clone, Copy, Default)]
pub struct VerbSurface {
    policy: SqlPolicy,
}

impl VerbSurface {
    #[must_use]
    pub const fn new(policy: SqlPolicy) -> Self {
        Self { policy }
    }

    /// What this server publishes: one tool per verb it admits.
    ///
    /// Six under [`SqlPolicy::Allowed`], five under [`SqlPolicy::Withheld`].
    #[must_use]
    pub fn tools(self) -> Vec<Tool> {
        self.verbs().map(Self::tool_for).collect()
    }

    /// The verbs this policy admits, in catalogue order.
    fn verbs(self) -> impl Iterator<Item = Verb> {
        let policy = self.policy;
        Verb::ALL
            .into_iter()
            .filter(move |v| *v != Verb::ExecuteSql || policy == SqlPolicy::Allowed)
    }

    /// One [`Verb`] as an MCP tool. The name is the wire name, so a tool call
    /// and a `{ verb, params }` envelope are the same string.
    fn tool_for(verb: Verb) -> Tool {
        // `params_schema` is a JSON Schema document and therefore an object;
        // the MCP model wants the map rather than the `Value` around it.
        let schema: JsonObject = match verb.params_schema() {
            serde_json::Value::Object(map) => map,
            other => unreachable!("a params schema is an object, got {other}"),
        };
        Tool::new(verb.name(), verb.description(), Arc::new(schema))
    }

    /// Turn a tool call into an [`Operation`], applying the policy.
    ///
    /// A verb this surface does not publish is reported as an unknown tool
    /// rather than as a refusal — it is not in the list the caller was given,
    /// so "no such tool" is the truthful answer and it leaks nothing about the
    /// deployment.
    ///
    /// # Errors
    ///
    /// A message for the caller: an unknown tool, params that do not parse, or
    /// a `read` reaching for a `where` this policy withholds.
    pub fn operation(
        self,
        name: &str,
        arguments: Option<JsonObject>,
    ) -> std::result::Result<Operation, String> {
        let verb = Verb::from_name(name)
            .filter(|v| self.verbs().any(|admitted| admitted == *v))
            .ok_or_else(|| format!("unknown tool `{name}`"))?;
        let params = serde_json::Value::Object(arguments.unwrap_or_default());
        verb.parse_params(params, self.policy.access())
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(surface: VerbSurface) -> Vec<String> {
        surface
            .tools()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect()
    }

    #[test]
    fn six_tools_when_the_hatch_is_open_and_five_when_it_is_not() {
        assert_eq!(
            names(VerbSurface::new(SqlPolicy::Allowed)),
            [
                "schema",
                "read",
                "expand",
                "path",
                "aggregate",
                "execute_sql"
            ],
        );
        assert_eq!(
            names(VerbSurface::new(SqlPolicy::Withheld)),
            ["schema", "read", "expand", "path", "aggregate"],
        );
    }

    /// **Every tool publishes its verb's own schema, and none publishes a
    /// `oneOf` over the other five.**
    ///
    /// This is the whole difference from `dispatch_verb`, so it is asserted
    /// rather than described: each input schema is an object whose required
    /// properties are that verb's, and `read`'s does not mention `sql`.
    #[test]
    fn each_tool_carries_its_own_contract() {
        for tool in VerbSurface::new(SqlPolicy::Allowed).tools() {
            let schema = serde_json::to_value(&*tool.input_schema).expect("a schema");
            assert_eq!(schema["type"], "object", "{} is not an object", tool.name);
            assert!(
                schema.get("oneOf").is_none(),
                "{} publishes a discriminated union, which is the thing this replaced",
                tool.name,
            );
            assert!(
                tool.description.as_deref().is_some_and(|d| !d.is_empty()),
                "{} has no description",
                tool.name,
            );
        }
        let read = VerbSurface::new(SqlPolicy::Allowed)
            .tools()
            .into_iter()
            .find(|t| t.name == "read")
            .expect("read is published");
        let schema = serde_json::to_value(&*read.input_schema).expect("a schema");
        let props = schema["properties"].as_object().expect("properties");
        assert!(props.contains_key("vertex_type") && props.contains_key("where"));
        assert!(!props.contains_key("sql"), "read is not execute_sql");
    }

    /// **One field, both consequences.** The surface that hides the hatch also
    /// refuses the predicate that carries the same authority, and there is no
    /// way to configure one without the other because there is one field.
    #[test]
    fn withholding_the_hatch_withholds_the_predicate_too() {
        let closed = VerbSurface::new(SqlPolicy::Withheld);
        let open = VerbSurface::new(SqlPolicy::Allowed);

        let read_where = |v: &str| {
            let mut m = JsonObject::new();
            m.insert("vertex_type".into(), "Person".into());
            m.insert("where".into(), v.into());
            Some(m)
        };
        let plain = || {
            let mut m = JsonObject::new();
            m.insert("vertex_type".into(), "Person".into());
            Some(m)
        };
        let sql = || {
            let mut m = JsonObject::new();
            m.insert("sql".into(), "SELECT 1".into());
            Some(m)
        };

        // Closed: the bounded read still works; both raw-SQL doors are shut,
        // and the hatch is shut by not existing.
        assert!(closed.operation("read", plain()).is_ok());
        let refused = closed
            .operation("read", read_where("age > 30"))
            .expect_err("where is raw SQL");
        assert!(refused.contains("read.where"), "got: {refused}");
        let missing = closed
            .operation("execute_sql", sql())
            .expect_err("the hatch is not registered");
        assert!(missing.contains("unknown tool"), "got: {missing}");

        // Open: both.
        assert!(open.operation("read", read_where("age > 30")).is_ok());
        assert!(open.operation("execute_sql", sql()).is_ok());
    }

    #[test]
    fn a_tool_name_outside_the_six_is_unknown() {
        let err = VerbSurface::default()
            .operation("materialize_graph", None)
            .expect_err("not a verb");
        assert!(err.contains("unknown tool"), "got: {err}");
    }

    /// Params that do not fit the verb's schema fail here rather than at the
    /// engine — `deny_unknown_fields` is on every params struct.
    #[test]
    fn params_are_checked_against_the_verbs_own_shape() {
        let mut m = JsonObject::new();
        m.insert("vertex_type".into(), "Person".into());
        m.insert("colour".into(), "blue".into());
        let err = VerbSurface::default()
            .operation("read", Some(m))
            .expect_err("unknown field");
        assert!(err.contains("colour"), "got: {err}");
    }
}
