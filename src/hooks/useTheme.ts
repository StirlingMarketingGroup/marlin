import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { Theme, ThemeDefinition, ViewPreferences } from '@/types';
import { PREFERENCES_UPDATED_EVENT } from '@/utils/events';
import { applyThemeDefinition, isThemeDefinition } from '@/utils/theme';
import { DEFAULT_THEME_IDS } from '@/themes';

const THEME_MEDIA_QUERY = '(prefers-color-scheme: dark)';

const isTheme = (value: unknown): value is Theme =>
  value === 'system' || value === 'dark' || value === 'light';

const getSystemTheme = (): Exclude<Theme, 'system'> => {
  if (typeof window === 'undefined' || !window.matchMedia) {
    return 'dark';
  }
  return window.matchMedia(THEME_MEDIA_QUERY).matches ? 'dark' : 'light';
};

const resolveScheme = (preference: Theme): Exclude<Theme, 'system'> =>
  preference === 'system' ? getSystemTheme() : preference;

const resolveThemeDefinition = (
  themes: ThemeDefinition[],
  scheme: Exclude<Theme, 'system'>,
  themeId?: string
): ThemeDefinition | null => {
  const findById = (id?: string) => themes.find((theme) => theme.id === id);
  const fallbackId = scheme === 'dark' ? DEFAULT_THEME_IDS.dark : DEFAULT_THEME_IDS.light;
  const byId = findById(themeId);
  if (byId && byId.colorScheme === scheme) {
    return byId;
  }
  const fallback = findById(fallbackId);
  return fallback || themes.find((theme) => theme.colorScheme === scheme) || themes[0] || null;
};

export interface ThemePreferenceState {
  mode: Theme;
  darkThemeId?: string;
  lightThemeId?: string;
  customThemes?: ThemeDefinition[];
}

export const useThemeSync = (preferences: ThemePreferenceState, themes: ThemeDefinition[]) => {
  useEffect(() => {
    if (typeof window === 'undefined') return;
    const media = window.matchMedia(THEME_MEDIA_QUERY);

    const apply = () => {
      const scheme = resolveScheme(preferences.mode);
      const themeId = scheme === 'dark' ? preferences.darkThemeId : preferences.lightThemeId;
      const theme = resolveThemeDefinition(themes, scheme, themeId);
      if (theme) {
        applyThemeDefinition(theme);
      }
    };

    apply();

    if (preferences.mode !== 'system') {
      return;
    }

    media.addEventListener('change', apply);
    return () => media.removeEventListener('change', apply);
  }, [preferences.mode, preferences.darkThemeId, preferences.lightThemeId, themes]);
};

export const useThemePreference = (initialPreference: Theme = 'system'): ThemePreferenceState => {
  const [preference, setPreference] = useState<ThemePreferenceState>({
    mode: initialPreference,
    darkThemeId: DEFAULT_THEME_IDS.dark,
    lightThemeId: DEFAULT_THEME_IDS.light,
    customThemes: [],
  });

  useEffect(() => {
    let isActive = true;

    const loadPreferences = async () => {
      try {
        const raw = await invoke<string>('read_preferences');
        if (!raw) return;
        const parsed = JSON.parse(raw || '{}') as { globalPreferences?: Partial<ViewPreferences> };
        const prefs = parsed.globalPreferences ?? {};
        const theme = prefs.theme;
        const darkThemeId = typeof prefs.darkThemeId === 'string' ? prefs.darkThemeId : undefined;
        const lightThemeId =
          typeof prefs.lightThemeId === 'string' ? prefs.lightThemeId : undefined;
        const customThemes = Array.isArray(prefs.customThemes)
          ? prefs.customThemes.filter(isThemeDefinition)
          : [];
        if (isActive) {
          setPreference({
            mode: isTheme(theme) ? theme : initialPreference,
            darkThemeId: darkThemeId ?? DEFAULT_THEME_IDS.dark,
            lightThemeId: lightThemeId ?? DEFAULT_THEME_IDS.light,
            customThemes,
          });
        }
      } catch (error) {
        console.warn('Failed to load theme preference:', error);
      }
    };

    void loadPreferences();
    return () => {
      isActive = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- only load on mount
  }, []);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let isActive = true;

    (async () => {
      try {
        const unlistenFn = await listen<Partial<ViewPreferences>>(
          PREFERENCES_UPDATED_EVENT,
          (evt) => {
            if (!isActive) return;
            const payload = evt.payload;
            setPreference((prev) => ({
              mode: isTheme(payload?.theme) ? (payload?.theme ?? prev.mode) : prev.mode,
              darkThemeId:
                typeof payload?.darkThemeId === 'string' ? payload.darkThemeId : prev.darkThemeId,
              lightThemeId:
                typeof payload?.lightThemeId === 'string'
                  ? payload.lightThemeId
                  : prev.lightThemeId,
              customThemes: Array.isArray(payload?.customThemes)
                ? payload.customThemes.filter(isThemeDefinition)
                : prev.customThemes,
            }));
          }
        );
        if (isActive) {
          unlisten = unlistenFn;
        } else {
          unlistenFn();
        }
      } catch (error) {
        if (isActive) {
          console.warn('Failed to listen for theme preference updates:', error);
        }
      }
    })();

    return () => {
      isActive = false;
      if (unlisten) {
        unlisten();
      }
    };
  }, []);

  return preference;
};
