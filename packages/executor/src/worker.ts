/**
 * The executor's worker: the module a browser run is in (`/docs/design/off-the-page`). It holds the
 * executor, instantiated from the module the page compiled, and asks the page for every host call;
 * `./protocol.ts` is both halves.
 */

import type { Host } from '@fossil-lang/types';

import { runHere, type RunOptions } from './here.js';
import { bootHere } from './load.js';
import { serve, type Port } from './protocol.js';

serve(globalThis as unknown as Port, async (request, host: Host | undefined) => {
  if (request.module !== undefined) await bootHere(request.module);
  const { program, into, limit, profiled } = request;
  const options: RunOptions =
    host !== undefined
      ? { host, into, limit, profiled }
      : { files: request.files ?? {}, path: request.path ?? '', into: 'memory', limit, profiled };
  return runHere(program, options);
});
