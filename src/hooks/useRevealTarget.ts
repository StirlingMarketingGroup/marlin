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
  const files = useAppStore((state) => state.files);
  const pendingRevealTargets = useAppStore((state) => state.pendingRevealTargets);
  const isMetadataComplete = useAppStore((state) => state.isMetadataComplete);
  const setSelectedFiles = useAppStore((state) => state.setSelectedFiles);
  const setSelectionAnchor = useAppStore((state) => state.setSelectionAnchor);
  const setSelectionLead = useAppStore((state) => state.setSelectionLead);
  const setPendingRevealTargets = useAppStore((state) => state.setPendingRevealTargets);

  useEffect(() => {
    if (!pendingRevealTargets) return;
    // Sizes and dates arrive last and can reorder the list, so the order is right only then.
    if (!isMetadataComplete) return;

    // On Windows, currentPath uses `/` and a listed path uses `\`.
    const targets = new Set(pendingRevealTargets.map((path) => path.replace(/\\/g, '/')));
    // The folder watcher can list a drop's items over two reads. Wait for all of them, or only the
    // first would be selected.
    const listed = files.filter((file) => targets.has(file.path.replace(/\\/g, '/')));
    if (listed.length < targets.size) return;

    // The view leaves out hidden files and what the filter does not match. While it shows none of
    // the targets they stay, so turning hidden files on still selects them.
    const shown = filteredFiles.filter((file) => targets.has(file.path.replace(/\\/g, '/')));
    if (shown.length === 0) return;

    const first = shown[0];
    const index = filteredFiles.indexOf(first);
    setSelectedFiles(shown.map((file) => file.path));
    // As select-all does: the anchor on the first, the lead on the last.
    setSelectionAnchor(first.path);
    setSelectionLead(shown[shown.length - 1].path);

    requestAnimationFrame(() => {
      const container = containerRef.current;
      if (container) {
        const nodes = container.querySelectorAll<HTMLElement>('[data-file-item="true"]');
        for (const el of nodes) {
          if (el.getAttribute('data-file-path') === first.path) {
            el.scrollIntoView({ block: 'nearest', inline: 'nearest' });
            return;
          }
        }
      }
      // The row is not rendered, so only the virtualizer can reach it.
      virtualizer.scrollToIndex(Math.floor(index / itemsPerRow), { align: 'center' });
    });

    setPendingRevealTargets(undefined);
  }, [
    pendingRevealTargets,
    isMetadataComplete,
    filteredFiles,
    files,
    containerRef,
    virtualizer,
    itemsPerRow,
    setSelectedFiles,
    setSelectionAnchor,
    setSelectionLead,
    setPendingRevealTargets,
  ]);
}
