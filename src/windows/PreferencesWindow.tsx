import { useCallback, useEffect, useMemo, useState, type ChangeEvent } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { invoke } from '@tauri-apps/api/core';
import { emit } from '@tauri-apps/api/event';
import { message, open as openDialog } from '@tauri-apps/plugin-dialog';
import { platform } from '@tauri-apps/plugin-os';
import { readTextFile } from '@tauri-apps/plugin-fs';
import { X } from 'phosphor-react';
import { WINDOW_CONTENT_TOP_PADDING } from '@/windows/windowLayout';
import { applyAccentVariables, DEFAULT_ACCENT, normalizeHexColor } from '@/utils/accent';
import { PREFERENCES_UPDATED_EVENT } from '@/utils/events';
import { DEFAULT_THEME_IDS } from '@/themes';
import { useThemeRegistry } from '@/hooks/useThemeRegistry';
import { isThemeDefinition, mergeThemes, parseThemeFile } from '@/utils/theme';
import type { Theme, ThemeDefinition, ViewPreferences } from '@/types';

const ACCENT_POLL_INTERVAL_MS = 5000;

export default function PreferencesWindow() {
  const windowRef = getCurrentWindow();
  const [accentMode, setAccentMode] = useState<'system' | 'custom'>('system');
  const [customColor, setCustomColor] = useState(DEFAULT_ACCENT);
  const [systemColor, setSystemColor] = useState(DEFAULT_ACCENT);
  const [themePreference, setThemePreference] = useState<Theme>('system');
  const [darkThemeId, setDarkThemeId] = useState(DEFAULT_THEME_IDS.dark);
  const [lightThemeId, setLightThemeId] = useState(DEFAULT_THEME_IDS.light);
  const [customThemes, setCustomThemes] = useState<ThemeDefinition[]>([]);
  const themes = useThemeRegistry(customThemes);
  const isMac = platform() === 'macos';

  const themesByScheme = useMemo(() => {
    const byScheme = {
      dark: [] as ThemeDefinition[],
      light: [] as ThemeDefinition[],
    };
    themes.forEach((theme) => {
      byScheme[theme.colorScheme].push(theme);
    });
    byScheme.dark.sort((a, b) => a.name.localeCompare(b.name));
    byScheme.light.sort((a, b) => a.name.localeCompare(b.name));
    return byScheme;
  }, [themes]);

  const previewColor = useMemo(
    () => (accentMode === 'system' ? systemColor : customColor),
    [accentMode, customColor, systemColor]
  );

  const closeWindow = useCallback(async () => {
    try {
      await windowRef.close();
    } catch (error) {
      console.warn('Failed to close preferences window:', error);
    }
  }, [windowRef]);

  const persistPreferences = useCallback(async (prefs: Partial<ViewPreferences>) => {
    try {
      await invoke('set_global_prefs', { prefs: JSON.stringify(prefs) });
      await emit(PREFERENCES_UPDATED_EVENT, prefs);
    } catch (error) {
      console.warn('Failed to persist preferences:', error);
    }
  }, []);

  useEffect(() => {
    let isActive = true;

    const loadPreferences = async () => {
      try {
        const raw = await invoke<string>('read_preferences');
        if (!raw) return;
        const parsed = JSON.parse(raw || '{}') as { globalPreferences?: Partial<ViewPreferences> };
        const global = parsed.globalPreferences ?? {};
        const mode = global.accentColorMode === 'custom' ? 'custom' : 'system';
        const custom = normalizeHexColor(global.accentColorCustom) ?? DEFAULT_ACCENT;
        const theme =
          global.theme === 'light' || global.theme === 'dark' || global.theme === 'system'
            ? global.theme
            : 'system';
        const darkId =
          typeof global.darkThemeId === 'string' ? global.darkThemeId : DEFAULT_THEME_IDS.dark;
        const lightId =
          typeof global.lightThemeId === 'string' ? global.lightThemeId : DEFAULT_THEME_IDS.light;
        const storedThemes = Array.isArray(global.customThemes)
          ? global.customThemes.filter(isThemeDefinition)
          : [];
        if (isActive) {
          setAccentMode(mode);
          setCustomColor(custom);
          setThemePreference(theme);
          setDarkThemeId(darkId);
          setLightThemeId(lightId);
          setCustomThemes(storedThemes);
        }
      } catch (error) {
        console.warn('Failed to load preferences:', error);
      }
    };

    void loadPreferences();
    return () => {
      isActive = false;
    };
  }, []);

  useEffect(() => {
    let isActive = true;
    let intervalId: number | undefined;

    const updateSystemAccent = async () => {
      try {
        const accent = await invoke<string>('get_system_accent_color');
        const normalized = normalizeHexColor(accent) ?? DEFAULT_ACCENT;
        if (!isActive) return;
        setSystemColor(normalized);
        applyAccentVariables(normalized);
      } catch (error) {
        if (!isActive) return;
        setSystemColor(DEFAULT_ACCENT);
        applyAccentVariables(DEFAULT_ACCENT);
        console.warn('Failed to read system accent color:', error);
      }
    };

    if (accentMode === 'system') {
      void updateSystemAccent();
      intervalId = window.setInterval(updateSystemAccent, ACCENT_POLL_INTERVAL_MS);
    } else {
      applyAccentVariables(customColor);
    }

    return () => {
      isActive = false;
      if (intervalId) {
        window.clearInterval(intervalId);
      }
    };
  }, [accentMode, customColor]);

  useEffect(() => {
    const handler = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        void closeWindow();
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [closeWindow]);

  const handleModeChange = useCallback(
    (event: ChangeEvent<HTMLSelectElement>) => {
      const nextMode = event.target.value === 'custom' ? 'custom' : 'system';
      setAccentMode(nextMode);
      void persistPreferences({
        accentColorMode: nextMode,
        accentColorCustom: customColor,
      });
    },
    [customColor, persistPreferences]
  );

  const handleThemeChange = useCallback(
    (event: ChangeEvent<HTMLSelectElement>) => {
      const value = event.target.value;
      const nextTheme = value === 'light' || value === 'dark' ? value : 'system';
      setThemePreference(nextTheme);
      void persistPreferences({ theme: nextTheme });
    },
    [persistPreferences]
  );

  const handleColorChange = useCallback(
    (event: ChangeEvent<HTMLInputElement>) => {
      const normalized = normalizeHexColor(event.target.value) ?? DEFAULT_ACCENT;
      setCustomColor(normalized);
      setAccentMode('custom');
      void persistPreferences({
        accentColorMode: 'custom',
        accentColorCustom: normalized,
      });
    },
    [persistPreferences]
  );

  const handleThemeSelect = useCallback(
    (theme: ThemeDefinition) => {
      const mode = theme.colorScheme;
      const accent = normalizeHexColor(theme.colors.accent) ?? DEFAULT_ACCENT;
      setThemePreference(mode);
      setAccentMode('custom');
      setCustomColor(accent);
      if (theme.colorScheme === 'dark') {
        setDarkThemeId(theme.id);
        void persistPreferences({
          theme: mode,
          darkThemeId: theme.id,
          accentColorMode: 'custom',
          accentColorCustom: accent,
        });
      } else {
        setLightThemeId(theme.id);
        void persistPreferences({
          theme: mode,
          lightThemeId: theme.id,
          accentColorMode: 'custom',
          accentColorCustom: accent,
        });
      }
    },
    [persistPreferences]
  );

  const handleImportTheme = useCallback(async () => {
    try {
      const selection = await openDialog({
        title: 'Import Theme',
        multiple: false,
        directory: false,
        filters: [{ name: 'Theme files', extensions: ['json', 'itermcolors'] }],
      });
      if (!selection) return;
      const path = Array.isArray(selection) ? selection[0] : selection;
      if (!path) return;
      const content = await readTextFile(path);
      const theme = parseThemeFile(content, path);
      if (!theme) {
        await message('Unsupported theme format. Use JSON or iTerm2 .itermcolors files.', {
          title: 'Theme Import Failed',
          okLabel: 'OK',
          kind: 'error',
        });
        return;
      }
      setCustomThemes((prev) => {
        const next = mergeThemes([...prev.filter((item) => item.id !== theme.id), theme]);
        const accent = normalizeHexColor(theme.colors.accent) ?? DEFAULT_ACCENT;
        setAccentMode('custom');
        setCustomColor(accent);
        const payload: Partial<ViewPreferences> = {
          customThemes: next,
          accentColorMode: 'custom',
          accentColorCustom: accent,
        };
        if (theme.colorScheme === 'dark') {
          setDarkThemeId(theme.id);
          payload.darkThemeId = theme.id;
        } else {
          setLightThemeId(theme.id);
          payload.lightThemeId = theme.id;
        }
        void persistPreferences(payload);
        return next;
      });
    } catch (error) {
      console.warn('Failed to import theme:', error);
      await message('Unable to import theme file.', {
        title: 'Theme Import Failed',
        okLabel: 'OK',
        kind: 'error',
      });
    }
  }, [persistPreferences]);

  return (
    <div className="h-screen bg-app-dark text-app-text">
      <div className="relative mx-auto flex h-full max-w-lg flex-col">
        <div
          data-tauri-drag-region
          className="flex items-center justify-between border-b border-app-border bg-app-dark"
          style={{
            minHeight: WINDOW_CONTENT_TOP_PADDING,
            paddingLeft: isMac ? '5.25rem' : '1.5rem',
            paddingRight: '1.5rem',
          }}
        >
          <h2 className="text-lg font-semibold text-app-text">Preferences</h2>
          {!isMac ? (
            <button
              onClick={() => void closeWindow()}
              className="p-1.5 rounded-lg hover:bg-app-light/50 text-app-muted hover:text-app-text transition-colors"
              aria-label="Close"
              data-tauri-drag-region={false}
            >
              <X size={18} weight="bold" />
            </button>
          ) : null}
        </div>

        <div className="flex-1 overflow-y-auto px-6 pb-8 pt-4">
          <section className="space-y-3">
            <div className="text-xs uppercase tracking-wide text-app-muted">Appearance</div>
            <div className="rounded-lg border border-app-border bg-app-dark/50 p-4 space-y-3">
              <div className="flex flex-wrap items-center justify-between gap-3">
                <label className="text-sm text-app-text">Theme Mode</label>
                <select
                  className="input-field h-9"
                  value={themePreference}
                  onChange={handleThemeChange}
                  data-tauri-drag-region={false}
                >
                  <option value="system">System</option>
                  <option value="light">Light</option>
                  <option value="dark">Dark</option>
                </select>
              </div>
              <div className="flex flex-wrap items-center justify-between gap-3">
                <label className="text-sm text-app-text">Accent Color</label>
                <div className="flex items-center gap-2">
                  <select
                    className="input-field h-9"
                    value={accentMode}
                    onChange={handleModeChange}
                    data-tauri-drag-region={false}
                  >
                    <option value="system">System</option>
                    <option value="custom">Custom</option>
                  </select>
                  <input
                    type="color"
                    value={customColor}
                    onChange={handleColorChange}
                    className="h-9 w-9 rounded-md border border-app-border bg-app-gray p-0.5"
                    aria-label="Pick accent color"
                    data-tauri-drag-region={false}
                  />
                </div>
              </div>

              <div className="flex items-center gap-3 rounded-lg border border-app-border bg-app-gray/40 px-3 py-2">
                <div
                  className="h-3.5 w-3.5 rounded-full border border-app-border"
                  style={{ backgroundColor: previewColor }}
                />
                <div className="text-xs text-app-muted">Live preview</div>
                <div
                  className="ml-auto h-6 w-16 rounded-md shadow-inner"
                  style={{ backgroundColor: previewColor }}
                />
              </div>
            </div>
          </section>

          <section className="space-y-3 mt-4">
            <div className="text-xs uppercase tracking-wide text-app-muted">Themes</div>
            <div className="rounded-lg border border-app-border bg-app-dark/50 p-4 space-y-4">
              <div className="text-xs text-app-muted">
                Pick a theme for dark and light modes. System mode uses both selections.
              </div>

              <div className="space-y-2">
                <div className="text-sm font-semibold text-app-text">Dark Themes</div>
                <div className="grid gap-3 sm:grid-cols-2">
                  {themesByScheme.dark.map((theme) => (
                    <button
                      key={theme.id}
                      type="button"
                      onClick={() => handleThemeSelect(theme)}
                      className={`rounded-lg border p-3 text-left transition-colors focus:outline-none focus:ring-2 focus:ring-[var(--accent)] ${
                        theme.id === darkThemeId
                          ? 'ring-2 ring-[var(--accent)]'
                          : 'hover:border-app-muted'
                      }`}
                      style={{
                        backgroundColor: theme.colors.appDark,
                        color: theme.colors.text,
                        borderColor:
                          theme.id === darkThemeId ? 'var(--accent)' : theme.colors.border,
                      }}
                      aria-pressed={theme.id === darkThemeId}
                      data-tauri-drag-region={false}
                    >
                      <div className="flex items-start justify-between gap-2">
                        <div>
                          <div className="text-sm font-semibold">{theme.name}</div>
                          {theme.author ? (
                            <div className="text-xs" style={{ color: theme.colors.muted }}>
                              {theme.author}
                            </div>
                          ) : null}
                        </div>
                        <span
                          className="text-[10px] uppercase tracking-wide px-2 py-0.5 rounded"
                          style={{
                            backgroundColor: theme.colors.appGray,
                            color: theme.colors.text,
                          }}
                        >
                          Dark
                        </span>
                      </div>
                      <div className="mt-2 flex flex-wrap gap-1">
                        {[
                          theme.colors.appDark,
                          theme.colors.appGray,
                          theme.colors.appLight,
                          theme.colors.accent,
                          theme.colors.green,
                          theme.colors.red,
                          theme.colors.yellow,
                        ].map((color, index) => (
                          <span
                            key={`${theme.id}-dark-${index}`}
                            className="h-3 w-3 rounded-sm border"
                            style={{ backgroundColor: color, borderColor: theme.colors.border }}
                          />
                        ))}
                      </div>
                    </button>
                  ))}
                </div>
              </div>

              <div className="space-y-2">
                <div className="text-sm font-semibold text-app-text">Light Themes</div>
                <div className="grid gap-3 sm:grid-cols-2">
                  {themesByScheme.light.map((theme) => (
                    <button
                      key={theme.id}
                      type="button"
                      onClick={() => handleThemeSelect(theme)}
                      className={`rounded-lg border p-3 text-left transition-colors focus:outline-none focus:ring-2 focus:ring-[var(--accent)] ${
                        theme.id === lightThemeId
                          ? 'ring-2 ring-[var(--accent)]'
                          : 'hover:border-app-muted'
                      }`}
                      style={{
                        backgroundColor: theme.colors.appDark,
                        color: theme.colors.text,
                        borderColor:
                          theme.id === lightThemeId ? 'var(--accent)' : theme.colors.border,
                      }}
                      aria-pressed={theme.id === lightThemeId}
                      data-tauri-drag-region={false}
                    >
                      <div className="flex items-start justify-between gap-2">
                        <div>
                          <div className="text-sm font-semibold">{theme.name}</div>
                          {theme.author ? (
                            <div className="text-xs" style={{ color: theme.colors.muted }}>
                              {theme.author}
                            </div>
                          ) : null}
                        </div>
                        <span
                          className="text-[10px] uppercase tracking-wide px-2 py-0.5 rounded"
                          style={{
                            backgroundColor: theme.colors.appGray,
                            color: theme.colors.text,
                          }}
                        >
                          Light
                        </span>
                      </div>
                      <div className="mt-2 flex flex-wrap gap-1">
                        {[
                          theme.colors.appDark,
                          theme.colors.appGray,
                          theme.colors.appLight,
                          theme.colors.accent,
                          theme.colors.green,
                          theme.colors.red,
                          theme.colors.yellow,
                        ].map((color, index) => (
                          <span
                            key={`${theme.id}-light-${index}`}
                            className="h-3 w-3 rounded-sm border"
                            style={{ backgroundColor: color, borderColor: theme.colors.border }}
                          />
                        ))}
                      </div>
                    </button>
                  ))}
                </div>
              </div>

              <div className="flex flex-wrap items-center justify-between gap-3 pt-1">
                <button
                  type="button"
                  onClick={() => void handleImportTheme()}
                  className="button-secondary"
                  data-tauri-drag-region={false}
                >
                  Import Theme
                </button>
                <div className="text-xs text-app-muted">
                  Supports JSON and iTerm2 .itermcolors files.
                </div>
              </div>
            </div>
          </section>
        </div>
      </div>
    </div>
  );
}
