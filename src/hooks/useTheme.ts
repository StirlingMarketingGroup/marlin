import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { Theme, ViewPreferences } from '@/types';
import { PREFERENCES_UPDATED_EVENT } from '@/utils/events';

const THEME_MEDIA_QUERY = '(prefers-color-scheme: dark)';

const isTheme = (value: unknown): value is Theme =>
  value === 'system' || value === 'dark' || value === 'light';

const getSystemTheme = (): Exclude<Theme, 'system'> => {
  if (typeof window === 'undefined' || !window.matchMedia) {
    return 'dark';
  }
  return window.matchMedia(THEME_MEDIA_QUERY).matches ? 'dark' : 'light';
};

const resolveTheme = (preference: Theme): Exclude<Theme, 'system'> =>
  preference === 'system' ? getSystemTheme() : preference;

const applyTheme = (theme: Exclude<Theme, 'system'>) => {
  if (typeof document === 'undefined') return;
  document.documentElement.dataset.theme = theme;
  document.documentElement.style.colorScheme = theme;
};

export const useThemeSync = (preference: Theme) => {
  useEffect(() => {
    if (typeof window === 'undefined') return;
    const media = window.matchMedia(THEME_MEDIA_QUERY);

    const apply = () => {
      applyTheme(resolveTheme(preference));
    };

    apply();

    if (preference !== 'system') {
      return;
    }

    media.addEventListener('change', apply);
    return () => media.removeEventListener('change', apply);
  }, [preference]);
};

export const useThemePreference = (initialPreference: Theme = 'system'): Theme => {
  const [preference, setPreference] = useState<Theme>(initialPreference);

  useEffect(() => {
    let isActive = true;

    const loadPreferences = async () => {
      try {
        const raw = await invoke<string>('read_preferences');
        if (!raw) return;
        const parsed = JSON.parse(raw || '{}') as { globalPreferences?: Partial<ViewPreferences> };
        const theme = parsed.globalPreferences?.theme;
        if (isActive && isTheme(theme)) {
          setPreference(theme);
        }
      } catch (error) {
        console.warn('Failed to load theme preference:', error);
      }
    };

    void loadPreferences();
    return () => {
      isActive = false;
    };
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;

    (async () => {
      try {
        unlisten = await listen<Partial<ViewPreferences>>(PREFERENCES_UPDATED_EVENT, (evt) => {
          const theme = evt.payload?.theme;
          if (isTheme(theme)) {
            setPreference(theme);
          }
        });
      } catch (error) {
        console.warn('Failed to listen for theme preference updates:', error);
      }
    })();

    return () => {
      if (unlisten) {
        unlisten();
      }
    };
  }, []);

  return preference;
};
