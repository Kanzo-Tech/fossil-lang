import { GUARDS } from "../guards/guards.mjs";

/**
 * The contract, rendered from the code that enforces it.
 *
 * A server component, and that is the whole point: it reads `guards/guards.mjs` at build time, so
 * the page below cannot describe a guard that does not exist, omit one that does, or paraphrase
 * what one proves. A page that transcribes a list is a page that is wrong one commit later — and
 * this particular list *is* the contract.
 *
 * The `cannotProve` half is given equal weight on purpose. The corpus is documented rather than
 * typed; the price of that choice is that a guard checks what it was asked to check and nothing
 * more, and a reader who does not see the limit beside the claim has not been told the price.
 */
export function GuardIndex() {
  return (
    <div className="not-prose my-6 flex flex-col gap-6">
      {GUARDS.map((guard) => (
        <section key={guard.id}>
          <h3 className="text-base font-semibold">
            <code>{guard.id}</code> — {guard.title}
          </h3>
          <p className="mt-1 text-sm text-fd-muted-foreground">{guard.proves}</p>
          <div className="guard-limit mt-2 text-sm">
            <span className="guard-limit__label">what it cannot prove</span>
            {guard.cannotProve}
          </div>
        </section>
      ))}
    </div>
  );
}
