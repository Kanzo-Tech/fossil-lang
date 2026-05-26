/**
 * Inject a singleton stylesheet for primitives' state pseudo-selectors.
 *
 * Inline styles can't express :hover / :focus-visible / [data-state="active"]
 * overrides, so we ship a small static stylesheet (~1-2 KB) referencing
 * --fossil-* tokens. Token values are resolved at runtime by the CSS-variable
 * cascade — the stylesheet itself has no hardcoded colors.
 *
 * Idempotent: re-injecting (on each primitive's mount or on React Strict
 * Mode double-render) is a no-op via the `STYLE_ID` element-presence check.
 *
 * SSR-safe: guards on `typeof document !== 'undefined'` so Next.js RSC
 * boundaries (server components rendering the component tree) don't crash.
 * The stylesheet is injected on first client-side mount of any primitive.
 *
 * **Plan ownership**: This file is owned by Plan 10-03 (Tabs/Dialog/Separator).
 * Plans 10-04 (DropdownMenu/Tooltip/ScrollArea) and 10-05 (Resizable/Toggle)
 * EXTEND the STYLESHEET below by adding new sections to their delimited
 * blocks (search for `=== plan 10-04 ===` / `=== plan 10-05 ===` markers).
 * Do NOT mutate sections owned by other plans.
 *
 * Per ADR-0034 + ADR-0033.
 */

const STYLE_ID = 'fossil-ui-primitive-styles';

const STYLESHEET = `
/* === plan 10-03: generic focus + disabled === */

[data-fossil-ui-primitive] *:focus-visible {
  outline: none;
  box-shadow: var(--fossil-focus-ring);
}

/* Any element marked [disabled] or [data-disabled] by Radix gets the same
   reduced-affordance treatment regardless of primitive. */
[data-slot][disabled],
[data-slot][data-disabled],
[data-slot] [disabled],
[data-slot] [data-disabled] {
  opacity: 0.5;
  pointer-events: none;
}

/* === plan 10-03: Tabs === */

[data-slot="tabs-trigger"] {
  position: relative;
  cursor: pointer;
}
[data-slot="tabs-trigger"][data-state="inactive"] {
  color: var(--fossil-colors-muted);
}
[data-slot="tabs-trigger"][data-state="inactive"]:hover {
  color: var(--fossil-colors-foreground);
}
[data-slot="tabs-trigger"][data-state="active"] {
  color: var(--fossil-colors-foreground);
}
[data-slot="tabs-list"][data-variant="default"] [data-slot="tabs-trigger"][data-state="active"] {
  background-color: var(--fossil-colors-background);
  box-shadow: 0 1px 2px 0 rgba(0, 0, 0, 0.05);
}
[data-slot="tabs-list"][data-variant="line"] [data-slot="tabs-trigger"][data-state="active"]::after {
  content: '';
  position: absolute;
  inset-inline: 0;
  bottom: -5px;
  height: 2px;
  background-color: var(--fossil-colors-foreground);
}

/* === plan 10-03: Dialog === */

[data-slot="dialog-overlay"] {
  position: fixed;
  inset: 0;
  background-color: rgba(15, 23, 42, 0.5);
  /* Slate-900 at 50% opacity — pre-resolved rgba per ADR-0034 rule 4.
     Hosts can override --fossil-dialog-overlay if a token slot is added later. */
}

/* === plan 10-04: DropdownMenu / Tooltip / ScrollArea === */
/* (Empty in this plan; Plan 10-04 appends its selectors here.) */

/* === plan 10-05: Resizable / Toggle === */
/* (Empty in this plan; Plan 10-05 appends its selectors here.) */
`.trim();

export function injectFossilUiStyles(): void {
  if (typeof document === 'undefined') return; // SSR guard
  if (document.getElementById(STYLE_ID)) return; // idempotent
  const style = document.createElement('style');
  style.id = STYLE_ID;
  style.setAttribute('data-fossil-ui', 'true');
  style.textContent = STYLESHEET;
  document.head.appendChild(style);
}
