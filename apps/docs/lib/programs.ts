import { readFileSync } from "node:fs";
import { join, extname, isAbsolute, normalize, sep } from "node:path";
import { repoRoot } from "@/lib/repo";

/**
 * The programs are the corpus, and the prose is a view of them.
 *
 * No page contains a fossil program. It names one, and this module reads it off disk at build
 * time. That inverts the usual arrangement — extracting fenced code out of prose and compiling it
 * makes the prose the source of truth, and a fragment that needs surrounding context to compile
 * forces a fake wrapper or a hidden line, both of which put a program on the page that is not the
 * program that ran. Here the file is the truth, and a region that no longer exists is a build
 * failure rather than a paragraph nobody re-read.
 */

/** Where the corpus lives. One directory per program, its data beside it. */
export const programsRoot = join(process.cwd(), "programs");

/**
 * A region marker, in the comment syntax of whichever file carries it: `//` in a fossil program,
 * `#` in a ShEx or Turtle document.
 *
 *     // #region identity
 *     @subject = "https://shop.example/user/{User.email}"
 *     // #endregion identity
 *
 * The name on `#endregion` is optional. Regions may nest and may overlap, because the anatomy page
 * shows the same lines under two different headings; every marker line is stripped from every
 * slice, so a region never leaks the scaffolding of another.
 */
const MARKER = /^\s*(?:\/\/|#)\s*#(region|endregion)\b\s*(\S*)/;

export interface Excerpt {
  code: string;
  /** Shiki language id, chosen from the extension. */
  lang: string;
  /**
   * 1-based line numbers **within `code`** to mark as highlighted. Empty unless the caller asked
   * for a region in context, in which case it is exactly the lines the region covers.
   */
  highlighted: number[];
}

/** Read a corpus file, or the repository file a reference page transcludes whole. */
function readSource(src: string, fromRepoRoot: boolean): string {
  if (isAbsolute(src) || normalize(src).split(sep).includes("..")) {
    throw new Error(`<Program src="${src}"> must be a path inside the corpus, with no "..".`);
  }

  const path = join(fromRepoRoot ? repoRoot : programsRoot, src);

  try {
    return readFileSync(path, "utf8");
  } catch {
    throw new Error(`<Program src="${src}"> names a file that is not on disk: ${path}`);
  }
}

/** Remove the deepest common indentation, so a nested region reads flush left. */
function dedent(lines: string[]): string[] {
  const widths = lines
    .filter((line) => line.trim().length > 0)
    .map((line) => line.length - line.trimStart().length);

  const common = widths.length > 0 ? Math.min(...widths) : 0;
  return lines.map((line) => line.slice(common));
}

function trimBlankEdges(lines: string[]): string[] {
  let start = 0;
  let end = lines.length;
  while (start < end && lines[start].trim() === "") start += 1;
  while (end > start && lines[end - 1].trim() === "") end -= 1;
  return lines.slice(start, end);
}

/**
 * Extension to Shiki language.
 *
 * `fossil` is ours — `lib/fossil-grammar.ts` registers it. ShEx has no grammar in the bundle and
 * borrows Turtle's, which carries the two things that matter in a shape document: `PREFIX` and the
 * angle-bracketed IRI.
 */
const LANGUAGES: Record<string, string> = {
  ".fossil": "fossil",
  ".shex": "turtle",
  ".ttl": "turtle",
  ".csv": "csv",
  ".json": "json",
  ".yaml": "yaml",
  ".yml": "yaml",
  ".nt": "turtle",
  ".bnf": "text",
  ".txt": "text",
};

/** One line of the file with every marker removed, and whether the named region covers it. */
interface Scanned {
  text: string;
  inRegion: boolean;
}

/**
 * Strip the markers and record which surviving lines the named region covers.
 *
 * One pass answers both questions the two modes ask, which is why they share it: slicing a region
 * out and marking it in place differ only in what they do with `inRegion` afterwards. `found` is
 * separate from "any line is in the region" on purpose — an empty region is declared, and only an
 * *undeclared* one is the error.
 */
function scan(src: string, lines: string[], region?: string): { lines: Scanned[]; found: boolean } {
  const out: Scanned[] = [];
  let depth = 0;
  let found = false;

  for (const line of lines) {
    const marker = MARKER.exec(line);

    if (marker) {
      const [, kind, name] = marker;
      if (kind === "region" && name === region) {
        depth += 1;
        found = true;
      } else if (kind === "endregion" && (name === region || name === "") && depth > 0) {
        depth -= 1;
      }
      continue;
    }

    out.push({ text: line, inRegion: depth > 0 });
  }

  if (region !== undefined && !found) {
    throw new Error(
      `<Program src="${src}" region="${region}"> names a region the file does not declare.`,
    );
  }

  return { lines: out, found };
}

function trimBlankEdgesOf(lines: Scanned[]): Scanned[] {
  let start = 0;
  let end = lines.length;
  while (start < end && lines[start].text.trim() === "") start += 1;
  while (end > start && lines[end - 1].text.trim() === "") end -= 1;
  return lines.slice(start, end);
}

/**
 * A file, or one named region of it, ready to render.
 *
 * A missing region throws rather than falling back to the whole file. Silence is the failure this
 * whole arrangement exists to remove: a page that quietly shows more than it meant to is a page
 * that has already drifted.
 */
export function excerpt(src: string, region?: string, fromRepoRoot = false): Excerpt {
  const raw = readSource(src, fromRepoRoot);
  const lang = LANGUAGES[extname(src)] ?? "text";
  const { lines } = scan(src, raw.replace(/\r\n/g, "\n").split("\n"), region);

  if (!region) {
    return { code: trimBlankEdges(lines.map((line) => line.text)).join("\n"), lang, highlighted: [] };
  }

  const kept = lines.filter((line) => line.inRegion).map((line) => line.text);
  return {
    code: trimBlankEdges(dedent(trimBlankEdges(kept))).join("\n"),
    lang,
    highlighted: [],
  };
}

/**
 * The whole file, with one region's lines marked instead of cut out.
 *
 * The reason this mode exists: a page that walks a program section by section — `anatomy.mdx` walks
 * `shop.fossil` through seven of them — shows the reader seven excerpts with nothing between them,
 * and the thing the page is actually teaching is *where each piece sits in the program*. Marking the
 * lines in place shows the same seven steps against one unchanging sixteen-line file.
 *
 * Line numbers are computed **after** the markers are stripped and the blank edges trimmed, because
 * that is the text Shiki will number. Nothing is dedented here: the file's own indentation is a
 * carrier of meaning once the surrounding lines are visible — in a fossil program the indentation
 * under a mapping header is what opens the block.
 */
export function inContext(src: string, region: string, fromRepoRoot = false): Excerpt {
  const raw = readSource(src, fromRepoRoot);
  const lang = LANGUAGES[extname(src)] ?? "text";
  const lines = trimBlankEdgesOf(scan(src, raw.replace(/\r\n/g, "\n").split("\n"), region).lines);

  return {
    code: lines.map((line) => line.text).join("\n"),
    lang,
    highlighted: lines.flatMap((line, index) => (line.inRegion ? [index + 1] : [])),
  };
}
