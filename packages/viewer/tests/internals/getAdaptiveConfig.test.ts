/**
 * Unit tests for getAdaptiveConfig — Phase 12 plan 12-03.
 *
 * Pins the production-tuned formula at its boundary values so future
 * contributors who tweak the lerp can't silently drift away from
 * Keasy's production behaviour (the formula was tuned across 10 to
 * 100,000 node graphs).
 */
import { describe, it, expect } from 'vitest';
import {
  getAdaptiveConfig,
  DEFAULT_GRAPH_CONFIG,
} from '../../src/internals/getAdaptiveConfig.js';

describe('getAdaptiveConfig', () => {
  it('returns spaceSize=2048 at the low endpoint (nodeCount=10)', () => {
    const config = getAdaptiveConfig(10);
    expect(config.spaceSize).toBe(2048);
  });

  it('returns spaceSize=8192 at the high endpoint (nodeCount=100_000)', () => {
    const config = getAdaptiveConfig(100_000);
    expect(config.spaceSize).toBe(8192);
  });

  it('enables renderLinks for nodeCount < 250_000 and disables above', () => {
    expect(getAdaptiveConfig(1000).renderLinks).toBe(true);
    expect(getAdaptiveConfig(249_999).renderLinks).toBe(true);
    expect(getAdaptiveConfig(250_000).renderLinks).toBe(false);
    expect(getAdaptiveConfig(500_000).renderLinks).toBe(false);
  });

  it('disables scalePointsOnZoom + renderHoveredPointRing at nodeCount >= 100_000', () => {
    const small = getAdaptiveConfig(10_000);
    expect(small.scalePointsOnZoom).toBe(true);
    expect(small.renderHoveredPointRing).toBe(true);

    const large = getAdaptiveConfig(100_000);
    expect(large.scalePointsOnZoom).toBe(false);
    expect(large.renderHoveredPointRing).toBe(false);
  });

  it('adds linkVisibilityDistanceRange + linkVisibilityMinTransparency for nodeCount > 5000 only', () => {
    const small = getAdaptiveConfig(1000) as {
      linkVisibilityDistanceRange?: [number, number];
      linkVisibilityMinTransparency?: number;
    };
    expect(small.linkVisibilityDistanceRange).toBeUndefined();
    expect(small.linkVisibilityMinTransparency).toBeUndefined();

    const large = getAdaptiveConfig(6000) as {
      linkVisibilityDistanceRange?: [number, number];
      linkVisibilityMinTransparency?: number;
    };
    expect(large.linkVisibilityDistanceRange).toEqual([50, 200]);
    expect(large.linkVisibilityMinTransparency).toBe(0.05);
  });

  it('exports DEFAULT_GRAPH_CONFIG as a static snapshot of getAdaptiveConfig(500)', () => {
    const snapshot = getAdaptiveConfig(500);
    expect(DEFAULT_GRAPH_CONFIG.spaceSize).toBe(snapshot.spaceSize);
    expect(DEFAULT_GRAPH_CONFIG.renderLinks).toBe(true);
    expect(DEFAULT_GRAPH_CONFIG.scalePointsOnZoom).toBe(true);
  });
});
