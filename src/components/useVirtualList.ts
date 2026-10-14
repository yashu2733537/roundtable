import { useCallback, useEffect, useRef, useState } from "react";
import type { RefObject } from "react";

interface VirtualWindow {
  /** First item index to render (inclusive). */
  start: number;
  /** Last item index to render (exclusive). */
  end: number;
  /** True when windowing is active (list exceeded the threshold). */
  virtualized: boolean;
}

interface Options {
  estimatedHeight?: number;
  overscan?: number;
  /** Below this count the full list renders (no windowing). */
  threshold?: number;
}

/**
 * Hand-rolled windowing for long message lists. No dependencies.
 *
 * The scroll container keeps its natural scrollHeight via top/bottom spacer
 * divs rendered by the caller. The estimate is deliberately coarse — variable
 * message heights mean the spacers are approximate — so callers use a
 * generous overscan and keep sticky-to-bottom logic outside this hook.
 */
export function useVirtualWindow(
  count: number,
  scrollRef: RefObject<HTMLDivElement | null>,
  opts?: Options,
): VirtualWindow & { onScrollMeas: () => void } {
  const estimatedHeight = opts?.estimatedHeight ?? 140;
  const overscan = opts?.overscan ?? 12;
  const threshold = opts?.threshold ?? 80;

  const [viewport, setViewport] = useState({ top: 0, height: 600 });
  const rafRef = useRef(0);

  const measure = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    setViewport({ top: el.scrollTop, height: el.clientHeight || 600 });
  }, [scrollRef]);

  const onScrollMeas = useCallback(() => {
    if (rafRef.current) return;
    rafRef.current = requestAnimationFrame(() => {
      rafRef.current = 0;
      measure();
    });
  }, [measure]);

  useEffect(() => {
    measure();
    const el = scrollRef.current;
    if (!el) return;
    const ro =
      typeof ResizeObserver !== "undefined"
        ? new ResizeObserver(() => measure())
        : null;
    if (ro) ro.observe(el);
    window.addEventListener("resize", measure);
    return () => {
      window.removeEventListener("resize", measure);
      if (ro) ro.disconnect();
      if (rafRef.current) cancelAnimationFrame(rafRef.current);
    };
  }, [measure, scrollRef]);

  // Re-measure when the item count changes (append while stuck to bottom).
  useEffect(() => {
    measure();
  }, [count, measure]);

  if (count <= threshold) {
    return { start: 0, end: count, virtualized: false, onScrollMeas };
  }

  const visibleCount = Math.max(
    1,
    Math.ceil(viewport.height / estimatedHeight),
  );
  const firstVisible = Math.max(
    0,
    Math.floor(viewport.top / estimatedHeight),
  );
  const start = Math.max(0, firstVisible - overscan);
  const end = Math.min(count, firstVisible + visibleCount + overscan);
  return { start, end, virtualized: true, onScrollMeas };
}

/** Spacer heights for the slice [start, end) of `count` items. */
export function spacerHeights(
  count: number,
  start: number,
  end: number,
  estimatedHeight: number,
): { top: number; bottom: number } {
  return {
    top: Math.max(0, start) * estimatedHeight,
    bottom: Math.max(0, count - Math.min(count, end)) * estimatedHeight,
  };
}
