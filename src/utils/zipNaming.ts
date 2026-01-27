import type { FileItem } from '@/types';
import { basename } from '@/utils/pathUtils';

const stripExtension = (name: string) => {
  const lastDot = name.lastIndexOf('.');
  if (lastDot <= 0) return name;
  return name.slice(0, lastDot);
};

interface ZipNameParams {
  selectedItems: FileItem[];
  visibleItems: FileItem[];
  currentPath: string;
}

export const getSuggestedZipName = ({
  selectedItems,
  visibleItems,
  currentPath,
}: ZipNameParams): string => {
  if (!selectedItems.length) return 'Archive.zip';

  if (selectedItems.length === 1 && selectedItems[0].is_directory) {
    return `${selectedItems[0].name}.zip`;
  }

  const visibleCount = visibleItems.length;
  if (visibleCount > 0) {
    const selectedRatio = selectedItems.length / visibleCount;
    if (selectedRatio >= 0.9) {
      const dirName = basename(currentPath);
      const safeDirName = dirName && dirName !== '/' ? dirName : 'Archive';
      return `${safeDirName}.zip`;
    }
  }

  const first = selectedItems[0];
  const baseName = first.is_directory ? first.name : stripExtension(first.name);
  return `${baseName}.zip`;
};
