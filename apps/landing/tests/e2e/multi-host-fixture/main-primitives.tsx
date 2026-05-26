/**
 * VIS-01 oracle — mounts ALL 8 @fossil-lang/ui primitives in a vanilla
 * Vite + React host. NO Tailwind, NO shadcn, NO Next.js. This is the
 * SC#1 proof of the IDE look reaching consumers via the primitive
 * package self-sufficiency (NOT via the @fossil-lang/playground bundle).
 *
 * Primitives mounted (from @fossil-lang/ui — 10-03/04/05 surface):
 *   - Tabs (variant=default + variant=line)
 *   - Dialog (open via DialogTrigger)
 *   - DropdownMenu (open via DropdownMenuTrigger)
 *   - Tooltip (provider + hover-driven content)
 *   - ScrollArea (overflowed content)
 *   - Resizable (2-panel horizontal split + handle)
 *   - Toggle + ToggleGroup (single + group with multiple selection)
 *   - Separator (horizontal between sections)
 *
 * Tokens applied via `themeToCssVars` + `cssVarsToStyle` from
 * @fossil-lang/playground. The IDE-grade theme is constructed inline as
 * `lightTheme` with `fonts.sizeBase=13px` + `fonts.sizeSmall=11px`
 * (matches packages/playground/src/theme/fossil-ide.ts shape). This
 * keeps the fixture decoupled from parallel plan 10-06's export of
 * `fossilIdeTheme` — when 10-06 ships and exports it from
 * @fossil-lang/playground, this fixture continues to work identically
 * because we replicate the same token shape.
 */
import { createRoot } from 'react-dom/client';
import {
  // Tabs
  Tabs,
  TabsList,
  TabsTrigger,
  TabsContent,
  // Dialog
  Dialog,
  DialogTrigger,
  DialogPortal,
  DialogOverlay,
  DialogContent,
  DialogTitle,
  DialogDescription,
  DialogClose,
  // DropdownMenu
  DropdownMenu,
  DropdownMenuTrigger,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  // Tooltip
  TooltipProvider,
  Tooltip,
  TooltipTrigger,
  TooltipContent,
  // ScrollArea
  ScrollArea,
  // Resizable
  ResizablePanelGroup,
  ResizablePanel,
  ResizableHandle,
  // Toggle + ToggleGroup
  Toggle,
  ToggleGroup,
  ToggleGroupItem,
  // Separator
  Separator,
} from '@fossil-lang/ui';
import {
  lightTheme,
  themeToCssVars,
  cssVarsToStyle,
} from '@fossil-lang/playground';

// Inline construction of the IDE-grade theme — same shape as
// packages/playground/src/theme/fossil-ide.ts. Decouples the fixture from
// parallel plan 10-06's playground re-export ordering. The ONLY divergence
// from lightTheme is fonts.sizeBase=13px + fonts.sizeSmall=11px.
const fossilIdeTheme = {
  ...lightTheme,
  fonts: {
    ...lightTheme.fonts,
    sizeBase: '13px',
    sizeSmall: '11px',
  },
};

function PrimitivesPage() {
  const cssVars = themeToCssVars(fossilIdeTheme);
  return (
    <div style={cssVarsToStyle(cssVars)} data-testid="primitives-root">
      <section data-testid="section-tabs-default">
        <h2>Tabs (variant=default)</h2>
        <Tabs defaultValue="a">
          <TabsList>
            <TabsTrigger value="a">Mapping</TabsTrigger>
            <TabsTrigger value="b">Source</TabsTrigger>
            <TabsTrigger value="c" disabled>
              Disabled
            </TabsTrigger>
          </TabsList>
          <TabsContent value="a">Mapping panel</TabsContent>
          <TabsContent value="b">Source panel</TabsContent>
        </Tabs>
      </section>

      <section data-testid="section-tabs-line">
        <h2>Tabs (variant=line)</h2>
        <Tabs defaultValue="a">
          <TabsList variant="line">
            <TabsTrigger value="a">Graph</TabsTrigger>
            <TabsTrigger value="b">Turtle</TabsTrigger>
            <TabsTrigger value="c">Vertices</TabsTrigger>
          </TabsList>
          <TabsContent value="a">Graph panel</TabsContent>
          <TabsContent value="b">Turtle panel</TabsContent>
          <TabsContent value="c">Vertices panel</TabsContent>
        </Tabs>
      </section>

      <Separator data-testid="separator-h" />

      <section data-testid="section-overlays" className="grid">
        <div data-testid="section-dialog">
          <h2>Dialog</h2>
          <Dialog>
            <DialogTrigger asChild>
              <button data-testid="dialog-open">Open dialog</button>
            </DialogTrigger>
            <DialogPortal>
              <DialogOverlay />
              <DialogContent>
                <DialogTitle>BibTeX</DialogTitle>
                <DialogDescription>
                  Reference dialog for VIS-01.
                </DialogDescription>
                <DialogClose asChild>
                  <button data-testid="dialog-close">Close</button>
                </DialogClose>
              </DialogContent>
            </DialogPortal>
          </Dialog>
        </div>

        <div data-testid="section-dropdown">
          <h2>DropdownMenu</h2>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <button data-testid="dropdown-open">Examples</button>
            </DropdownMenuTrigger>
            <DropdownMenuContent>
              <DropdownMenuItem>hello.fossil</DropdownMenuItem>
              <DropdownMenuItem>airports.fossil</DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem disabled>Disabled item</DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>

        <div data-testid="section-tooltip">
          <h2>Tooltip</h2>
          <TooltipProvider>
            <Tooltip>
              <TooltipTrigger asChild>
                <button data-testid="tooltip-trigger">Hover me</button>
              </TooltipTrigger>
              <TooltipContent>Compile and run</TooltipContent>
            </Tooltip>
          </TooltipProvider>
        </div>

        <div data-testid="section-toggle">
          <h2>Toggle / ToggleGroup</h2>
          <Toggle data-testid="toggle-single" aria-label="Bold">
            Bold
          </Toggle>
          <ToggleGroup
            type="multiple"
            data-testid="toggle-group"
            aria-label="Entity types"
          >
            <ToggleGroupItem value="users" aria-label="Users">
              Users
            </ToggleGroupItem>
            <ToggleGroupItem value="orders" aria-label="Orders">
              Orders
            </ToggleGroupItem>
          </ToggleGroup>
        </div>
      </section>

      <section data-testid="section-scroll">
        <h2>ScrollArea</h2>
        <ScrollArea
          type="always"
          style={{
            height: 120,
            width: 240,
            border: '1px solid var(--fossil-colors-border)',
          }}
        >
          <div style={{ padding: 'var(--fossil-spacing-2)' }}>
            {Array.from({ length: 30 }).map((_, i) => (
              <div key={i}>Row {i + 1}</div>
            ))}
          </div>
        </ScrollArea>
      </section>

      <section data-testid="section-resizable">
        <h2>Resizable</h2>
        <ResizablePanelGroup
          direction="horizontal"
          style={{
            height: 120,
            border: '1px solid var(--fossil-colors-border)',
          }}
        >
          <ResizablePanel defaultSize={60}>Panel A</ResizablePanel>
          <ResizableHandle withHandle />
          <ResizablePanel defaultSize={40}>Panel B</ResizablePanel>
        </ResizablePanelGroup>
      </section>
    </div>
  );
}

const rootEl = document.getElementById('root');
if (!rootEl) {
  throw new Error('Multi-host fixture: #root not found in primitives.html');
}
createRoot(rootEl).render(<PrimitivesPage />);
