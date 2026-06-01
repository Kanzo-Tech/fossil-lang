/**
 * @fossil-lang/editor — public entry point.
 */

// === 11-01: Transport contract ===
export type {
  Transport,
  JsonRpcRequest,
  JsonRpcResponse,
  JsonRpcNotification,
  SendOptions,
} from './transports/types.js';

// === 11-02: FossilEditor + props ===
export { FossilEditor } from './FossilEditor.js';
export type { FossilEditorProps } from './FossilEditor.js';

// === 11-03: Transports — Worker / Http / Null ===
export {
  WorkerTransport,
  createWorkerTransport,
} from './transports/Worker.js';
export type { WorkerTransportOpts } from './transports/Worker.js';
export { NullTransport } from './transports/Null.js';
export { HttpTransport } from './transports/Http.js';
export type { HttpTransportOpts } from './transports/Http.js';

// === Schema introspection — re-export the canonical @fossil-lang/introspect ===
// so a host (keasy, playground) imports the editor + the introspection helpers
// from one place. The `descriptors` prop on <FossilEditor/> consumes the
// `InferredDescriptor[]` these produce.
export {
  introspect,
  extractSourceRefs,
  duckdbTypeToFossilPrimitive,
  describeSql,
  buildDescriptor,
} from '@fossil-lang/introspect';
export type {
  InferredDescriptor,
  InferredColumn,
  InferredPrimitive,
  SourceRef,
  DescribeRow,
  IntrospectIO,
} from '@fossil-lang/introspect';
