import { getHighlighter } from "fumadocs-core/highlight";

/**
 * Shiki's grammar type, taken from the function that consumes it.
 *
 * `shiki` is fumadocs' dependency, not this app's — the manifest names neither the package nor a
 * version, so importing from it would be claiming something the install does not promise. Reading
 * the type off `getHighlighter`'s own signature keeps the grammar checked against the exact shape
 * the call accepts, and moves with it.
 */
type Grammar = NonNullable<NonNullable<Parameters<typeof getHighlighter>[1]>["langs"]>[number];

/**
 * A TextMate grammar for fossil, small on purpose.
 *
 * It colours what the language actually distinguishes and nothing else: the comment, the string and
 * its holes, the two binders, the language's own `@` slots, and the difference between a name that
 * is being called and a name that is a type. There is no attempt to know whether `User` is a source
 * or a shape — that is the compiler's job, and a highlighter that guesses at it would be wrong in
 * exactly the cases a reader is most likely to be looking at.
 */
export const fossil: Grammar = {
  name: "fossil",
  scopeName: "source.fossil",
  patterns: [
    { include: "#comment" },
    { include: "#string" },
    { include: "#slot" },
    { include: "#keyword" },
    { include: "#number" },
    { include: "#call" },
    { include: "#type" },
    { include: "#operator" },
  ],
  repository: {
    comment: {
      patterns: [{ match: "//.*$", name: "comment.line.double-slash.fossil" }],
    },
    string: {
      patterns: [
        {
          name: "string.quoted.double.fossil",
          begin: '"',
          end: '"',
          patterns: [
            { match: "\\{\\{|\\}\\}", name: "constant.character.escape.fossil" },
            { match: "\\\\.", name: "constant.character.escape.fossil" },
            // A hole holds an ordinary expression. Colouring it as one unit is the honest
            // simplification: the point on the page is that something is interpolated here.
            { match: "\\{[^{}]*\\}", name: "variable.other.interpolation.fossil" },
          ],
        },
      ],
    },
    slot: {
      // `@subject`, `@rename` — the language's own namespace, against the bare names the shape
      // declares. Marked as a keyword because that is what it is.
      patterns: [{ match: "@[A-Za-z_][A-Za-z0-9_]*", name: "keyword.control.fossil" }],
    },
    keyword: {
      patterns: [{ match: "\\b(type|from|on|as|and|or|not)\\b", name: "keyword.other.fossil" }],
    },
    number: {
      patterns: [{ match: "\\b\\d+(?:\\.\\d+)?\\b", name: "constant.numeric.fossil" }],
    },
    call: {
      patterns: [
        {
          match: "([A-Za-z_][A-Za-z0-9_]*)(?=\\s*\\()",
          captures: { 1: { name: "entity.name.function.fossil" } },
        },
      ],
    },
    type: {
      patterns: [{ match: "\\b[A-Z][A-Za-z0-9_]*\\b", name: "entity.name.type.fossil" }],
    },
    operator: {
      patterns: [{ match: ":=|==|!=|>=|<=|[=<>]", name: "keyword.operator.fossil" }],
    },
  },
};

/**
 * Put the grammar where the highlighter looks for one, on the instance every code block shares.
 *
 * Registering the grammar object directly is not enough, and the reason is worth writing down
 * because the failure looks like success right up to the build. Shiki loads a language by *name*
 * against the bundle it was built with, so a block asking for `fossil` sends the string down that
 * path however many grammar objects have been loaded by hand — and the string is not in the bundle.
 * Adding an entry to the bundle record is what makes the name resolve, and it is the same record
 * `highlight()` consults before deciding whether to fall back to plain text.
 *
 * It mutates a shared registry, which is a real side effect and is stated rather than hidden: one
 * key, added once, on the highlighter this app owns.
 */
let loading: Promise<unknown> | undefined;

export function loadFossilGrammar(): Promise<unknown> {
  loading ??= getHighlighter("js").then((instance) => {
    const bundle = instance.getBundledLanguages() as Record<string, unknown>;
    bundle.fossil ??= () => Promise.resolve([fossil]);
  });
  return loading;
}
