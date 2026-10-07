// The public surface: the host contract, the engine, the error, the wire rows and `referenceTo`.
// What siblings share and a host never calls is `./internal`, with no semver promise.
export type { Host, HostCall } from './host.js';
export type { Engine, Table, Vector } from './engine.js';
export type { Diagnostic } from './diagnostic.js';
export type {
  Access,
  CompletionItem,
  CompletionKind,
  DiagnosticRelatedInformation,
  EdgeDrops,
  Format,
  FormatKind,
  Hover,
  InferredColumn,
  InferredDescriptor,
  Input,
  Location,
  Position,
  Primitive,
  Range,
  Replacement,
  Role,
  RunReport,
  Scope,
  SemanticToken,
  StorageCredential,
  Token,
  TokenKind,
} from './wire.gen.js';
export { HOST_MS } from './wire.gen.js';
export {
  CODES,
  DETAILS,
  FossilError,
  TITLES,
  helpUrl,
  isCode,
  isFossilError,
  type Code,
  type Foreign,
  type Problem,
  type Severity,
} from './error.js';
export { referenceTo } from './reference.js';
