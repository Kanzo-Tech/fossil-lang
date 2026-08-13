"use client";

import { useTheme } from "next-themes";
import { useEffect, useId, useRef, useState } from "react";

/**
 * The target of `remarkMdxMermaid`, which is fumadocs' own plugin — it rewrites a ```mermaid fence
 * into `<Mermaid chart="…" />` and leaves the rendering entirely to whoever supplies the component.
 *
 * Rendered in the browser, not at build time, and that is a cost accepted rather than overlooked.
 * The build-time route is `rehype-mermaid`, which renders through `mermaid-isomorphic`, whose peer
 * dependency is `playwright` — mermaid needs a DOM and real text metrics, so rendering it in Node
 * means downloading a browser at install time. That is a large, network-bound step added to a
 * path-filtered docs workflow so that two diagrams can be SVG a few hundred milliseconds sooner.
 *
 * The lazy `import("mermaid")` is what keeps the bill on the page that owes it: the library lands in
 * an async chunk that is fetched when this effect runs, so a docs page with no diagram never asks
 * for it. Only the wrapper below is in the shared MDX bundle.
 */
export function Mermaid({ chart }: { chart: string }) {
  const id = useId().replace(/[^a-zA-Z0-9]/g, "");
  const container = useRef<HTMLDivElement>(null);
  const [svg, setSvg] = useState("");
  const { resolvedTheme } = useTheme();

  useEffect(() => {
    let live = true;

    void (async () => {
      const { default: mermaid } = await import("mermaid");

      mermaid.initialize({
        startOnLoad: false,
        // The diagrams carry `<b>` and `<br/>` in their labels; `loose` is what lets them through.
        // Every chart on this site is written in this repository, so there is no untrusted input.
        securityLevel: "loose",
        fontFamily: "inherit",
        theme: resolvedTheme === "dark" ? "dark" : "default",
      });

      // The container is passed so mermaid measures text in the font it will actually be shown in.
      // It appends a temporary node, reads the metrics and removes it before returning.
      const rendered = await mermaid.render(`mermaid-${id}`, chart, container.current ?? undefined);
      if (live) setSvg(rendered.svg);
    })();

    return () => {
      live = false;
    };
  }, [chart, id, resolvedTheme]);

  return (
    <div
      ref={container}
      className="my-6 overflow-x-auto [&_svg]:mx-auto [&_svg]:h-auto [&_svg]:max-w-full"
      // The SVG is mermaid's output over a chart authored in this repository.
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  );
}
