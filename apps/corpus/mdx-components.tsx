import defaultMdxComponents from "fumadocs-ui/mdx";
import { Callout } from "fumadocs-ui/components/callout";
import { Card, Cards } from "fumadocs-ui/components/card";
import { Step, Steps } from "fumadocs-ui/components/steps";
import type { MDXComponents } from "mdx/types";
import { GuardIndex } from "@/components/guard-index";
import { Mermaid } from "@/components/mermaid";
import { VectorTable } from "@/components/vector-table";

export function getMDXComponents(components?: MDXComponents): MDXComponents {
  return {
    ...defaultMdxComponents,
    Callout,
    Card,
    Cards,
    Step,
    Steps,
    GuardIndex,
    Mermaid,
    VectorTable,
    ...components,
  };
}
