/**
 * `@fossil-lang/types/internal` — what the `@fossil-lang/*` packages share with each other and a
 * host never calls. **Siblings only, and no semver promise**: a name here changes in any release.
 *
 * It is an `exports` subpath rather than `stripInternal`, because the siblings typecheck against
 * the emitted `.d.ts` and a stripped name would not be there for them either.
 */
export type { DocumentWorkspace, UnreadDocument } from './host.js';
export { diagnosticOf, type DiagnosticBase } from './diagnostic.js';
export type { GrantPlan, LocationName, MissingDocument } from './wire.gen.js';
export { RENEW_BEFORE_MS } from './wire.gen.js';
export { attachCause, type Related } from './error.js';
export { MODULE_MS, boot, loader, until, within } from './deadline.js';
