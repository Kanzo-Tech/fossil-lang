/**
 * What to draw the corpus **with** — read off the corpus, rather than written down in the app.
 *
 * `Canvas.tsx` carried the answers as string literals: `fill="cluster_id"` on the renderer,
 * `field="birth_year"` on the chart, and `dense_id, birth_year, postcode, cluster_id` in the
 * crossfilter's view. Every one of them is a fact about *the bench corpus*, in a component that
 * claims to be a reader of any corpus — so pointing this app at a second one drew a blank canvas
 * and a chart over a column that is not there, with no error saying which of the four was wrong.
 *
 * ## The manifest declares no encoding, and that is a finding rather than a gap
 *
 * The obvious source is the vertex manifest's `properties:`, and it is not one. A property is a
 * name, a data type and `is_primary` — [`/docs/design/position`](/docs/design/position) makes the
 * same observation about coordinates, in the same words: *a property is a name and a type*. There
 * is no field in which a writer could say *this column is the community*, and after that page's
 * argument there is a good reason not to add one: a default is a writer deciding a reader's
 * question. So this module is the reader deciding it, out loud and in one place.
 *
 * ## Which half of the corpus answers which question
 *
 * `openCorpus` is explicit that the manifest's vocabulary and the payload's are two different
 * lists — three declared properties against seven columns on disk for the bench corpus — and the
 * split runs straight through this file:
 *
 * - **The bytes say what is drawable.** `cluster_id`, `x`, `y` and `dense_id` are the layout pass's
 *   output and are declared by no `properties:` entry, so a derivation that believed the manifest
 *   would find no column to colour by on a corpus that has one. {@link Encoding.fill} and
 *   {@link Encoding.label} come from `Corpus.types`, which is one `DESCRIBE` per type.
 * - **The manifest says which one is interesting.** Of the columns the bytes say are drawable,
 *   which the reader should put a chart over is a question the payload cannot answer — and
 *   `graph.graph.yml` answers it anyway, for a different reason: `quasi_identifiers` names the
 *   columns whose distribution is a re-identification risk, which is exactly the set worth showing
 *   a reader alongside the bound that was reached. {@link Encoding.brush} prefers one.
 *
 * Neither half is consulted about the other, and the fallbacks are stated rather than implied: a
 * corpus with no privacy block still gets a chart, and a corpus with no integral column gets a
 * canvas of one colour rather than an exception.
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
import type { CorpusField, CorpusTypes, CorpusVertexType } from '@fossil-lang/corpus';

/**
 * The column the addressing numbers rows by. Never drawn, never brushed: it is the row's ADDRESS,
 * and a histogram over it is a picture of the Morton curve.
 */
const ADDRESS = 'dense_id';

/** The position. Drawn as the position, so it is never also a colour or a chart. */
const GEOMETRY = ['x', 'y'];

/**
 * The community column, named once in this app instead of at three call sites.
 *
 * It is a name and not a derivation because nothing in the artefact makes it derivable — see the
 * header — and `crates/fossil-layout` is what writes it, under this name, into every corpus the
 * pass touches. `FrameParams.fill` in `@fossil-lang/corpus` defaults to the same string for the
 * same reason, which is why a request that names no column still draws the right picture.
 *
 * Preferred rather than assumed: {@link categoricalOf} falls back to the first integral column a
 * corpus written by something else happens to carry, and to nothing at all if it carries none.
 */
const COMMUNITY = 'cluster_id';

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
   * The crossfilter view's projection: the address, the colour, and the declared
   * quasi-identifiers the payload actually carries.
   *
   * Narrow on purpose. `label` is excluded even where it exists — it is the widest column in the
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
export function categoricalOf(type: CorpusVertexType): string | null {
  const drawable = type.fields.filter(
    (f) => f.name !== ADDRESS && !GEOMETRY.includes(f.name) && f.name !== type.identity,
  );
  const community = drawable.find((f) => f.name === COMMUNITY);
  if (community !== undefined && integral(community)) return community.name;
  return drawable.find(integral)?.name ?? null;
}

/**
 * What this app draws `type` with, or `null` for a corpus that declares no vertex type at all.
 *
 * Pure, and a function of its two arguments: the same corpus and the same declaration answer the
 * same encoding in the tab and in the verifier, which is what makes the verifier worth running.
 */
export function encodingFor(params: EncodingParams): Encoding | null {
  const { quasiIdentifiers = [], type, types } = params;
  const declared =
    type === undefined ? types.vertices[0] : types.vertices.find((v) => v.type === type);
  if (declared === undefined) return null;

  const fill = categoricalOf(declared);
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
  // The declaration orders the candidates and the bytes decide the set: a declared quasi-identifier
  // that is not numeric is not a chart, and a numeric column nobody declared is still a chart.
  const brush =
    brushable.find((f) => quasi.includes(f.name))?.name ?? brushable[0]?.name ?? null;

  return {
    type: declared.type,
    count: Number(declared.count),
    fill,
    label: declared.identity,
    brush,
    columns: [ADDRESS, ...(fill === null ? [] : [fill]), ...quasi.filter((c) => c !== fill)],
  };
}
