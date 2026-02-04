import { useEffect, useMemo, useState } from 'react';
import type { ThemeDefinition } from '@/types';
import { BUILT_IN_THEMES } from '@/themes';
import { loadThemesFromConfigDir, mergeThemes } from '@/utils/theme';

export const useThemeRegistry = (customThemes?: ThemeDefinition[]) => {
  const [themes, setThemes] = useState<ThemeDefinition[]>(() => BUILT_IN_THEMES);
  const custom = useMemo(() => customThemes ?? [], [customThemes]);

  useEffect(() => {
    let isActive = true;
    (async () => {
      const fileThemes = await loadThemesFromConfigDir();
      const merged = mergeThemes([...BUILT_IN_THEMES, ...fileThemes, ...custom]);
      if (isActive) {
        setThemes(merged);
      }
    })();
    return () => {
      isActive = false;
    };
  }, [custom]);

  return themes;
};
