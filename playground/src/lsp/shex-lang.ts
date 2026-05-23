// Monaco language registration for ShEx (Shape Expressions).
//
// LSP is not wired for ShEx in v0.1 (the fossil-wasm Worker only handles
// `.fossil` documents). A minimal Monarch tokenizer gives users readable
// syntax highlighting in the ShEx panel.

import * as monaco from 'monaco-editor';

export function registerShexLanguage(): void {
    monaco.languages.register({
        id: 'shex',
        extensions: ['.shex'],
        aliases: ['ShEx'],
    });
    monaco.languages.setMonarchTokensProvider('shex', {
        keywords: ['PREFIX', 'BASE', 'CLOSED', 'EXTRA', 'START', 'OR', 'AND'],
        tokenizer: {
            root: [
                [/\/\/.*$/, 'comment'],
                [/<[^>]*>/, 'string.iri'],
                [/"([^"\\]|\\.)*"/, 'string'],
                [/[A-Z_]+(?=\s)/, { cases: { '@keywords': 'keyword', '@default': 'identifier' } }],
                [/[a-zA-Z_][\w-]*:[a-zA-Z_][\w-]*/, 'type'],
                [/\d+/, 'number'],
            ],
        },
    });
    monaco.languages.setLanguageConfiguration('shex', {
        comments: { lineComment: '//' },
    });
}
