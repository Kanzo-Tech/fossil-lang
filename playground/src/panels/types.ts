// Forward-compatible panel contract. 07-04 implemented this with
// <textarea>; 07-05 swaps in Monaco editors while preserving the
// `text` getter/setter shape — the Reset button (07-08) and
// example-load path both depend on this surface.

export type PanelHandle = {
    get text(): string;
    set text(v: string);
    dispose(): void;
};
