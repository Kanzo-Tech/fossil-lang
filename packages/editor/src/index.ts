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
// HttpTransport added in Task 2 of plan 11-03.
