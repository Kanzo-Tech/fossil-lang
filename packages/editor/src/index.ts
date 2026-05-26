/**
 * @fossil-lang/editor — public entry point.
 *
 * Phase 11 plan 11-01: scaffold only — Transport contract exposed.
 * Plan 11-02 adds FossilEditor + FossilEditorProps.
 * Plan 11-03 adds WorkerTransport + HttpTransport + NullTransport.
 */
export type {
  Transport,
  JsonRpcRequest,
  JsonRpcResponse,
  JsonRpcNotification,
  SendOptions,
} from './transports/types.js';
