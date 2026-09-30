import { FossilError, type DocumentWorkspace, type Host, type UnreadDocument } from '@fossil-lang/types';

import { read } from './objects.js';

/**
 * Read every document a workspace is missing through `host`, until nothing new is missing: a
 * registered document can name another.
 *
 * The one IO loop over {@link Host}. Fossil says what is missing, where it lives and which
 * connection it goes through; this reads and registers. Each document is attempted once, so one
 * that cannot be read ends the loop instead of repeating it.
 */
export async function resolveDocuments(
  workspace: DocumentWorkspace,
  host: Host,
): Promise<{ registered: number; unread: UnreadDocument[] }> {
  const attempted = new Set<string>();
  const unread: UnreadDocument[] = [];
  let registered = 0;
  workspace.setConnections(await connections(host));
  const decoder = new TextDecoder();

  for (;;) {
    const pending = workspace.missingDocuments().filter((d) => !attempted.has(d.key));
    if (pending.length === 0) return { registered, unread };
    for (const d of pending) attempted.add(d.key);

    const results = await read(host, pending);
    for (const [i, d] of pending.entries()) {
      const result = results[i]!;
      if (result.ok) {
        workspace.registerDocument(d.key, decoder.decode(result.bytes));
        registered++;
      } else {
        unread.push({ ...d, problem: result.problem });
      }
    }
  }
}

async function connections(host: Host): Promise<Record<string, string>> {
  try {
    return await host.connections();
  } catch (cause) {
    throw FossilError.of('storage/host-refused', { scope: 'connections' }, 'the host refused connections', { cause });
  }
}
