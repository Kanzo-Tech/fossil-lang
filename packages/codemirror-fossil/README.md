# @fossil-lang/codemirror-fossil

The fossil language layer for CodeMirror 6. Five halves by now, and every one is an
answer `@fossil-lang/wasm` already had:

| half | what drives it | what you see |
|---|---|---|
| highlighting | `tokenize()` + `tokenKinds()` | the compiler's own lexer, coloured by the host's own theme |
| diagnostics | `check()` → `@codemirror/lint` | squiggles, with the checker's messages verbatim |
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

// `host` is the host's `SourceHost`: the connection map, and signing what the program names.
const program = await openProgram('hello.fossil', { host, text });

const extensions = fossil({ ...program, onNavigate: (target) => console.log(target) });
```

`openProgram` answers with exactly this package's option names — `tokenize`, `tokenKinds`,
`uri`, `check`, `hover`, `complete`, `definition` — over one workspace, so the host writes
the one thing that is its own to decide: where a definition in another file goes.

**Every source takes the text, and that repetition is the design.** The workspace answers
about the text of the last `updateFile`; the checker is debounced, hover fires on
mouse-move and completion on nearly every keystroke, so three of the four run between two
checks. A source taking only a position would let a host query text it had not pushed and
get a range one keystroke wrong. `openProgram` pushes before every answer and compares
against what it last sent, so the push costs a string comparison in the common case; a
host wiring `FossilPlayground` by hand owes the same discipline.

`@kanzo-tech/ui`'s `CodeEditor` takes exactly that as its `extensions` prop and
holds it in a live-reconfigured `Compartment`. So does a bare `EditorView`.

## `kind` is a number and the legend is the contract

`TokenRow.kind` is `fossil_syntax::lexer::Token as u32` — a variant discriminant,
which any reorder of the enum remaps with nothing going red.

The version of this package deleted in `873cbc0` hard-copied that table:

```ts
export enum FossilKind { Whitespace = 0, Newline = 1, Comment = 2, KwPrefix = 3, … }
```

under a comment saying it «MUST stay in sync with
`crates/fossil-syntax/src/lexer.rs` — reorders are a breaking change». Nothing
enforced it, and by the time the package went it named nine variants the lexer no
longer had (`KwPrefix`, `KwIn`, `KwUse`, `KwAs`, `KwIri`, `Template`, `AbsIri`,
`EnvVar`, `Pipe`), was missing three it had gained (`True`, `False`, `Null`), and
every discriminant from 3 upwards pointed at the wrong token.

So `fossil-wasm` now ships `tokenKinds()`, a legend of variant names indexed by
the discriminant, and `src/tags.ts` maps **names**. A reorder moves both sides at
once. The guard is `token_kinds_legend_indexes_by_kind` in
`crates/fossil-wasm/tests/tokenize.rs`, which compares the legend against the
lexer itself — on the side that knows.

## What it does not do

- **Semantic highlighting.** `semanticLegend()` is on the main thread but the
  tokens are not — they still come back only over the Worker. This is why `Ident`
  carries no tag: the lexer cannot tell a type from a binding from a column, and
  guessing would only have to be undone by the overlay that can.
- **Code actions.** `fossil-ide` has two quick fixes and both hang off a
  diagnostic. `@codemirror/lint`'s `Diagnostic.actions` is where they go, and they
  need the structured diagnostic that the `CheckRow` wire form flattens.
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
