import { useEffect, useRef, useCallback } from 'react';
import { useAppStore } from '../store/useAppStore';
import { usePrefersReducedMotion } from './usePrefersReducedMotion';
import type { FileItem } from '../types';
import React from 'react';

// Batch threshold - skip animations for operations affecting more than this many files
const ANIMATION_BATCH_THRESHOLD = 10;

interface UseFileAnimationsOptions {
  filteredFiles: FileItem[];
}

interface UseFileAnimationsResult {
  /** Whether a file is in entering state */
  isEntering: (path: string) => boolean;
  /** Whether a file is in exiting state */
  isExiting: (path: string) => boolean;
  /** Handler for transitionend events on file items */
  handleTransitionEnd: (e: React.TransitionEvent, filePath: string) => void;
}

/**
 * Custom hook that encapsulates file enter/exit animation logic.
 * Detects new files, handles animations, and provides the transitionend callback.
 */
export function useFileAnimations({
  filteredFiles,
}: UseFileAnimationsOptions): UseFileAnimationsResult {
  const {
    animationState,
    markFilesEntering,
    clearEnteringState,
    removeExitedFiles,
    consumeSkipAnimationPaths,
  } = useAppStore();

  const prefersReducedMotion = usePrefersReducedMotion();

  // Animation refs
  const prevFilesRef = useRef<Set<string>>(new Set());
  const isInitialLoadRef = useRef(true);

  // Detect newly added files and mark them as entering
  useEffect(() => {
    const currentPaths = new Set(filteredFiles.map((f) => f.path));

    // Skip animations if user prefers reduced motion or on initial load
    if (prefersReducedMotion || isInitialLoadRef.current) {
      prevFilesRef.current = currentPaths;
      if (isInitialLoadRef.current) {
        isInitialLoadRef.current = false;
      }
      consumeSkipAnimationPaths(); // Clear any pending skip paths
      return;
    }

    const prevPaths = prevFilesRef.current;

    // Get paths that should skip animation (e.g., renamed files)
    const skipPaths = consumeSkipAnimationPaths();

    // Find new files (in current but not in previous), excluding skip paths
    const newPaths: string[] = [];
    for (const path of currentPaths) {
      if (!prevPaths.has(path) && !skipPaths.has(path)) {
        newPaths.push(path);
      }
    }

    // Skip animations for batch operations (>10 files)
    if (newPaths.length > 0 && newPaths.length <= ANIMATION_BATCH_THRESHOLD) {
      markFilesEntering(newPaths);

      // Remove entering state on the next-next frame to trigger the transition.
      // The first rAF ensures the component has rendered with `data-entering="true"`.
      // The second rAF runs after the browser has painted, and removing the attribute
      // at this point will trigger the CSS transition from the entering state.
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          clearEnteringState(newPaths);
        });
      });
    }

    // Update ref for next comparison
    prevFilesRef.current = currentPaths;
  }, [
    filteredFiles,
    prefersReducedMotion,
    markFilesEntering,
    clearEnteringState,
    consumeSkipAnimationPaths,
  ]);

  // Handle transitionend for exiting files
  const handleTransitionEnd = useCallback(
    (e: React.TransitionEvent, filePath: string) => {
      // Only handle opacity transitions to avoid double-firing
      if (e.propertyName !== 'opacity') return;
      // Ignore bubbled events from children
      if (e.target !== e.currentTarget) return;
      // Read fresh state to avoid stale closure issues
      if (useAppStore.getState().animationState.exiting[filePath]) {
        removeExitedFiles([filePath]);
      }
    },
    [removeExitedFiles]
  );

  const isEntering = useCallback(
    (path: string) => Boolean(animationState.entering[path]),
    [animationState.entering]
  );

  const isExiting = useCallback(
    (path: string) => Boolean(animationState.exiting[path]),
    [animationState.exiting]
  );

  return {
    isEntering,
    isExiting,
    handleTransitionEnd,
  };
}
