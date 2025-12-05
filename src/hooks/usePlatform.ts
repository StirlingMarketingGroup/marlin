import { useMemo } from 'react';

interface PlatformInfo {
  isMac: boolean;
  isWindows: boolean;
  isLinux: boolean;
}

function detectPlatform(): PlatformInfo {
  if (typeof navigator === 'undefined') {
    return { isMac: false, isWindows: false, isLinux: false };
  }

  const ua = navigator.userAgent.toLowerCase();
  const platform = navigator.platform?.toLowerCase() ?? '';

  // Use both userAgent and platform for reliability
  const isMac = ua.includes('mac') || platform.includes('mac');
  const isWindows = ua.includes('win') || platform.includes('win');
  const isLinux = ua.includes('linux') || platform.includes('linux');

  return { isMac, isWindows, isLinux };
}

// Cached result for non-hook contexts
let cachedPlatform: PlatformInfo | null = null;

export function getPlatform(): PlatformInfo {
  if (!cachedPlatform) {
    cachedPlatform = detectPlatform();
  }
  return cachedPlatform;
}

export function usePlatform(): PlatformInfo {
  return useMemo(() => getPlatform(), []);
}
