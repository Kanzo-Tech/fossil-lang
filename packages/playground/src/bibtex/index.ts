/**
 * BibTeX cite modal module barrel (PLAY-08).
 *
 * Public API for the cite modal + the underlying citation templates.
 * Consumers re-import these via `@fossil-lang/playground` (see
 * `../index.tsx` for the public re-exports).
 */

export { BibtexModal, type BibtexModalProps } from './BibtexModal.js';
export {
  HARTIG_BIBTEX,
  HARTIG_PLAINTEXT,
  buildPlaygroundBibtex,
  buildPlaygroundPlaintext,
} from './cite-templates.js';
