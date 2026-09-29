import { ServerCodeBlock } from "fumadocs-ui/components/codeblock.rsc";
import { excerpt, inContext } from "@/lib/programs";
import { loadFossilGrammar } from "@/lib/fossil-grammar";

export interface ProgramProps {
  /** Path inside `programs/`, e.g. `shop/shop.fossil` — or inside the repository, with `repo`. */
  src: string;
  /** A `#region` the file declares. Omit for the whole file. */
  region?: string;
  /** Heading on the block. Defaults to the path, which is what a reader needs to find the file. */
  title?: string;
  /** Resolve `src` from the repository root instead of the corpus. For the one normative file. */
  repo?: boolean;
  /**
   * Show the whole file with `region`'s lines highlighted, instead of cutting the region out.
   *
   * For a page that walks one program section by section: the reader sees the same file every time
   * and the highlight moves down it. Needs `region`; without one there is nothing to mark.
   */
  context?: boolean;
}

/**
 * Transclude a program, one region of it, or one region marked inside it.
 *
 *     <Program src="shop/shop.fossil" />
 *     <Program src="shop/shop.fossil" region="identity" />
 *     <Program src="shop/shop.fossil" region="identity" context />
 *
 * A server component, and that is the whole mechanism: the file is read where the page is built, so
 * a page cannot show a program that is not on disk and a region cannot outlive the lines it named.
 */
export async function Program({ src, region, title, repo = false, context = false }: ProgramProps) {
  if (context && !region) {
    throw new Error(`<Program src="${src}" context> needs a region; there is nothing to highlight.`);
  }

  await loadFossilGrammar();
  const { code, lang, highlighted } =
    context && region ? inContext(src, region, repo) : excerpt(src, region, repo);

  const marked = new Set(highlighted);

  return (
    <ServerCodeBlock
      code={code}
      lang={lang}
      /*
       * The class, not a Shiki meta string. `ServerCodeBlock` does pass `meta.__raw` through to
       * `codeToHast`, but nothing in this install reads it: fumadocs bundles four of Shiki's comment
       * transformers and `transformerMetaHighlight` — the one that understands `{1-3}` — is not
       * among them, so the meta form highlights nothing and fails silently. `highlighted` is the
       * class `transformerNotationHighlight` sets and the one `fumadocs-ui/css/lib/shiki.css`
       * styles, so a marked line here looks exactly like a `[!code highlight]` line anywhere else.
       */
      transformers={[
        {
          name: "fossil:region",
          line(node, line) {
            if (!marked.has(line)) return;
            const existing = node.properties.class;
            node.properties.class = existing ? `${existing} highlighted` : "highlighted";
          },
        },
      ]}
      codeblock={{ title: title ?? (region ? `${src} — ${region}` : src) }}
    />
  );
}
