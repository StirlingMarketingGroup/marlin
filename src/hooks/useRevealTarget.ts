import { useEffect, type RefObject } from 'react';
import type { Virtualizer } from '@tanstack/react-virtual';
import type { FileItem } from '@/types';
import { useAppStore } from '@/store/useAppStore';

export function useRevealTarget(
  filteredFiles: FileItem[],
  containerRef: RefObject<HTMLElement | null>,
  virtualizer: Virtualizer<HTMLDivElement, Element>,
  itemsPerRow: number
): void {
  const pendingRevealTarget = useAppStore((state) => state.pendingRevealTarget);
  const isMetadataComplete = useAppStore((state) => state.isMetadataComplete);
  const setSelectedFiles = useAppStore((state) => state.setSelectedFiles);
  const setSelectionAnchor = useAppStore((state) => state.setSelectionAnchor);
  const setSelectionLead = useAppStore((state) => state.setSelectionLead);
  const setPendingRevealTarget = useAppStore((state) => state.setPendingRevealTarget);

  useEffect(() => {
    if (!pendingRevealTarget) return;
    // Sizes and dates arrive last and can reorder the list, so the index is right only then.
    if (!isMetadataComplete) return;

    // On Windows, currentPath uses `/` and a listed path uses `\`.
    const target = pendingRevealTarget.replace(/\\/g, '/');
    const index = filteredFiles.findIndex((file) => file.path.replace(/\\/g, '/') === target);
    if (index < 0) return;

    const file = filteredFiles[index];
    setSelectedFiles([file.path]);
    setSelectionAnchor(file.path);
    setSelectionLead(file.path);

    requestAnimationFrame(() => {
      const container = containerRef.current;
      if (container) {
        const nodes = container.querySelectorAll<HTMLElement>('[data-file-item="true"]');
        for (const el of nodes) {
          if (el.getAttribute('data-file-path') === file.path) {
            el.scrollIntoView({ block: 'nearest', inline: 'nearest' });
            return;
          }
        }
      }
      // The row is not rendered, so only the virtualizer can reach it.
      virtualizer.scrollToIndex(Math.floor(index / itemsPerRow), { align: 'center' });
    });

    setPendingRevealTarget(undefined);
  }, [
    pendingRevealTarget,
    isMetadataComplete,
    filteredFiles,
    containerRef,
    virtualizer,
    itemsPerRow,
    setSelectedFiles,
    setSelectionAnchor,
    setSelectionLead,
    setPendingRevealTarget,
  ]);
}
