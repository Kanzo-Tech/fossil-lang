/**
 * One program, open in a workspace, answering an editor — LSP's `didOpen` → requests → `didClose`,
 * TypeScript's `LanguageService`, written once.
 *
 * 1. **The text arrives through {@link FossilProgram.update}, and only there.** Every question
 *    answers about the text of the last update, as an LSP request answers about the last
 *    `didChange`. `@fossil-lang/codemirror-fossil` calls it from the view's update cycle, inside the
 *    transaction that changed the document, so no hover, completion or lint source can read a newer
 *    document than the workspace holds. `update` is the one call that bumps the Salsa revision.
 * 2. **Read what the program names before you check it.** The compiler performs no IO. An edit can
 *    name a shape document the workspace has not read, so {@link FossilProgram.diagnostics} first
 *    runs `resolveDocuments` over the host's {@link Host} — which reads nothing when nothing is
 *    missing.
 * 3. **The sources are described when the host says so.** {@link FossilProgram.describe} profiles
 *    what the program reads on the host's engine and hands the answer to the checker. It is never
 *    run inside a check: a check runs on every pause in typing, a describe is a credential and a
 *    query per source, and only the host knows when a source may have changed.
 *
 * One workspace answers all of it, so hover answers from the same revision as the squiggles on
 * screen. The position methods take a SHARED borrow on the Rust side, so they nest inside a live
 * check rather than poisoning it.
 */
import { FossilWorkspace, tokenize } from '../pkg/fossil_wasm.js';
import { resolveDocuments } from '@fossil-lang/storage';
import {
  FossilError,
  type CompletionItem,
  type DataRef,
  type Diagnostic,
  type Host,
  type Hover,
  type Input,
  type Location,
  type Profiled,
  type SemanticToken,
  type Token,
} from '@fossil-lang/types';
import { profileInputs, type ProfileIO } from './describe.js';
import { diagnosticOf, until, type DocumentWorkspace, type UnreadDocument } from '@fossil-lang/types/internal';

import { initFossilWasm } from './load.js';

/** What {@link open} takes beside the key. */
export interface OpenOptions {
  /** The connection map, and the credentials a document the program names is read with. */
  host: Host;
  /** The buffer's text at open. Defaults to empty. */
  text?: string;
  /** Stops the open — the boot and the first read of the documents — rejecting with its reason. */
  signal?: AbortSignal;
}

/** A program open in its own workspace. Every question answers about the text of the last {@link update}. */
export interface FossilProgram {
  /** The key the program is open under — `Diagnostic.uri` and `Location.uri` carry it. */
  readonly uri: string;
  /** The compiler's lexer, for highlighting. Needs no workspace. */
  tokenize(text: string): Token[];
  /** The buffer's text now — LSP's `didChange`. Free when it is the text already held. */
  update(text: string): void;
  /**
   * Read every document the text names that the workspace lacks, and check. No debounce:
   * `fossil()`'s linter waits out its own delay AND waits for this to return before it schedules
   * again, which is what an LSP client does with `textDocument/diagnostic`.
   *
   * A document that could not be read is a diagnostic of its own at the top of the program, under
   * the code of its read — `storage/host-silent`, `storage/unreachable`, … — beside the checker's,
   * which see it only as missing.
   */
  diagnostics(): Promise<Diagnostic[]>;
  /** What the compiler knows about each name and reference — shapes, declarations, connections —
   *  laid over {@link tokenize}'s lexical highlighting. */
  semanticTokens(): SemanticToken[];
  /** The type under the cursor, and the type the target shape demands of it — or `null`. */
  hover(line: number, character: number): Hover | null;
  /** The candidates at the cursor, narrowed by the receiver's type. */
  completion(line: number, character: number): CompletionItem[];
  /** Where the name under the cursor is defined. Often in the shape document: read `uri`. */
  definition(line: number, character: number): Location[];
  /**
   * What the program reads, through the connection map — what a profile DESCRIBEs. Resolves
   * the documents first, because a location goes through the connection map the host answers.
   */
  inputs(): Promise<Input[]>;
  /**
   * The RML-Core mapping the program's types describe, as Turtle: what `mapping.ttl` is, before any
   * run. A pure function of the types the program declares and imports — it reads no source, so it
   * needs no {@link describe} first — and the document a reader parses with rudof's `Mapping.parse`.
   * A program with no type describes nothing and answers an empty mapping.
   *
   * It differs from the document a run writes in one way: a column the engine produces as a
   * decimal or as bytes is `xsd:string` here, because only a run knows. Rejects with
   * `write/ambiguous-iri` when two types, or two columns of one type, share an IRI — what the run
   * would refuse.
   */
  mapping(): Promise<string>;
  /**
   * Profile what the program reads, on `engine`, under the credentials the program's host vends —
   * {@link inputs}, then `profile` — and give the answer to the checker. Answers the profile, which
   * is what a run takes as `profiled`.
   *
   * A described source types the program under the key it was written as. A source that could not
   * be described is a warning of {@link diagnostics}' at the call that reads it — `io.csv("…")` —
   * under its problem's code (`source/not-found`, `storage/*`, `engine/failed`), once per key. The
   * latest answer about a key wins, so a source that describes on a later call loses its warning.
   */
  describe(io: Omit<ProfileIO, 'host'>): Promise<Profiled>;
  /** Free the workspace. Every member fails after it. */
  close(): void;
  /** {@link close}, for `using`. */
  [Symbol.dispose](): void;
}

