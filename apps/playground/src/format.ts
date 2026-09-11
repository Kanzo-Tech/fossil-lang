/**
 * The three formatters every ledger in this app prints through.
 *
 * Here rather than beside one of them because both ledgers and the prose around them use the same
 * three, and a second `toLocaleString` with a different default is how two numbers on one screen
 * come to disagree about what a thousand separator is.
 */
export const KB = (bytes: number) => `${(bytes / 1024).toFixed(0)} kB`;
export const MB = (bytes: number) => `${(bytes / 1024 / 1024).toFixed(1)} MB`;
export const n = (value: number) => value.toLocaleString();
