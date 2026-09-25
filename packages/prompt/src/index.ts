/**
 * @fossil-lang/prompt — the Fossil surface language as a system prompt for a model writing
 * programs.
 *
 * ```ts
 * import { FOSSIL_PROMPT } from '@fossil-lang/prompt';
 *
 * const system = `${FOSSIL_PROMPT}\n\n${hostRules}`;
 * ```
 *
 * It is a string and it has no dependencies, on purpose: the call that uses it is usually a
 * server-side LLM route, which should import text and not a wasm module. What a host adds is its
 * own — which connections exist, what the model is given, what shape the answer takes.
 *
 * Nothing in it can drift from the language unnoticed: the library is generated from
 * `catalogue.bnf`, the example is a conformance program, and the prose is pinned to
 * `grammar.bnf`. See `./surface.ts` for what holds each part.
 */
import { LIBRARY, NAMES } from './catalogue.generated.js';
import { EXAMPLE, FORBIDDEN, SURFACE, type ForbiddenForm } from './surface.js';

export { EXAMPLE, FORBIDDEN, LIBRARY, NAMES };
export type { ForbiddenForm };

/** The forbidden forms, as the prompt states them. */
const FORBIDDEN_SECTION = [
  '## Forms that DO NOT exist',
  '',
  'Earlier Fossil had these. They are gone and the checker refuses them:',
  '',
  ...FORBIDDEN.map((f) => `- No ${f.form}. Instead: ${f.instead}.`),
].join('\n');

/** The whole prompt: the surface, the library, and the forms that are gone. */
export const FOSSIL_PROMPT = [SURFACE, LIBRARY, FORBIDDEN_SECTION].join('\n\n');
