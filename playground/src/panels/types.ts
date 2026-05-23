// Forward-compatible panel contract. 07-04 implemented this with
// <textarea>; 07-05 swaps in Monaco editors while preserving the
// `text` getter/setter shape — the Reset button (07-08) and
// example-load path both depend on this surface.
//
// 07-06 adds the OPTIONAL `onDidChangeContent` hook — only the ShEx
// panel implements it today (so the ShEx editor's onDidChangeContent
// can drive the debounced `fossil/setTargetShex` custom LSP request).
// Other panels can implement it later (e.g. if the playground wants to
// re-trigger `fossil/checkAll` on CSVW edits in a future plan).

export type PanelHandle = {
    get text(): string;
    set text(v: string);
    dispose(): void;
    /**
     * Optional subscription to content-change events. Returns a disposer.
     * When absent, the caller falls back to polling `text` or simply
     * does nothing (the ShEx routing is the only consumer in v0.1).
     */
    onDidChangeContent?(cb: () => void): () => void;
};
