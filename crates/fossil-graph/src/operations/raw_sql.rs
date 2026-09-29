//! The permission that puts a caller's SQL in front of the engine.
//!
//! [`ExecuteSqlParams::sql`](super::sql::ExecuteSqlParams::sql) reaches the
//! engine **unparsed**. It is not a `String`: it is [`RawSql`], which has **no
//! `Deserialize` impl** and one constructor taking a [`RawSqlAccess`] token.
//! That takes the envelope with it — neither
//! [`ExecuteSqlParams`](super::sql::ExecuteSqlParams) nor
//! [`Operation`](super::Operation) can derive `Deserialize`, and the one way in
//! from the wire is [`Operation::from_wire`](super::Operation::from_wire):
//!
//! ```text
//! from_wire(value: &Value, sql: Option<RawSqlAccess>) -> Result<Operation>
//! ```
//!
//! `None` closes `execute_sql`; `Some` opens it. A binding cannot parse the
//! verb without having taken the decision.
//!
//! This held two fields together while `read` carried a `where` of the same
//! authority; `read` is gone, and the token still does the other half of its
//! job.
//!
//! # What it does NOT claim
//!
//! It does not stop a binding from granting access — `RawSqlAccess::granted()`
//! is public and any binding may call it. Withholding SQL is a policy, and a
//! library cannot hold a policy for its consumers.
//!
//! It is also not a sanitiser. [`RawSql`] does not parse, validate or escape
//! anything; it records that a caller with the permission supplied it. The
//! reason there is nothing to sanitise towards is
//! [the privacy argument](https://fossil-lang.org/docs/design/privacy): a
//! corpus is files and a recipient holds them, so a read-time gate has no
//! chokepoint to stand on. This is about which *bindings* expose the engine,
//! not about making the engine safe.

use serde::Serialize;

/// Permission to put a caller's SQL in front of the engine.
///
/// A binding constructs one when its policy allows raw SQL and passes it to
/// [`Operation::from_wire`](super::Operation::from_wire); it passes `None`
/// when the policy does not. The token is deliberately trivial — it carries no
/// data and costs nothing at run time. Its whole job is to make the decision
/// a value a caller has to hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawSqlAccess(());

impl RawSqlAccess {
    /// Grant it. Named rather than `new` because the call site is a policy
    /// decision and should read like one.
    #[must_use]
    pub const fn granted() -> Self {
        Self(())
    }
}

/// A fragment of SQL supplied by a caller, reaching the engine unparsed.
///
/// `Serialize` (the wire form is the bare string it always was) and
/// `JsonSchema` (`{"type": "string"}`, so the published contract is unchanged),
/// but **deliberately not `Deserialize`**: that absence is what forces every
/// wire entry point through [`Operation::from_wire`](super::Operation::from_wire)
/// and its permission argument. See the [module docs](self).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(transparent)]
#[schemars(transparent)]
pub struct RawSql(String);

impl RawSql {
    /// Wrap a fragment. The token is the point: there is no way to reach this
    /// constructor without having made the permission decision.
    ///
    /// The `access` parameter is consumed by value and unused — it is a
    /// witness, not an input.
    #[must_use]
    pub fn new(access: RawSqlAccess, sql: impl Into<String>) -> Self {
        let RawSqlAccess(()) = access;
        Self(sql.into())
    }

    /// The fragment, for the executor that splices it into a statement.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RawSql {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
