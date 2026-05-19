//! Extension trait widening [`fossil_base::System`] with an output-descriptor
//! accessor.
//!
//! # Why an extension trait (not a method on `System`)
//!
//! Cycle-safety. Putting `output_descriptor_kind() -> &OutputDescriptorKind`
//! on `fossil-base::System` would force `fossil-base` to know about
//! `OutputDescriptorKind`, inverting the natural dependency direction
//! (`fossil-descriptors-output -> fossil-base`). Plan 03-03 Task 2 step 3
//! mandates Option B unconditionally — see ADR-0006 for the full rationale.
//!
//! `fossil-base::System` stays UNCHANGED; the descriptor type catalogue
//! lives entirely in `fossil-descriptors-output`.
//!
//! # Wiring contract
//!
//! Hosts (`fossil-cli`, `fossil-wasm`) implement BOTH
//! [`fossil_base::System`] AND [`SystemWithDescriptors`] on their concrete
//! `System` struct (e.g. `CliSystem`, `WasmSystem`). Callers
//! (`fossil-hir`'s `typecheck_mapping` in plan 03-05) reach the descriptor
//! via:
//!
//! ```ignore
//! use fossil_descriptors_output::SystemWithDescriptors;
//! // `db.system()` returns `&dyn fossil_base::System`; the extension
//! // method `output_descriptor_kind()` is resolved against that trait
//! // object IFF a `SystemWithDescriptors` impl exists for the concrete
//! // type behind the trait object — which it does for the host systems.
//! // But because `system()` returns `&dyn fossil_base::System`, callers
//! // need a generic-over-`System` wrapper to invoke the extension. The
//! // simplest path: hosts construct a typed accessor in their own crate
//! // and expose it via the host-specific `Db` impl. Plan 03-05 wires this.
//! ```
//!
//! Why we deliberately do NOT use a blanket
//! `impl<T: fossil_base::System> SystemWithDescriptors for T {}` — that
//! would lock every `System` impl to the `AcceptAll` default; hosts that
//! load a `ShEx` schema (Phase 6+) need to override.

use crate::OutputDescriptorKind;

/// Extension trait — `output_descriptor_kind()` accessor for hosts.
///
/// Default impl returns [`OutputDescriptorKind::ACCEPT_ALL_DEFAULT`]. Hosts
/// that load a `ShEx` schema override this method to return their
/// `Self::ShEx(ShExDescriptor)` variant.
pub trait SystemWithDescriptors: fossil_base::System {
    /// Return the host's current output descriptor.
    ///
    /// Default: `&OutputDescriptorKind::ACCEPT_ALL_DEFAULT` (a static const,
    /// so the borrow is `'static`). Hosts override this method to return a
    /// reference into their own storage (an `Arc<OutputDescriptorKind>`
    /// field on the host's `System` struct, typically).
    fn output_descriptor_kind(&self) -> &OutputDescriptorKind {
        &OutputDescriptorKind::ACCEPT_ALL_DEFAULT
    }
}
