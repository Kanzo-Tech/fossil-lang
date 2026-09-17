/**
 * What to draw the corpus **with** — read off the corpus, rather than written down in the app.
 *
 * `Canvas.tsx` carried the answers as string literals: `fill="cluster_id"` on the renderer,
 * `field="birth_year"` on the chart, and `dense_id, birth_year, postcode, cluster_id` in the
 * crossfilter's view. Every one of them is a fact about *the bench corpus*, in a component that
 * claims to be a reader of any corpus — so pointing this app at a second one drew a blank canvas
 * and a chart over a column that is not there, with no error saying which of the four was wrong.
 *
 * ## The manifest declares an encoding now, and this reads it
 *
 * This header used to say the opposite — *there is no field in which a writer could say `this
 * column is the community`, so this module is the reader deciding it* — and
 * [`/docs/design/position`](/docs/design/position) retracts the sentence it took that from. What
 * retracted it was not an argument. It was a count: the declaration was already being made, by
 * `quasi_identifiers`, which is a disclosure-control field doing encoding work because it was the
 * only writer-side statement in reach, and by a generated constant no consumer imported.
 *
 * A vertex type declares its channels — a `name`, the `column`, the `scale` it is read on
 * (`categorical` or `quantitative`), and, for a categorical, the `domain` no Parquet footer holds.
 * **All three states of the block are read here**, and they are `coordinates:`' three:
 *
 * - **no `channels:` key** — the writer said nothing, and the derivation below answers exactly as
 *   it answered before the field existed. This is not tidiness: a corpus written before the block
 *   has to keep drawing, and that is what makes the block a declaration rather than a flag day.
 * - **an empty list** — the writer declares the type carries no channel. There is no colour, and
 *   the reader does not go looking for one in the bytes.
 * - **a list** — the answer. {@link Encoding.fill} is its categorical and nothing else is.
 *
 * ## Which half of the corpus answers which question
 *
 * `openCorpus` is explicit that the manifest's vocabulary and the payload's are two different
 * lists — three declared properties against seven columns on disk for the bench corpus — and the
 * split runs straight through this file:
 *
 * - **The declaration says what the corpus is drawn with**, where there is one. `channels:` is the
 *   only field in the format whose subject is the drawing, so nothing outranks it.
 * - **The bytes say what is drawable**, and answer alone for a corpus that declares nothing.
 *   `cluster_id`, `x`, `y` and `dense_id` are the layout pass's output and are declared by no
 *   `properties:` entry, so a derivation that believed the *property* list would find no column to
 *   colour by on a corpus that has one. {@link Encoding.label} is from `Corpus.types` either way.
 * - **The privacy block still ranks what is left over.** Which numeric column deserves a chart is
 *   a question `channels:` answers only if the writer declared a quantitative one; where it did
 *   not, `quasi_identifiers` names the columns whose distribution is a re-identification risk,
 *   which is the set worth showing beside the bound that was reached. {@link Encoding.brush}
 *   prefers a declared channel, then one of those, then the first numeric column there is.
 *
 * The fallbacks are stated rather than implied: a corpus with no privacy block still gets a chart,
 * and a corpus with no channel and no integral column gets a canvas of one colour rather than an
 * exception.
 *
 * ## There is no size ramp, and the reason is the door rather than this file
 *
 * `SliceRequest.r` is in `@kanzo-tech/graph`'s contract and `Slice.sizes` is in its answer, so the
 * renderer would spend a radius ramp on a column if it were handed one. It is not: `Corpus.frame`
 * carries **one** categorical per row — `Frame.categories`, from `FrameParams.fill` — and no
 * measure, so the streaming path has nothing to fill a `sizes` array from. The baseline could
 * (`whole.ts` loads the payload itself), and a size ramp on one side of a comparison and not the
 * other is not a comparison. What would change it is a second column on `FrameParams` and a second
 * array on `Frame`; nothing has asked for one, and every level file already carries every column,
 * so it would cost a projection rather than an artefact.
 */
import { PAYLOAD_ADDRESS, PAYLOAD_CATEGORICAL, PAYLOAD_COORDINATES } from '@fossil-lang/corpus';
import type { CorpusField, CorpusTypes, CorpusVertexType } from '@fossil-lang/corpus';
import type { Channel } from '@fossil-lang/executor';

/**
 * One channel a vertex type declares. **Re-exported and not restated**: the wire shape of
 * `fossil_sinks::manifest::Channel` has one statement in TypeScript, in the package that mirrors
 * the rest of that manifest, and a second one here would be a fifth spelling of a name this file
 * exists to stop spelling.
 */
