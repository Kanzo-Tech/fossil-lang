import type { Problem } from './error.js';
import type { InferredDescriptor, Input } from './wire.gen.js';

/**
 * What a host's profile of the sources answered — `profile`'s result in `@fossil-lang/profile`,
 * passed as it came back: the sources it described, and the ones it could not, each with its problem.
 *
 * The editor takes it (`FossilProgram.registerProfile` in `@fossil-lang/wasm`) and so does the run
 * (`run`'s `profiled` in `@fossil-lang/executor`), so a run checks a program against the columns the
 * editor checked it against.
 */
export interface Profiled {
  descriptors: readonly InferredDescriptor[];
  undescribed: readonly { source: Pick<Input, 'key'>; problem: Problem }[];
}
