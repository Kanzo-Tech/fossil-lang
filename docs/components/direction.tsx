/**
 * The direction, rendered from frontmatter rather than written in prose.
 *
 * It is drawn by the page shell, above the body, so a reader never has to infer whether a sentence
 * describes what is built or what is intended — a page that declares a direction says so at the
 * top, once, and the body is then free to describe the thing itself.
 *
 * There used to be a second block here, `today`, carrying a `backedBy` path. It was retired: the
 * guard behind it could only prove the cited file existed, never that it asserted the claim, and
 * eight of forty-nine citations had drifted to a line that said something else while CI stayed
 * green. What replaced it is not a wider guard — it is transclusion. `<Program src= region= />`
 * reads the file at build time and a region that no longer exists is a build failure, which is the
 * property `backedBy` was reaching for and never had.
 */

export interface Direction {
  summary: string;
  /**
   * A route on this site, when the argument is somewhere else. Absent when the page argues its own
   * direction below — pointing at itself would be a restatement, not a citation.
   */
  arguedIn?: string;
}

export function DirectionNote({ direction }: { direction?: Direction }) {
  if (!direction) return null;

  return (
    <div className="my-6 flex flex-col gap-3">
      <div className="register">
        <span className="register__label">Where this is going</span>
        <p className="m-0">{direction.summary}</p>
        {direction.arguedIn ? (
          <span className="register__cite">
            The argument is in <a href={direction.arguedIn}>{direction.arguedIn}</a>
          </span>
        ) : null}
      </div>
    </div>
  );
}