export type { Channel } from '@fossil-lang/executor';

/**
 * The column the addressing numbers rows by. Never drawn, never brushed: it is the row's ADDRESS,
 * and a histogram over it is a picture of the Morton curve.
 */
const ADDRESS = PAYLOAD_ADDRESS[0]!;

/** The position. Drawn as the position, so it is never also a colour or a chart. */
const GEOMETRY: readonly string[] = PAYLOAD_COORDINATES;

/**
 * The community column — **the convention, for a corpus that declares nothing**, and read out of
 * `corpus.bnf` rather than written down here.
 *
 * `PAYLOAD_CATEGORICAL` is the `categorical` role of the writer's column table, generated by
 * `cargo xtask corpus` and glossed there as *an ordinal the writer computed, for a reader to colour
 * by*. It is the same constant `FrameParams.fill` defaults to in `@fossil-lang/corpus`, so the
 * door and this app no longer state the name independently — and that is why
 * `scripts/verify-encoding.mjs` no longer compares the two as if they were two sources.
 *
 * A convention and not an answer: it says what fossil's own writer emits, and a corpus that
 * declares a channel outranks it. {@link categoricalOf} falls back past it to the first integral
 * column a corpus written by something else happens to carry, and to nothing at all if it carries
 * none.
 */
const COMMUNITY = PAYLOAD_CATEGORICAL[0]!;

/** DuckDB's spellings of the whole-number types — the ones an ordinal can be read out of. */
const INTEGRAL = new Set([
  'TINYINT',
  'SMALLINT',
  'INTEGER',
  'BIGINT',
  'HUGEINT',
  'UTINYINT',
  'USMALLINT',
  'UINTEGER',
  'UBIGINT',
  'UHUGEINT',
]);

/** …and the rest of the numeric ones. `DECIMAL` carries its precision, so it is matched by prefix. */
const REAL = new Set(['FLOAT', 'DOUBLE', 'REAL']);

const integral = (field: CorpusField): boolean => INTEGRAL.has(field.type);
const numeric = (field: CorpusField): boolean =>
  integral(field) || REAL.has(field.type) || field.type.startsWith('DECIMAL');

/** What this app draws one vertex type with. Every field is a column name or `null`. */
export interface Encoding {
  /** The vertex type drawn — the named one, or the first the index names. */
  readonly type: string;
  /** How many of it there are, off the manifest's `vertex_count`: the mask's length, not a count. */
  readonly count: number;
  /**
   * The categorical a point is coloured by, or `null` for a corpus carrying none.
   *
   * Handed to the renderer as `SliceRequest.fill` and reached again by both sources, which need it
   * to compose SQL when a request names a CSS colour rather than a column.
   */
  readonly fill: string | null;
  /**
   * The column a vertex is **named** by — the subject IRI, when the payload carries one.
   *
   * Nothing draws it, and that is the measurement rather than an omission: `Slice.subjects` is
   * opt-in in `@kanzo-tech/graph` because it costs 1.87× the tile — 8.016 compressed bytes per row
   * against 9.23 for the four columns a point is drawn from. So the drawing path carries addresses
   * and this says what would be asked for if something had to be named.
   */
  readonly label: string | null;
  /** The numeric column the histogram bins and brushes, or `null` when the corpus has none. */
  readonly brush: string | null;
  /**
   * The crossfilter view's projection: the address, the colour, the chart's column, and the
   * declared quasi-identifiers the payload actually carries.
   *
   * **{@link brush} is in it explicitly, and used to be in it by luck** — the chart brushes a
   * column of this view, and the brush was reached through the quasi-identifiers, so it was
   * projected only as long as the corpus declared it one. A declared quantitative channel need not
   * be a quasi-identifier, and a `CREATE VIEW` without the column the histogram bins fails at open,
   * in a browser, with the corpus already fetched.
   *
   * Narrow otherwise. `label` is excluded even where it exists — it is the widest column in the
   * corpus and neither client filters or draws by it, so projecting it would put it in the way of
   * every scan the two make.
   */
  readonly columns: readonly string[];
}

