// Semantic-tokens legend mirror.
//
// The authoritative source is the LSP server's `initialize` response
// (`textDocument.semanticTokens.legend`). This file is a fallback used
// only if a consumer needs the legend before the server has responded,
// or in tests.
//
// KEEP this synced with `crates/fossil-ide/src/semantic.rs`
// `semantic_legend()` — order matters (the LSP wire protocol uses
// numeric indices into the legend).

export const semanticTokensLegend = {
    tokenTypes: [
        'keyword', 'string', 'number', 'comment', 'operator',
        'variable', 'function', 'type', 'parameter', 'property',
    ],
    tokenModifiers: [] as string[],
};
