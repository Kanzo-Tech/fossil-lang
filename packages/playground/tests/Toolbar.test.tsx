/**
 * Toolbar.test.tsx — covers the Phase 14 plan 14-04 Toolbar extraction.
 * The component composes @fossil-lang/ui Tooltip primitives + the existing
 * BibtexModal trigger. The Run / Reset buttons are surfaced via test ids so
 * downstream playground tests + Playwright specs can target them stably.
 */
import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import '@testing-library/jest-dom/vitest';
import { Toolbar } from '../src/component/Toolbar.js';

describe('Toolbar (14-04)', () => {
  it('renders Run + Reset buttons + banner landmark', () => {
    render(
      <Toolbar
        onRun={vi.fn()}
        onReset={vi.fn()}
        running={false}
        permalink={undefined}
      />,
    );
    expect(screen.getByTestId('toolbar-run')).toBeInTheDocument();
    expect(screen.getByTestId('toolbar-reset')).toBeInTheDocument();
    expect(screen.getByRole('banner')).toBeInTheDocument();
  });

  it('calls onRun when Run clicked', () => {
    const onRun = vi.fn();
    render(
      <Toolbar
        onRun={onRun}
        onReset={vi.fn()}
        running={false}
        permalink={undefined}
      />,
    );
    fireEvent.click(screen.getByTestId('toolbar-run'));
    expect(onRun).toHaveBeenCalledOnce();
  });

  it('disables Run + flips visible text when running=true (accessible name stable)', () => {
    render(
      <Toolbar
        onRun={vi.fn()}
        onReset={vi.fn()}
        running={true}
        permalink={undefined}
      />,
    );
    const btn = screen.getByTestId('toolbar-run');
    expect(btn).toBeDisabled();
    expect(btn).toHaveTextContent(/running/i);
    expect(btn).toHaveAttribute('aria-busy', 'true');
  });

  it('calls onReset when Reset clicked', () => {
    const onReset = vi.fn();
    render(
      <Toolbar
        onRun={vi.fn()}
        onReset={onReset}
        running={false}
        permalink={undefined}
      />,
    );
    fireEvent.click(screen.getByTestId('toolbar-reset'));
    expect(onReset).toHaveBeenCalledOnce();
  });
});
