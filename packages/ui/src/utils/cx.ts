/**
 * cx — tiny class-name joiner. Filters out falsy values + joins with a
 * single space.
 *
 * Replaces `cn()` from Keasy's `web/src/lib/utils.ts` which combines
 * clsx + tailwind-merge; @fossil-lang/ui has no Tailwind utility
 * collisions to resolve so a plain concat suffices. Zero dependencies.
 *
 * @example
 *   cx('btn', isActive && 'btn-active', null) // 'btn btn-active'
 */
export type ClassValue = string | number | false | null | undefined;

export function cx(...args: ClassValue[]): string {
  return args.filter((v) => typeof v === 'string' && v.length > 0).join(' ');
}