/** What {@link encodingFor} reads. Both halves of the corpus, and neither is derived from the other. */
export interface EncodingParams {
  /** `Corpus.types` — the payload vocabulary, one `DESCRIBE` per type. */
  readonly types: CorpusTypes;
  /**
   * The drawn type's `channels:` block, in its three states: `undefined` where the manifest has no
   * such key, `[]` where it declares the type carries none, and a list where it declares them.
   *
   * A parameter and not a read, for the reason `quasiIdentifiers` is one: this module stays a pure
   * function of two artefacts so that `scripts/verify-encoding.mjs` can run it in Node over the same
   * corpus the tab opens. {@link channelsFor} is the reader, and it is in this file because a
   * channel is an encoding and nothing else in the app has a use for one.
   */
  readonly channels?: readonly Channel[];
  /**
   * `graph.graph.yml`'s `quasi_identifiers`, as the `<Type>.<column>` pairs it writes them in.
   *
   * A `string[]` and not a `Declaration`, so that this module imports nothing of the app: the
   * reader is `bound.ts, readDeclaration`, which reaches DuckDB, and a pure function is what
   * `scripts/verify-encoding.mjs` can run in Node against the same corpus the tab opens.
   */
  readonly quasiIdentifiers?: readonly string[];
  /** Which vertex type. Defaults to the first the index names — the door's own rule. */
  readonly type?: string;
}

/**
 * The column to colour by, from the bytes.
 *
 * Exported because both sources need it and neither has an {@link Encoding}: a `BoundedSource` is
 * built before the corpus resolves and composes its SQL after, so it derives this from the corpus
 * it opened rather than taking a constant from its options.
 */
export function categoricalOf(
  type: CorpusVertexType,
  channels?: readonly Channel[],
): string | null {
  const drawable = type.fields.filter(
    (f) => f.name !== ADDRESS && !GEOMETRY.includes(f.name) && f.name !== type.identity,
  );

  // Declared is declared, and the block is exhaustive about colour: a manifest that lists channels
  // and no categorical one has said there is nothing to colour by, so the derivation below is not
  // consulted. Falling back to it there would be the reader overruling the writer on the one
  // question the writer now has a field for.
  if (channels !== undefined) {
    // The column has to be ON DISK — a declaration the bytes do not honour is not drawable, and
    // `scripts/verify-encoding.mjs` is where that mismatch gets named rather than papered over. A
    // channel over the address or the position is skipped for the same reason a derived one is:
    // those are already drawn, as themselves.
    const named = channels.find(
      (c) => c.scale === 'categorical' && drawable.some((f) => f.name === c.column),
    );
    return named?.column ?? null;
  }

  const community = drawable.find((f) => f.name === COMMUNITY);
  if (community !== undefined && integral(community)) return community.name;
  return drawable.find(integral)?.name ?? null;
}

/**
 * The `channels:` block of one vertex manifest, or `undefined` where the document has no such key.
 *
 * **A line scanner, like the four others over this format**, and for the same reason: the shape is
 * flat two-space `key: scalar` under `- ` items, and `crates/fossil-sinks` freezes it precisely so
 * that independent parsers agree. `packages/corpus/src/manifest.ts`, `apps/corpus/guards/
 * manifest.mjs` and `src/bound.ts` are the others; this one is here rather than beside `bound.ts`
 * because that module reads `graph.graph.yml`'s `privacy:` block and a channel is per TYPE.
 *
 * An entry whose `scale` is outside the closed set is dropped rather than guessed at: a reader
 * DISPATCHES on that field, and there is no third thing to do with a column. A block of nothing but
 * such entries therefore reads as the EMPTY state — the key was there, and none of what it declared
 * is a channel this reader can draw — which is the honest answer and not the same as *no key*. An
 * entry the payload does not carry is kept: this returns the declaration, and {@link categoricalOf}
 * is where it meets the bytes.
 */
export function readChannels(manifest: string): readonly Channel[] | undefined {
  const lines = manifest.split('\n');
  // `channels: []` is how an empty sequence is written — the same one line `properties: []` is
  // written on — and it is the SECOND state, not the absence of the key.
  const at = lines.findIndex((l) => /^channels:\s*(\[\s*\])?\s*$/.test(l));
  if (at < 0) return undefined;
  if (lines[at]!.includes('[')) return [];

  const entries: Array<Record<string, string>> = [];
  for (const line of lines.slice(at + 1)) {
    const item = /^- ([a-z_]+):\s*(.*)$/.exec(line);
    if (item) {
      entries.push({ [item[1]!]: item[2]!.trim() });
      continue;
    }
    const field = /^ {2}([a-z_]+):\s*(.*)$/.exec(line);
    const current = entries.at(-1);
    if (field && current !== undefined) {
      current[field[1]!] = field[2]!.trim();
      continue;
    }
    // Anything else ends the block: the next top-level key, or the end of the document.
    break;
  }

  return entries
    .filter((raw) => raw.scale === 'categorical' || raw.scale === 'quantitative')
    .map((raw) => ({
      name: raw.name ?? '',
      column: raw.column ?? '',
      scale: raw.scale as Channel['scale'],
      ...(raw.domain === undefined ? {} : { domain: Number(raw.domain) }),
      ...(raw.derived_by === undefined ? {} : { derived_by: raw.derived_by }),
    }));
}

