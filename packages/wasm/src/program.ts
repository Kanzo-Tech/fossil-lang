/**
 * One program, open in a workspace, answering an editor — what every host of the checker wrote by
 * hand before this existed.
 *
 * `FossilPlayground` is the workspace and it is right that it stays general: it holds any number
 * of files and answers about each. What a host with ONE program in ONE editor needs on top of it
 * is always the same protocol, and two copies of it had already diverged in their comments:
 *
 * 1. **Push before you ask.** Four callers at four rates — the debounced linter, hover on pointer
 *    rest, completion on nearly every keystroke, goto-def on a key — share one workspace, and all
 *    four answer about the text of the last `updateFile`. So every entry point takes the text and
 *    pushes it first, and a string comparison against what was last pushed makes that free in the
 *    common case. `updateFile` is the one call that bumps the Salsa revision; calling it per
 *    mouse-move would invalidate the memoised check the squiggles came from for nothing.
 * 2. **Read what the program names before you check it.** The compiler performs no IO. An edit
 *    can name a shape document the workspace has not read, so a check first runs
 *    `resolveDocuments` over the host's {@link SourceHost} — which reads nothing when nothing is
 *    missing.
 *
 * The result is shaped for `@fossil-lang/codemirror-fossil`'s `fossil()`: `tokenize`,
 * `tokenKinds`, `uri`, `check`, `hover`, `complete` and `definition` are exactly its option
 * names, so a host spreads it and adds what is its own to decide:
 *
 * ```ts
 * const program = await openProgram('job.fossil', { host, text });
 * const extensions = fossil({ ...program, onNavigate });
 * ```
 *
 * One workspace rather than one per rate, for the reason `fossil()`'s own header gives: a second
 * workspace for the position queries would answer from a different revision than the squiggles on
 * screen. The position methods take a SHARED borrow on the Rust side, so they nest inside a live
 * check rather than poisoning it.
 */
import { resolveDocuments, type ProgramSource, type SourceHost } from '@fossil-lang/types';

import { FossilPlayground, tokenize, tokenKinds } from './client.js';
import type {
  CheckRow,
  CompletionRow,
  DefinitionRow,
  HoverRow,
  InferredDescriptorJson,
} from './index.js';
import { initFossilWasm, type InitInput } from './load.js';

/** What {@link openProgram} takes beside the key. */
export interface OpenProgramOptions {
  /** The host's one capability: the connection map, and signing what the program names. */
  host: SourceHost;
  /** The buffer's text at open. Defaults to empty; every entry point takes the text anyway. */
  text?: string;
  /** How a signed document is read. Defaults to the global `fetch`. */
  fetch?: typeof fetch;
  /** The module's `.wasm`, for a host with no bundler — see {@link initFossilWasm}. */
  wasm?: InitInput;
}

/**
 * A program open in its own workspace. Every member that answers about the program takes the
 * current text, and pushes it first — see the module header for why that repetition is the design.
 */
export interface FossilProgram {
  /** The key the program is open under — `CheckRow.uri` and `DefinitionRow.uri` carry it. */
  readonly uri: string;
  /** The compiler's lexer, for highlighting. Needs no workspace, and is here so the spread works. */
  readonly tokenize: typeof tokenize;
  /** The legend for a token's `kind`. */
  readonly tokenKinds: typeof tokenKinds;
  /**
   * Push the text, read every document it names that the workspace lacks, and check. No debounce:
   * `fossil()`'s linter waits out its own delay AND waits for this to return before it schedules
   * again, which is what an LSP client does with `didChange`.
   */
  check(text: string): Promise<CheckRow[]>;
  /** The type under the cursor, and the type the target shape demands of it — or `null`. */
  hover(text: string, line: number, character: number): HoverRow | null;
  /** The candidates at the cursor, narrowed by the receiver's type. */
  complete(text: string, line: number, character: number): CompletionRow[];
  /** Where the name under the cursor is defined. Often in the shape document: read `uri`. */
  definition(text: string, line: number, character: number): DefinitionRow[];
  /**
   * The data sources the program reads, as fossil resolved them — what introspection DESCRIBEs.
   * Resolves the documents first, because a source's locator goes through the connection map.
   */
  sources(text: string): Promise<ProgramSource[]>;
  /**
   * A host-introspected input schema, registered under the URI the program wrote. The compiler
   * never introspects a source itself; push this before the check that should see it.
   */
  registerDescriptor(descriptor: InferredDescriptorJson): void;
  /** The workspace underneath, for the rare question this surface does not ask. */
  readonly workspace: FossilPlayground;
  /** Free the workspace. Every member fails after it. */
  close(): void;
}

/**
 * Boot the module, open `uri` in a fresh workspace, and read the documents its text names.
 *
 * One call and the answer goes straight into `fossil()`. The boot is memoised, so a second program
 * in the same tab costs a workspace and nothing else.
 */
export async function openProgram(uri: string, options: OpenProgramOptions): Promise<FossilProgram> {
  const { host, text = '', fetch: fetchImpl = globalThis.fetch, wasm } = options;
  await initFossilWasm(wasm);

  const workspace = new FossilPlayground();
  const handle = workspace.openFile(uri, text);
  let pushed = text;

  const sync = (next: string): void => {
    if (next === pushed) return;
    workspace.updateFile(handle, next);
    pushed = next;
  };
  const settle = async (next: string): Promise<void> => {
    sync(next);
    await resolveDocuments(workspace.workspace(handle), host, fetchImpl);
  };

  await settle(text);

  return {
    uri,
    tokenize,
    tokenKinds,
    async check(next) {
      await settle(next);
      return workspace.check();
    },
    hover(next, line, character) {
      sync(next);
      return workspace.hover(handle, line, character);
    },
    complete(next, line, character) {
      sync(next);
      return workspace.completions(handle, line, character);
    },
    definition(next, line, character) {
      sync(next);
      return workspace.gotoDefinition(handle, line, character);
    },
    async sources(next) {
      await settle(next);
      return workspace.sources(handle);
    },
    registerDescriptor(descriptor) {
      workspace.registerInferredDescriptor(descriptor);
    },
    workspace,
    close() {
      workspace.free();
    },
  };
}
