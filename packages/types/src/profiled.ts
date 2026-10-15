import type { Problem } from './error.js';
import type { InferredDescriptor, Input, Profile } from './wire.gen.js';

/** A source a profile could not describe, and why. */
export interface UndescribedSource {
  source: Input;
  problem: Problem;
}

/**
 * What a profile of a program's sources answered: the profile, whose sources are named by their
 * binding; each described source's columns keyed by what the program wrote; and the sources it could
 * not describe, each with its problem.
 *
 * `FossilProgram.describe` in `@fossil-lang/wasm` answers it, and the run takes it as it came back
 * (`run`'s `profiled` in `@fossil-lang/executor`), so a run checks a program against the columns the
 * editor checked it against.
 */
export interface Profiled {
  profile: Profile;
  descriptors: readonly InferredDescriptor[];
  undescribed: readonly UndescribedSource[];
}
