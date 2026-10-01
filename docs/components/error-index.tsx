import Link from "next/link";
import { catalogue } from "@/lib/problems";

/**
 * Every live code, grouped by area, rendered from the catalogue rather than transcribed — a code
 * added or merged changes this list in the same commit, and no page here restates it.
 */
export function ErrorIndex() {
  const areas = new Map<string, { code: string; title: string }[]>();
  for (const entry of catalogue()) {
    const area = entry.code.split("/")[0] as string;
    areas.set(area, [...(areas.get(area) ?? []), entry]);
  }

  return (
    <div className="not-prose my-6 flex flex-col gap-6">
      {[...areas].map(([area, entries]) => (
        <section key={area}>
          <h3 className="text-base font-semibold">
            <code>{area}</code>
          </h3>
          <ul className="mt-2 flex flex-col gap-1 text-sm">
            {entries.map(({ code, title }) => (
              <li key={code}>
                <Link href={`/docs/errors/${code}`} className="underline-offset-4 hover:underline">
                  <code>{code}</code>
                </Link>{" "}
                — {title}
              </li>
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}
