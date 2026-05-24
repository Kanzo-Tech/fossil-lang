// Dataset hard limit for the playground (PLAY-12 / Phase-7 SC#4).
//
// Lives on the main thread — refusal happens BEFORE bytes reach the
// DuckDB-WASM Worker. The constant is tunable per deployment without a
// Rust rebuild (and without redeploying any compiler-core crate).
//
// Rationale (see ADR-0026 + 07-RESEARCH §"Pattern 5"):
//   - The DuckDB-WASM heap is recovered ONLY by `worker.terminate()`
//     (P-MOD-1). A 50MB CSV registered into the Worker is unreachable
//     by JS GC and lives until the next Reset.
//   - The UI refusing the bytes at the main thread keeps the heap clean
//     and shifts large-dataset workflows to the CLI (`fossil run`).
//
// 07-08 wires this into BOTH the Run handler (load-bearing check —
// catches paste-into-Run AND drag-drop-then-Run) AND the CSV panel
// onPaste path (belt-and-suspenders; gives faster feedback than waiting
// for Run).

/** Hard limit in bytes for any CSV input the playground will accept. */
export const CSV_LIMIT_BYTES = 10 * 1024 * 1024;   // 10 MB

/** Thrown when `assertWithinLimit` is given content over `CSV_LIMIT_BYTES`. */
export class CsvTooLargeError extends Error {
    readonly sizeBytes: number;
    constructor(sizeBytes: number) {
        const mb = (sizeBytes / 1024 / 1024).toFixed(1);
        super(
            `CSV input is ${mb} MB; playground cap is 10 MB. ` +
            `Install the CLI for larger datasets.`,
        );
        this.name = 'CsvTooLargeError';
        this.sizeBytes = sizeBytes;
    }
}

/**
 * Throws `CsvTooLargeError` if the UTF-8 byte length of `text` exceeds
 * `CSV_LIMIT_BYTES`. Uses `TextEncoder` (NOT `text.length`) so multi-byte
 * code points are counted accurately — the DuckDB Worker measures bytes,
 * not code units.
 */
export function assertWithinLimit(text: string): void {
    const bytes = new TextEncoder().encode(text).byteLength;
    if (bytes > CSV_LIMIT_BYTES) throw new CsvTooLargeError(bytes);
}

/**
 * Show the "use the CLI" refusal modal. Returns a Promise that resolves
 * when the user dismisses it. Uses the browser-native `<dialog>` element
 * — keyboard-dismissable (Esc) for free.
 *
 * Idempotent: if a modal is already in the DOM (rapid double-paste),
 * the existing one is reused and only the message text is updated.
 */
export function showCsvLimitModal(err: CsvTooLargeError): Promise<void> {
    return new Promise(resolve => {
        // Reuse an open modal if present (rapid double-trigger guard).
        const existing = document.querySelector<HTMLDialogElement>(
            'dialog.csv-limit-modal[open]',
        );
        if (existing) {
            const msg = existing.querySelector<HTMLElement>('.csv-limit-msg');
            if (msg) msg.textContent = err.message;
            // Resolve immediately — caller may treat duplicate refusal as
            // a no-op; the first modal will still close normally.
            resolve();
            return;
        }

        const dlg = document.createElement('dialog');
        dlg.className = 'csv-limit-modal';
        dlg.innerHTML = `
            <h3>CSV too large</h3>
            <p class="csv-limit-msg"></p>
            <p>For larger datasets use the CLI:</p>
            <pre>cargo install --git https://github.com/&lt;org&gt;/fossil
fossil run my-mapping.fossil --csv my-data.csv</pre>
            <button id="csv-limit-ok" type="button" autofocus>OK</button>
        `;
        // textContent (not innerHTML) for the message — defence-in-depth
        // against any future change that lets user-controlled bytes near
        // this string.
        const msgEl = dlg.querySelector<HTMLElement>('.csv-limit-msg');
        if (msgEl) msgEl.textContent = err.message;

        document.body.appendChild(dlg);
        const close = (): void => {
            dlg.close();
            dlg.remove();
            resolve();
        };
        dlg.querySelector<HTMLButtonElement>('#csv-limit-ok')!
            .addEventListener('click', close);
        // `<dialog>` fires `cancel` on Esc; clean up the node in that
        // path too so the DOM doesn't accumulate orphan dialogs.
        dlg.addEventListener('cancel', close);
        dlg.showModal();
    });
}
