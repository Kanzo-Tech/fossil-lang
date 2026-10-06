# @fossil-lang/codemirror-fossil

The fossil language layer for CodeMirror 6. Five halves by now, and every one is an
answer `@fossil-lang/wasm` already had:

| half | what drives it | what you see |
|---|---|---|
| highlighting | `tokenize()`, then `semanticTokens()` over it | the compiler's own lexer, coloured by the host's own theme; shapes, declarations and `@connections` told apart where the program is open |
| diagnostics | `check()` → `@codemirror/lint` | squiggles, with the checker's messages and help verbatim, and its two quick fixes — did-you-mean and split-mapping — as actions |
| hover | `hover()` → `hoverTooltip` | the type of what you wrote AND the type the target shape demands of it |
| completion | `completions()` → `@codemirror/autocomplete` | the receiver's members, spelled bare — `trim`, not `str.trim` |
| go to definition | `gotoDefinition()` → a keymap | `F12`, `Alt-.`, Mod-click; a target in the shape document goes to the host |

There is no TypeScript lexer here and there will not be one. `grammar.bnf` is
normative, `crates/fossil-syntax` implements it, and a second implementation in
another language drifts — that is not a hypothetical, it is what the discriminant
table in this package's predecessor did.

## Extensions, not an editor

Everything exported is an `Extension`. No component, no `EditorView`, and no
colours — the one `baseTheme` sets the margins of the hover tooltip's own markup,
at the lowest precedence CodeMirror has. The editor is the host's decision:

```ts
import { fossil } from '@fossil-lang/codemirror-fossil';
import { openProgram } from '@fossil-lang/wasm';

// `host` is the host's `Host`: the connection map, and credentials to read what the program names.
const program = await openProgram('hello.fossil', { host, text });

const extensions = fossil({ ...program, onNavigate: (target) => console.log(target) });
```

`openProgram` answers with exactly this package's option names — `tokenize`,
`semanticTokens`, `uri`, `check`, `hover`, `complete`, `definition` — over one workspace, so the host writes
the one thing that is its own to decide: where a definition in another file goes.

**Every source takes the text, and that repetition is the design.** The workspace answers
about the text of the last `updateFile`; the checker is debounced, hover fires on
mouse-move and completion on nearly every keystroke, so three of the four run between two
checks. A source taking only a position would let a host query text it had not pushed and
get a range one keystroke wrong. `openProgram` pushes before every answer and compares
against what it last sent, so the push costs a string comparison in the common case; a
host wiring `FossilWorkspace` by hand owes the same discipline.

**A check that fails is a row, not a throw.** When `check` rejects, the linter draws the
failure on the first character by its code (`internal/bug` when fossil did not raise it) and
hands `onDiagnostics` that one row — `uncheckedRow(uri, cause)` — through the same call as any
batch. A host counting rows for a panel or a badge wraps nothing; a failure before there is a
linter, `openProgram` rejecting, is reported with the same `uncheckedRow`.

`@kanzo-tech/ui`'s `CodeEditor` takes exactly that as its `extensions` prop and
holds it in a live-reconfigured `Compartment`. So does a bare `EditorView`.

## `kind` is a name

`TokenRow.kind` is the lexer's variant name — `"Comment"`, `"KwFrom"` — typed by the
`Token` union `@fossil-lang/types` generates from the Rust enum, so `src/tags.ts` maps
names and a key the lexer does not have fails the type-check. Offsets are UTF-16 code
units, the units CodeMirror indexes in.

## Two layers of colour

`tokenize()` paints first and needs no workspace. `semanticTokens()` — the same
`fossil-ide` answer the language server sends, as absolute rows with the legend's
names — is laid over it, the way rust-analyzer's tokens sit over a TextMate grammar:

| semantic kind | tag |
|---|---|
| `type` | `tags.typeName` |
| `function` | `tags.function(tags.variableName)` |
| `property` | `tags.propertyName` |
| `parameter` | `tags.attributeName` |
| `variable` | `tags.variableName` |
| `namespace` | `tags.namespace` — `io`, and the `@connection` carved out of a reference |
| `keyword` | `tags.keyword` — only where the lexer painted nothing (`type`, `as`) |
| `declaration` modifier | `tags.definition(…)` around the kind's tag |

The semantic layer wins where it names something the lexer cannot; where it repeats
what the lexer already said (strings, numbers, operators, `and`), the lexer's finer tag
stays. Without `semanticTokens`, or if it throws, the lexical layer is the whole answer.

## What it does not do

- **Indentation.** Fossil is INDENT/DEDENT. An `indentService` needs the parser's
  view of block openers, not the lexer's.

## Peers

All required — this package is the wiring between them and fossil, so a consumer
with none of them has no use for it.

`@codemirror/autocomplete`, `@codemirror/language`, `@codemirror/lint`,
`@codemirror/state`, `@codemirror/view`, `@fossil-lang/wasm`.

`@fossil-lang/wasm` is a peer rather than a dependency because the host owns when
the module is instantiated, and two copies of the wasm-bindgen glue is the
duplication that `packages/wasm/src/client.ts` was split to prevent.
