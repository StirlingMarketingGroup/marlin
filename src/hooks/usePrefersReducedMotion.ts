import { useState, useEffect } from 'react';

const getMatch = () =>
  typeof window !== 'undefined' &&
  typeof window.matchMedia === 'function' &&
  window.matchMedia('(prefers-reduced-motion: reduce)');

/**
 * Returns true if the user prefers reduced motion.
 * Reactively updates if the system setting changes.
 */
export function usePrefersReducedMotion(): boolean {
  const [prefersReducedMotion, setPrefersReducedMotion] = useState(() => {
    const match = getMatch();
    return match ? match.matches : false;
  });

  useEffect(() => {
    const mediaQuery = getMatch();
    if (!mediaQuery) return;

    const listener = (event: MediaQueryListEvent) => {
      setPrefersReducedMotion(event.matches);
    };

    mediaQuery.addEventListener('change', listener);
    return () => mediaQuery.removeEventListener('change', listener);
  }, []);

  return prefersReducedMotion;
}
