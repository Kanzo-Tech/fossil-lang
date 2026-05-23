// Monaco language registration for `.fossil`.
//
// No Monarch tokenizer: semantic tokens from the LSP do the highlighting.
// Call `registerFossilLanguage()` BEFORE creating any model with
// language === 'fossil' (Monaco refuses to attach an unregistered id).

import * as monaco from 'monaco-editor';

export function registerFossilLanguage(): void {
    monaco.languages.register({
        id: 'fossil',
        extensions: ['.fossil'],
        aliases: ['Fossil'],
    });
    // Minimal language configuration — comment marker is `//` (Fossil syntax).
    monaco.languages.setLanguageConfiguration('fossil', {
        comments: { lineComment: '//' },
        brackets: [
            ['{', '}'],
            ['[', ']'],
            ['(', ')'],
            ['<', '>'],
        ],
    });
}
