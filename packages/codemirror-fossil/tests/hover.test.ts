/**
 * The hover half: the Markdown renderer, and what the source is asked.
 *
 * The renderer is worth testing because it is deliberately small — three
 * constructs, and a promise that everything else stays legible rather than
 * disappearing. The failure mode of a hand-rolled Markdown renderer is silent
 * loss, and a hover that drops the second block is a hover that has stopped
 * being bidirectional without saying so.
 */
import { describe, expect, it } from 'vitest';

import { renderMarkdown } from '../src/hover.js';

/** What `render_markdown_bidirectional` emits for a resolved target. */
const BIDIRECTIONAL = [
  '```fossil',
  'String',
  '```',
  '',
  '*from InputDescriptor { source_name: "User", column: "name" }*',
  '',
  '```fossil',
  'Integer',
  '```',
  '',
  '*target type (ShEx shape constraint)*',
].join('\n');

describe('renderMarkdown', () => {
  it('keeps both fenced blocks, which is what makes hover bidirectional', () => {
    const dom = renderMarkdown(BIDIRECTIONAL);
    const code = [...dom.querySelectorAll('pre')].map((p) => p.textContent);
    expect(code).toEqual(['String', 'Integer']);
  });

  it('keeps the prose between the blocks, italicised', () => {
    const dom = renderMarkdown(BIDIRECTIONAL);
    const ems = [...dom.querySelectorAll('em')].map((e) => e.textContent);
    expect(ems).toContain('target type (ShEx shape constraint)');
    expect(dom.textContent).toContain('InputDescriptor');
  });

  it('renders inline code as code', () => {
    const dom = renderMarkdown('field type: `String`');
    expect(dom.querySelector('code')?.textContent).toBe('String');
  });

  it('does not lose text it has no construct for', () => {
    // A heading is not in the vocabulary. It must survive as text rather than
    // vanish — a renderer that silently drops what it does not know is how a
    // hover loses half its content and nothing goes red.
    const dom = renderMarkdown('## a heading nobody emits yet');
    expect(dom.textContent).toContain('a heading nobody emits yet');
  });

  it('treats a type name containing markup as text', () => {
    // No `innerHTML` anywhere: `List<String>` is a type, not an element.
    const dom = renderMarkdown('```fossil\nList<String>\n```');
    expect(dom.querySelector('pre')?.textContent).toBe('List<String>');
    expect(dom.querySelector('string')).toBeNull();
  });
});
