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

/* DropdownMenu — content surface + item highlight state */
[data-slot="dropdown-menu-content"],
[data-slot="dropdown-menu-sub-content"] {
  background-color: var(--fossil-colors-background);
  color: var(--fossil-colors-foreground);
  border: 1px solid var(--fossil-colors-border);
  border-radius: var(--fossil-radii-lg);
  padding: var(--fossil-spacing-1);
  box-shadow: 0 4px 12px rgba(0, 0, 0, 0.1);
  min-width: 8rem;
}
[data-slot="dropdown-menu-item"],
[data-slot="dropdown-menu-checkbox-item"],
[data-slot="dropdown-menu-radio-item"],
[data-slot="dropdown-menu-sub-trigger"] {
  padding: var(--fossil-spacing-1) var(--fossil-spacing-2);
  border-radius: var(--fossil-radii-sm);
  cursor: pointer;
  outline: none;
  font-size: var(--fossil-fonts-sizeSmall);
  color: var(--fossil-colors-foreground);
  transition: background-color var(--fossil-motion-duration-fast) var(--fossil-motion-easing);
}
[data-slot="dropdown-menu-item"]:hover,
[data-slot="dropdown-menu-item"][data-highlighted],
[data-slot="dropdown-menu-checkbox-item"]:hover,
[data-slot="dropdown-menu-checkbox-item"][data-highlighted],
[data-slot="dropdown-menu-radio-item"]:hover,
[data-slot="dropdown-menu-radio-item"][data-highlighted],
[data-slot="dropdown-menu-sub-trigger"]:hover,
[data-slot="dropdown-menu-sub-trigger"][data-highlighted],
[data-slot="dropdown-menu-sub-trigger"][data-state="open"] {
  background-color: var(--fossil-colors-muted);
  color: var(--fossil-colors-foreground);
}
[data-slot="dropdown-menu-separator"] {
  height: 1px;
  background-color: var(--fossil-colors-border);
  margin: var(--fossil-spacing-1) 0;
}
[data-slot="dropdown-menu-label"] {
  padding: var(--fossil-spacing-1) var(--fossil-spacing-2);
  font-size: var(--fossil-fonts-sizeSmall);
  font-weight: 500;
  color: var(--fossil-colors-muted);
}

/* Tooltip — content surface + fade-in keyframes */
[data-slot="tooltip-content"] {
  background-color: var(--fossil-colors-foreground);
  color: var(--fossil-colors-background);
  padding: var(--fossil-spacing-1) var(--fossil-spacing-2);
  border-radius: var(--fossil-radii-sm);
  font-size: var(--fossil-fonts-sizeSmall);
  line-height: 1.4;
  max-width: 280px;
  z-index: 60;
}
[data-slot="tooltip-content"][data-state="delayed-open"],
[data-slot="tooltip-content"][data-state="instant-open"] {
  animation: fossil-tooltip-fade-in var(--fossil-motion-duration-fast) var(--fossil-motion-easing);
}
@keyframes fossil-tooltip-fade-in {
  from { opacity: 0; transform: translateY(2px); }
  to   { opacity: 1; transform: translateY(0); }
}

/* ScrollArea — visible-on-hover scrollbar pattern */
[data-slot="scroll-area"] {
  position: relative;
  overflow: hidden;
}
[data-slot="scroll-area-viewport"] {
  width: 100%;
  height: 100%;
  border-radius: inherit;
}
[data-slot="scroll-area-scrollbar"] {
  display: flex;
  user-select: none;
  touch-action: none;
  padding: 2px;
  background-color: transparent;
  opacity: 0;
  transition: opacity var(--fossil-motion-duration-fast) var(--fossil-motion-easing),
              background-color var(--fossil-motion-duration-fast) var(--fossil-motion-easing);
}
[data-slot="scroll-area"]:hover [data-slot="scroll-area-scrollbar"],
[data-slot="scroll-area-scrollbar"][data-state="visible"] {
  opacity: 1;
}
[data-slot="scroll-area-scrollbar"][data-state="hidden"] {
  opacity: 0;
}
[data-slot="scroll-area-scrollbar"][data-orientation="vertical"] {
  width: 10px;
  height: 100%;
}
[data-slot="scroll-area-scrollbar"][data-orientation="horizontal"] {
  height: 10px;
  width: 100%;
  flex-direction: column;
}
[data-slot="scroll-area-thumb"] {
  flex: 1;
  background-color: var(--fossil-colors-border);
  border-radius: var(--fossil-radii-full);
  position: relative;
}

