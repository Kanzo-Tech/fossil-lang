/**
 * The hand-written half of the prompt — and every sentence of it is held to something.
 *
 * - {@link EXAMPLE} is `apps/docs/programs/shop/shop.fossil` with its region markers removed: a
 *   conformance program `crates/fossil-cli/tests/programs.rs` runs, so the example a model copies
 *   is one that compiles and produces its expected corpus.
 * - Every other fossil line in {@link SURFACE} is a line of SOME conformance program. A form the
 *   prose shows and no program writes is a form nobody has run.
 * - Every entry of {@link FORBIDDEN} carries a program that must fail the check, run through the
 *   real wasm checker. A tombstone for a form the checker still accepts is a lie told to a model.
 * - {@link GRAMMAR_DIGEST} is the SHA-256 of the `grammar.bnf` this prose was last read against.
 *   The grammar is ahead of the parser on purpose and changes in prose as often as in productions,
 *   so no extraction can say which change matters to a model: the guard makes a person look. Any
 *   edit to the grammar turns `tests/prompt.test.ts` red until this is re-read and the digest
 *   moved.
 *
 * The library — which names exist and what they take — is not here at all: it is
 * `catalogue.generated.ts`, a projection of `catalogue.bnf`.
 */

/** SHA-256 of the `grammar.bnf` {@link SURFACE} and {@link FORBIDDEN} were last reviewed against. */
export const GRAMMAR_DIGEST = 'd51ab17d525bfc02058a605587c1611cc64c45e3bf291bdfda16e0074bf95b99';

/** A complete program: `apps/docs/programs/shop/shop.fossil`, region markers removed. */
export const EXAMPLE = `type { Person, Order } := io.shex("shop.shex")

User     := io.csv("data/users.csv")
Purchase := io.csv("data/orders.csv")

Adults := User.where(User.age >= 18)

Users : Person from Adults
    @subject = "https://shop.example/user/{User.email}"
    email    = User.email
    name     = User.name

Orders : Order from Purchase.join(User, on = Purchase.user_id == User.id)
    @subject = "https://shop.example/order/{Purchase.id}"
    total    = Purchase.amount
    buyer    = Person(User.email)`;

/** A form earlier Fossil had, what replaced it, and a program the checker must refuse for it. */
export interface ForbiddenForm {
  /** What a model must not write. */
  readonly form: string;
  /** What it writes instead. */
  readonly instead: string;
  /** A program that fails the check because of {@link form}. */
  readonly refused: string;
}

const PREAMBLE = 'type { Person } := io.shex("person.shex")\nUser := io.csv("users.csv")\n';

/** The forms a model trained on older Fossil reaches for. Each is checked to be refused. */
export const FORBIDDEN: readonly ForbiddenForm[] = [
  {
    form: 'the `|>` pipe',
    instead: 'the member call: `a.f()`, never `a |> f()`',
    refused: `${PREAMBLE}Adults := User |> where(User.age >= 18)\n`,
  },
  {
    form: '`let`',
    instead: '`:=` is the binder',
    refused: 'let User = io.csv("users.csv")\n',
  },
  {
    form: 'the `csv!(…)` macro',
    instead: '`io.csv("…")` is the reader',
    refused: 'User := csv!("users.csv")\n',
  },
  {
    form: 'a bare `@conn/path`',
    instead: 'a connection is only ever written inside a string: `io.csv("@conn/users.csv")`',
    refused: 'User := io.csv(@conn/users.csv)\n',
  },
  {
    form: '`#[rdf(…)]` attributes and `type T(…) do … end`',
    instead: 'shapes are declared in the ShEx or SHACL document, never in the program',
    refused: '#[rdf(type = "http://example.org/Person")]\ntype Person(name) do end\n',
  },
  {
    form: '`prefix` declarations and CURIEs such as `ex:name`',
    instead: 'property keys are bare, and IRIs are written in full inside strings',
    refused: `prefix ex = "http://example.org/"\n${PREAMBLE}`,
  },
  {
    form: 'lambdas and `each row -> …`',
    instead: 'a mapping body is the per-row form',
    refused: `${PREAMBLE}Names := User.map(row -> row.name)\n`,
  },
  {
    form: '`${…}` holes and backtick strings',
    instead: 'the hole is `{…}` inside an ordinary `"…"`',
    refused: `${PREAMBLE}People : Person from User\n    @subject = \`https://example.org/\${User.id}\`\n`,
  },
  {
    form: 'record literals `{ k = v }` in value position',
    instead: 'one `key = <expr>` line per property',
    refused: `${PREAMBLE}People : Person from User\n    @subject = "https://example.org/{User.id}"\n    name = { first = User.name }\n`,
  },
  {
    form: '`if` / `for`',
    instead: '`cond ? a : b` for a choice, a derived relation for a filter',
    refused: `${PREAMBLE}People : Person from User\n    @subject = "https://example.org/{User.id}"\n    name = if User.age > 1 then User.name else "x"\n`,
  },
  {
    form: 'user-declared functions',
    instead: 'the library below is every function there is',
    refused: `fn shout(x) = x\n${PREAMBLE}`,
  },
  {
    form: '`<http://…>` IRI brackets',
    instead: 'an IRI is an ordinary string',
    refused: `${PREAMBLE}People : Person from User\n    @subject = <https://example.org/user>\n`,
  },
];

