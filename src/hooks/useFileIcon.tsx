import { useCallback } from 'react';
import {
  Folder,
  ImageSquare,
  MusicNote,
  VideoCamera,
  FileText,
  AppWindow,
  Package,
  FilePdf,
  PaintBrush,
  Palette,
  Disc,
  Cube,
} from 'phosphor-react';
import { FileItem } from '../types';
import AppIcon from '@/components/AppIcon';
import { FileTypeIcon, resolveVSCodeIcon } from '@/components/FileTypeIcon';
import FileExtensionBadge from '@/components/FileExtensionBadge';
import {
  getEffectiveExtension,
  isArchiveExtension,
  isVideoExtension,
  isMacOSBundle,
  isAppBundle,
} from '@/utils/fileTypes';

export type IconSize = 'small' | 'large';

interface SizeConfig {
  iconClass: string;
  appIconClass: string;
  appIconSize: number;
  fileTypeSize: 'small' | 'large';
}

const SIZE_CONFIG: Record<IconSize, SizeConfig> = {
  small: {
    iconClass: 'w-5 h-5',
    appIconClass: 'w-5 h-5',
    appIconSize: 64,
    fileTypeSize: 'small',
  },
  large: {
    iconClass: 'w-12 h-12',
    appIconClass: 'w-16 h-16',
    appIconSize: 64,
    fileTypeSize: 'large',
  },
};

export function useFileIcon(size: IconSize, isMac: boolean) {
  const config = SIZE_CONFIG[size];

  const getFileIcon = useCallback(
    (file: FileItem) => {
      // macOS-specific file types
      if (isMac) {
        const fileName = file.name.toLowerCase();
        if (file.is_directory && fileName.endsWith('.app')) {
          return (
            <AppIcon
              path={file.path}
              size={config.appIconSize}
              className={config.appIconClass}
              rounded={size === 'large'}
              priority="high"
              fallback={<AppWindow className={`${config.iconClass} text-accent`} />}
            />
          );
        }

        if (fileName.endsWith('.pkg')) {
          return <Package className={`${config.iconClass} text-blue-500`} weight="fill" />;
        }

        if (fileName.endsWith('.dmg')) {
          return <Disc className={`${config.iconClass} text-app-muted`} weight="fill" />;
        }

        if (isMacOSBundle(file) && !isAppBundle(file)) {
          return <Package className={`${config.iconClass} text-purple-400`} weight="fill" />;
        }
      }

      // Directories
      if (file.is_directory) {
        return <Folder className={`${config.iconClass} text-accent`} weight="fill" />;
      }

      const effectiveExtension = getEffectiveExtension(file);
      const ext = effectiveExtension?.toLowerCase();

      if (!ext) {
        const special = resolveVSCodeIcon(file.name);
        if (special) return <FileTypeIcon name={file.name} size={config.fileTypeSize} />;
        return <FileExtensionBadge extension={effectiveExtension} size={config.fileTypeSize} />;
      }

      // Image files
      if (['jpg', 'jpeg', 'png', 'gif', 'webp', 'svg', 'bmp'].includes(ext)) {
        return <ImageSquare className={`${config.iconClass} text-app-green`} />;
      }

      // PDF files
      if (ext === 'pdf') {
        return <FilePdf className={`${config.iconClass} text-red-500`} />;
      }

      // Adobe Illustrator files
      if (ext === 'ai' || ext === 'eps') {
        return <PaintBrush className={`${config.iconClass} text-orange-500`} />;
      }

      // Photoshop files
      if (ext === 'psd' || ext === 'psb') {
        return <Palette className={`${config.iconClass} text-blue-500`} />;
      }

      // Audio files
      if (['mp3', 'wav', 'flac', 'aac', 'm4a', 'ogg'].includes(ext)) {
        return <MusicNote className={`${config.iconClass} text-app-yellow`} />;
      }

      // Video files
      if (isVideoExtension(ext)) {
        return <VideoCamera className={`${config.iconClass} text-app-red`} />;
      }

      // Archive files
      if (isArchiveExtension(ext)) {
        return <FileTypeIcon name={file.name} ext={ext} size={config.fileTypeSize} />;
      }

      // 3D model: STL
      if (ext === 'stl') {
        return <Cube className={`${config.iconClass} text-app-green`} />;
      }

      // VSCode-style file icons for code/config types
      if (resolveVSCodeIcon(file.name, ext)) {
        return <FileTypeIcon name={file.name} ext={ext} size={config.fileTypeSize} />;
      }

      // Text files
      if (['txt', 'md', 'json', 'xml', 'yml', 'yaml', 'toml'].includes(ext)) {
        return <FileText className={`${config.iconClass} text-app-text`} />;
      }

      return <FileExtensionBadge extension={effectiveExtension} size={config.fileTypeSize} />;
    },
    [isMac, config, size]
  );

  return getFileIcon;
}
