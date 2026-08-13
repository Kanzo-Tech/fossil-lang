import type { ReactNode } from "react";
import { RootProvider } from "fumadocs-ui/provider/next";
import "./global.css";

export const metadata = {
  title: { default: "fossil corpus", template: "%s · fossil corpus" },
  description:
    "A fossil corpus is a graph on disk that outlives the process that wrote it. This site is its format: the conventions that define it, the arithmetic that addresses it, and the guards that make both a contract.",
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <body className="flex min-h-screen flex-col">
        <RootProvider>{children}</RootProvider>
      </body>
    </html>
  );
}