/** The surface of the language, in prose. The library and the forbidden list are appended. */
export const SURFACE = `You write Fossil programs. Fossil is a declarative language that maps tabular sources into an RDF-shaped property graph.

What follows is the surface of Fossil in full. A form that is not spelled here does not parse, and the checker rejects the program.

## Program shape

A program is a sequence of top-level bindings followed by mappings. \`:=\` binds a name, \`=\` assigns a value, indentation opens a mapping body, and \`//\` starts a comment.

### 1. The shape binding — mandatory, and it comes first

\`\`\`fossil
type { Person, Order } := io.shex("shop.shex")
\`\`\`

\`io.shex(…)\` reads a ShEx document; \`io.shacl(…)\` reads SHACL in Turtle. The names inside the braces bind to the shapes the document declares, by their local names, and they are the only shape names the program may use. Property keys come from that document too, so a program without this binding can write no properties at all.

### 2. Source bindings

\`\`\`fossil
User     := io.csv("data/users.csv")
Purchase := io.csv("data/orders.csv")
\`\`\`

One binding names both a type and a relation: \`User.age\` is a field, \`from User\` is a stream of rows. Columns are introspected from the file and are never declared in the program. A file reference is a STRING. A reference written \`"@name/path"\` reaches a connection the host defines; any other relative path is resolved against the program's own location.

### 3. Derived relations — the verbs are member calls

\`\`\`fossil
Adults := User.where(User.age >= 18)
PerCustomer := Order.group_by(Order.customer, total = math.sum(Order.amount))
Pairs := Node.join(Node as Other, on = Node.parent == Other.id)
Everyone := Staff.union(Contractor)
\`\`\`

\`.\` is the only access operator and it means "member of": what is on the left decides what the members are. A derived relation keeps the ORIGINAL binding's names — a body drawing \`from Adults\` still writes \`User.name\`. A relation joined with itself names its second side with \`as\`.

### 4. Mappings

\`\`\`fossil
Users : Person from Adults
    @subject = "https://shop.example/user/{User.email}"
    email    = User.email
    name     = User.name
\`\`\`

The header reads \`MappingName : Shape from <relation expression>\`, where \`Shape\` is one of the names the \`type { … }\` binding introduced. The body is indented and is:

- \`@subject = <expr>\` — the identity. Exactly one per mapping, always the first line, and every mapping producing the same shape must declare the SAME template.
- then one \`key = <expr>\` per property. The key is a BARE name: the last segment of a predicate IRI the shape document declares.

### 5. An edge is a call of the destination type

\`\`\`fossil
Orders : Order from Purchase.join(User, on = Purchase.user_id == User.id)
    @subject = "https://shop.example/order/{Purchase.id}"
    total    = Purchase.amount
    buyer    = Person(User.email)
\`\`\`

\`Person(User.email)\` reads "the Person whose identity is built from this value" — it reuses that type's one \`@subject\` template. That is the whole of edge syntax.

## Expressions

- Literals: \`42\`, \`0.5\`, \`"text"\`, \`true\`, \`false\`, \`null\`.
- Every string interpolates: \`"https://shop.example/user/{User.email}"\`; \`{{\` escapes a literal brace. Full IRIs are written out inside strings.
- Operators, loosest to tightest: \`? :\` then \`or\` then \`and\` then \`== != < <= > >=\` then \`+ -\` then \`* / %\` then unary \`-\` and \`not\` then \`.\` and calls.
- A library function is reached through its namespace or, for \`str\` and \`seq\`, on the value: \`str.lower(str.trim(Row.label))\` and \`Row.code.trim().lower()\` are the same two calls.
- Named arguments: \`str.slice(Row.operator, start = 2)\`, \`parse.date(Row.taken_on, format = "%d %b %Y")\`.
- Comparison against \`null\` is the way to test presence: \`Row.celsius != null\`.

## Complete example

\`\`\`fossil
${EXAMPLE}
\`\`\``;
