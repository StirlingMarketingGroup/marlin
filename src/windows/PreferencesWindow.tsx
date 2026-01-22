import { useCallback, useEffect, useMemo, useState, type ChangeEvent } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { invoke } from '@tauri-apps/api/core';
import { emit } from '@tauri-apps/api/event';
import { X } from 'phosphor-react';
import { WINDOW_CONTENT_TOP_PADDING } from '@/windows/windowLayout';
import { applyAccentVariables, DEFAULT_ACCENT, normalizeHexColor } from '@/utils/accent';
import { PREFERENCES_UPDATED_EVENT } from '@/utils/events';
import type { Theme, ViewPreferences } from '@/types';

const ACCENT_POLL_INTERVAL_MS = 5000;

export default function PreferencesWindow() {
  const windowRef = getCurrentWindow();
  const [accentMode, setAccentMode] = useState<'system' | 'custom'>('system');
  const [customColor, setCustomColor] = useState(DEFAULT_ACCENT);
  const [systemColor, setSystemColor] = useState(DEFAULT_ACCENT);
  const [themePreference, setThemePreference] = useState<Theme>('system');

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
    } catch (error) {
      console.warn('Failed to persist preferences:', error);
    }
    try {
      await emit(PREFERENCES_UPDATED_EVENT, prefs);
    } catch (error) {
      console.warn('Failed to emit preferences update:', error);
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
        if (isActive) {
          setAccentMode(mode);
          setCustomColor(custom);
          setThemePreference(theme);
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

  return (
    <div className="min-h-screen bg-app-dark text-app-text">
      <div
        className="relative mx-auto flex h-full max-w-lg flex-col gap-4 px-6 pb-8"
        style={{ paddingTop: WINDOW_CONTENT_TOP_PADDING }}
      >
        <div data-tauri-drag-region className="absolute inset-x-2 top-0 h-10 rounded-lg" />

        <button
          onClick={() => void closeWindow()}
          className="absolute top-2 right-2 p-1.5 rounded-lg hover:bg-app-light/50 text-app-muted hover:text-app-text transition-colors"
          aria-label="Close"
          data-tauri-drag-region={false}
        >
          <X size={18} weight="bold" />
        </button>

        <header className="pt-2">
          <h2 className="text-lg font-semibold text-app-text">Preferences</h2>
        </header>

        <section className="space-y-3">
          <div className="text-xs uppercase tracking-wide text-app-muted">Appearance</div>
          <div className="rounded-lg border border-app-border bg-app-dark/50 p-4 space-y-3">
            <div className="flex flex-wrap items-center justify-between gap-3">
              <label className="text-sm text-app-text">Theme</label>
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
      </div>
    </div>
  );
}
