/**
 * The two registers, rendered from frontmatter rather than written in prose.
 *
 * They are drawn by the page shell, above the body, so a reader never has to infer which one a
 * sentence is in — the answer is at the top of every page, in two blocks that do not look alike.
 * Nothing under `characteristics/` or `protocols/` may carry only one of them; `content.test.ts`
 * fails the build if it does.
 */

export interface Direction {
  summary: string;
  decidedBy: string;
}

export interface Today {
  summary: string;
  backedBy?: string;
  unmeasured?: true;
}

export function Registers({ direction, today }: { direction?: Direction; today?: Today }) {
  if (!direction && !today) return null;

  return (
    <div className="my-6 flex flex-col gap-3">
      {direction ? (
        <div className="register">
          <span className="register__label">Where this is going</span>
          <p className="m-0">{direction.summary}</p>
          <span className="register__cite">
            Decided by <code>{direction.decidedBy}</code>
          </span>
        </div>
      ) : null}
      {today ? (
        <div className="register register--today">
          <span className="register__label">True today</span>
          <p className="m-0">{today.summary}</p>
          <span className="register__cite">
            {today.backedBy ? (
              <>
                Backed by <code>{today.backedBy}</code>
              </>
            ) : (
              <>Unmeasured — nothing on disk would go red if this stopped being true.</>
            )}
          </span>
        </div>
      ) : null}
    </div>
  );
}
