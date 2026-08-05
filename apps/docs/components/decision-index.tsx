import { readDecisions } from "@/lib/decisions";

/**
 * A server component, and that is the point: it reads `decisions/` at build time, so the table
 * below cannot disagree with the directory. A page that transcribes a list is a page that is wrong
 * one commit later.
 */
export function DecisionIndex() {
  const decisions = readDecisions();

  return (
    <div className="overflow-x-auto">
      <table>
        <thead>
          <tr>
            <th>#</th>
            <th>Title</th>
            <th>Status</th>
            <th>Date</th>
          </tr>
        </thead>
        <tbody>
          {decisions.map((decision) => (
            <tr key={decision.file}>
              <td>{decision.number ?? "—"}</td>
              <td>{decision.title}</td>
              <td>{decision.status ?? "—"}</td>
              <td>{decision.date ?? "—"}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <p className="text-fd-muted-foreground text-sm">
        {decisions.length} records, read from <code>decisions/</code> at build time.
      </p>
    </div>
  );
}
