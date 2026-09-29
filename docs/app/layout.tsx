import type { ReactNode } from "react";
import { RootProvider } from "fumadocs-ui/provider/next";
import { basePath } from "@/lib/base-path";
import "./global.css";

export const metadata = {
  title: { default: "fossil", template: "%s · fossil" },
  description:
    "A mapping language and a corpus compiler. This site documents where fossil is going, and marks what is already true.",
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html lang="en" suppressHydrationWarning>
      <body className="flex min-h-screen flex-col">
        {/* `type: "static"` is the client half of `app/api/search/route.ts`'s `staticGET`. `api`
            carries the prefix because the client `fetch`es it, and Next prefixes no `fetch`. */}
        <RootProvider search={{ options: { type: "static", api: `${basePath}/api/search` } }}>
          {children}
        </RootProvider>
      </body>
    </html>
  );
}
