/**
 * cx — tiny class-name joiner. Filters out falsy values + joins with a
 * single space.
 *
 * Replaces Keasy's `cn()` helper (which combines a class-list composer with
 * a utility-class deduplicator); @fossil-lang/ui has no utility-class
 * collisions to resolve so a plain concat suffices. Zero dependencies.
 *
 * @example
 *   cx('btn', isActive && 'btn-active', null) // 'btn btn-active'
 */
export type ClassValue = string | number | false | null | undefined;

export function cx(...args: ClassValue[]): string {
  return args.filter((v) => typeof v === 'string' && v.length > 0).join(' ');
}