/** The documents half of `handle` in `workspace`, for `resolveDocuments`. */
function documents(workspace: FossilWorkspace, handle: ReturnType<FossilWorkspace['openFile']>): DocumentWorkspace {
  return {
    setConnections: (connections) => workspace.setConnections(connections),
    missingDocuments: () => workspace.missingDocuments(handle),
    registerDocument: (key, text) => workspace.registerDocument(key, text),
  };
}

/**
 * Boot the module, open `uri` in a fresh workspace, and read the documents its text names.
 *
 * A boot that succeeded is kept, so a second program in the same tab costs a workspace and nothing
 * else.
 */
export async function open(uri: string, options: OpenOptions): Promise<FossilProgram> {
  const { host, text = '', signal } = options;
  await until(initFossilWasm(), signal);

  const workspace = new FossilWorkspace();
  const handle = workspace.openFile(uri, text);
  let held = text;
  // What the last read of the documents could not read; every settle reads what is missing again.
  let unread: readonly UnreadDocument[] = [];
  const settle = async (stop?: AbortSignal): Promise<void> => {
    ({ unread } = await resolveDocuments(documents(workspace, handle), host, { signal: stop }));
  };

  try {
    await settle(signal);
  } catch (cause) {
    workspace.free();
    throw cause;
  }

  const close = (): void => workspace.free();
  return {
    uri,
    tokenize,
    update(next) {
      if (next === held) return;
      workspace.updateFile(handle, next);
      held = next;
    },
    async diagnostics() {
      await settle();
      const rows = workspace.diagnostics();
      const failed = unread.map(({ key, problem }) => diagnosticOf(uri, problem, `${key}: ${problem.detail}`));
      return [...failed, ...rows];
    },
    semanticTokens: () => workspace.semanticTokens(handle),
    // `serde_wasm_bindgen` writes `None` as `undefined`; a host should have one falsy answer to check.
    hover: (line, character) => workspace.hover(handle, line, character) ?? null,
    completion: (line, character) => workspace.completion(handle, line, character),
    definition: (line, character) => workspace.definition(handle, line, character),
    async inputs() {
      await settle();
      return workspace.inputs(handle);
    },
    async mapping() {
      await settle();
      return workspace.mapping(handle);
    },
    async describe(io) {
      await settle(io.signal);
      const profiled = await profileInputs(workspace.inputs(handle), { ...io, host });
      for (const descriptor of profiled.descriptors) workspace.registerInferredDescriptor(JSON.stringify(descriptor));
      for (const { source, problem } of profiled.undescribed) {
        workspace.registerUndescribed(source.key, JSON.stringify(problem));
      }
      return profiled;
    },
    close,
    [Symbol.dispose]: close,
  };
}

/**
 * What `text` reads, with no editor — TypeScript's `ts.preProcessFile`: a stateless scan, in a
 * workspace of its own that is freed before it answers, so it never touches an editor's program.
 *
 * With a `host`, each location goes through `host.connections()` and `connection` names the one it
 * lies under; without one, every location is its key anchored beside the program. It reads no
 * document. Boots the module itself.
 */
export async function inputs(
  text: string,
  { host, signal }: { host?: Host; signal?: AbortSignal } = {},
): Promise<Input[]> {
  await until(initFossilWasm(), signal);
  const workspace = new FossilWorkspace();
  try {
    const handle = workspace.openFile('', text);
    // The connections half of the one IO loop: a workspace that reports nothing missing reads nothing.
    if (host !== undefined) {
      await resolveDocuments(
        { ...documents(workspace, handle), missingDocuments: () => [] },
        host,
        { signal },
      );
    }
    return workspace.inputs(handle);
  } finally {
    workspace.free();
  }
}

/**
 * What the sources a host names are, before there is a program — the files an assistant is shown:
 * each read as a program at the workspace's root would read it, bound under its `bindingName` and
 * expanded through `host.connections()`, then profiled as {@link FossilProgram.describe} profiles a
 * program's inputs. No program text is written to get there. Boots the module itself.
 *
 * @throws {FossilError} `api/invalid-argument` when two references make one binding (`a.csv` and
 *   `a.json`): a profile names its sources by binding, and a suffix to tell them apart is a name
 *   nobody wrote.
 */
export async function profile(sources: readonly DataRef[], io: ProfileIO): Promise<Profiled> {
  const { host, signal } = io;
  await until(initFossilWasm(), signal);
  const workspace = new FossilWorkspace();
  let named: Input[];
  try {
    await resolveDocuments(
      {
        setConnections: (connections) => workspace.setConnections(connections),
        missingDocuments: () => [],
        registerDocument: () => {},
      },
      host,
      { signal },
    );
    named = workspace.inputsNamed(sources);
  } finally {
    workspace.free();
  }
  const bindings = named.map((i) => i.binding);
  if (new Set(bindings).size !== bindings.length) {
    throw FossilError.of('api/invalid-argument', {
      argument: 'sources',
      expected: 'references whose binding names differ',
    });
  }
  return profileInputs(named, io);
}