/* === plan 10-05: Resizable / Toggle === */

/* Toggle — base affordance + hover + pressed state. Used both by <Toggle>
   (standalone) and <ToggleGroupItem> (which intentionally reuses the same
   data-slot="toggle" selector so a single CSS rule applies to both — see
   ToggleGroupItem in src/primitives/Toggle.tsx). */
[data-slot="toggle"] {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: var(--fossil-spacing-2);
  height: var(--fossil-size-control-base);
  padding: 0 var(--fossil-spacing-2);
  border-radius: var(--fossil-radii-md);
  font-size: var(--fossil-fonts-sizeSmall);
  font-weight: 500;
  background: transparent;
  color: var(--fossil-colors-foreground);
  border: 1px solid transparent;
  cursor: pointer;
  white-space: nowrap;
  transition: background-color var(--fossil-motion-duration-fast) var(--fossil-motion-easing),
              border-color var(--fossil-motion-duration-fast) var(--fossil-motion-easing),
              color var(--fossil-motion-duration-fast) var(--fossil-motion-easing);
}
[data-slot="toggle"]:hover {
  background-color: var(--fossil-colors-muted);
}
[data-slot="toggle"][data-state="on"] {
  background-color: var(--fossil-colors-muted);
  color: var(--fossil-colors-foreground);
}
[data-slot="toggle"][data-variant="outline"] {
  border-color: var(--fossil-colors-border);
}
[data-slot="toggle"][data-variant="outline"]:hover {
  background-color: var(--fossil-colors-muted);
}
[data-slot="toggle-group"] {
  display: inline-flex;
  align-items: center;
  gap: var(--fossil-spacing-1);
}

/* Resizable — panel group layout + handle hover affordance.
   react-resizable-panels sets its own [data-resize-handle-state="hover|drag"]
   attribute during pointer interactions (in addition to the standard :hover);
   we key off both for parity with the Keasy reference. */
[data-slot="resizable-panel-group"] {
  display: flex;
  width: 100%;
  height: 100%;
}
[data-slot="resizable-panel-group"][data-panel-group-direction="vertical"] {
  flex-direction: column;
}
[data-slot="resizable-handle"] {
  position: relative;
  flex: 0 0 1px;
  background-color: var(--fossil-colors-border);
  transition: background-color var(--fossil-motion-duration-fast) var(--fossil-motion-easing);
}
[data-slot="resizable-handle"]:hover,
[data-slot="resizable-handle"][data-resize-handle-state="hover"],
[data-slot="resizable-handle"][data-resize-handle-state="drag"] {
  background-color: var(--fossil-colors-muted);
}
[data-slot="resizable-handle"][data-panel-group-direction="horizontal"] {
  width: 1px;
  height: 100%;
  cursor: col-resize;
}
[data-slot="resizable-handle"][data-panel-group-direction="vertical"] {
  height: 1px;
  width: 100%;
  cursor: row-resize;
}
[data-slot="resizable-handle-grip"] {
  position: absolute;
  inset: 50% auto auto 50%;
  transform: translate(-50%, -50%);
  display: flex;
  align-items: center;
  justify-content: center;
  background-color: var(--fossil-colors-border);
  border-radius: var(--fossil-radii-sm);
  width: 12px;
  height: 12px;
  font-size: 10px;
  line-height: 1;
  color: var(--fossil-colors-muted);
}
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