/**
 * The channels declared by whichever of `manifests` is the document for `type` — `undefined` if
 * none of them is, which is the same answer as a document that declares nothing and means the same
 * thing to a reader: derive.
 *
 * Takes the texts rather than a path so that this file reaches no network and no filesystem, and so
 * that the tab (which has the manifests already, from `bench.ts`) and the verifier (which reads
 * them off disk) hand it the same thing.
 */
export function channelsFor(
  manifests: Iterable<string>,
  type: string,
): readonly Channel[] | undefined {
  for (const text of manifests) {
    const declared = /^type:\s*(.+)$/m.exec(text)?.[1]?.trim().replace(/^['"]|['"]$/g, '');
    if (declared === type) return readChannels(text);
  }
  return undefined;
}

/**
 * What this app draws `type` with, or `null` for a corpus that declares no vertex type at all.
 *
 * Pure, and a function of its two arguments: the same corpus and the same declaration answer the
 * same encoding in the tab and in the verifier, which is what makes the verifier worth running.
 */
export function encodingFor(params: EncodingParams): Encoding | null {
  const { channels, quasiIdentifiers = [], type, types } = params;
  const declared =
    type === undefined ? types.vertices[0] : types.vertices.find((v) => v.type === type);
  if (declared === undefined) return null;

  const fill = categoricalOf(declared, channels);
  const carried = new Set(declared.fields.map((f) => f.name));
  // `Person.birth_year` names the column of ONE type; another type's quasi-identifiers are not
  // this one's columns, and on a cross-type corpus they are not even in this relation.
  const quasi = quasiIdentifiers
    .filter((q) => q.startsWith(`${declared.type}.`))
    .map((q) => q.slice(declared.type.length + 1))
    .filter((column) => carried.has(column));

  /**
   * What a binned histogram is the right form for: numeric, not the address, not the position,
   * not the identity, and not the colour — brushing the colour would be brushing the legend.
   */
  const brushable = declared.fields.filter(
    (f) =>
      numeric(f) &&
      f.name !== ADDRESS &&
      !GEOMETRY.includes(f.name) &&
      f.name !== declared.identity &&
      f.name !== fill,
  );
  /**
   * The declarations order the candidates and the bytes decide the set.
   *
   * **A quantitative channel outranks a quasi-identifier, and that is the whole of what this
   * change is for**: which column's distribution is worth a chart is an encoding question, and it
   * was being answered by a disclosure-control field because that field was the only writer-side
   * statement in reach. Where the block declares no quantitative channel the privacy field still
   * ranks — it is a real signal about a real corpus, and an unranked first-numeric-column is a
   * worse chart than the one a reader gets today.
   *
   * Unlike the colour, a `channels:` block that declares none of these does NOT silence the chart:
   * the block says what the type carries as a channel, and a histogram over a column nobody called
   * a channel is still a histogram. The colour is different because `categorical` IS the colour
   * role — there is no other thing it could be declaring.
   */
  const channelBrush = (channels ?? [])
    .filter((c) => c.scale === 'quantitative')
    .map((c) => c.column)
    .find((column) => brushable.some((f) => f.name === column));
  const brush =
    channelBrush ?? brushable.find((f) => quasi.includes(f.name))?.name ?? brushable[0]?.name ?? null;

  return {
    type: declared.type,
    count: Number(declared.count),
    fill,
    label: declared.identity,
    brush,
    // Deduped and in reading order: the address the view is keyed by, the colour, the column the
    // chart bins, and the quasi-identifiers. The brush is listed rather than assumed to be one of
    // them — see {@link Encoding.columns}.
    columns: [
      ...new Set([
        ADDRESS,
        ...(fill === null ? [] : [fill]),
        ...(brush === null ? [] : [brush]),
        ...quasi,
      ]),
    ],
  };
}
