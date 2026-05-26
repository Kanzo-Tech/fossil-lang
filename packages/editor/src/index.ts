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

// === 11-03: Transports — Worker / Http / Null land in plan 11-03 ===
