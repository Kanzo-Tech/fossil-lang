import defaultMdxComponents from "fumadocs-ui/mdx";
import { Accordion, Accordions } from "fumadocs-ui/components/accordion";
import { Callout } from "fumadocs-ui/components/callout";
import { Card, Cards } from "fumadocs-ui/components/card";
import { Step, Steps } from "fumadocs-ui/components/steps";
import type { MDXComponents } from "mdx/types";
import { Mermaid } from "@/components/mermaid";
import { Program } from "@/components/program";
import { GuardIndex } from "@/components/guard-index";
import { VectorTable } from "@/components/vector-table";

export function getMDXComponents(components?: MDXComponents): MDXComponents {
  return {
    ...defaultMdxComponents,
    Accordion,
    Accordions,
    Callout,
    Card,
    Cards,
    Step,
    Steps,
    Mermaid,
    Program,
    GuardIndex,
    VectorTable,
    ...components,
  };
}
