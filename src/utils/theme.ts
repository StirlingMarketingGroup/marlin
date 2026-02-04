import { readDir, readTextFile } from '@tauri-apps/plugin-fs';
import { BaseDirectory } from '@tauri-apps/api/path';
import { normalizeHexColor } from '@/utils/accent';
import { defaultDarkTheme, defaultLightTheme } from '@/themes/default';
import type { ThemeColorScheme, ThemeDefinition } from '@/types';

const REQUIRED_COLOR_KEYS = [
  'appDark',
  'appDarker',
  'appGray',
  'appLight',
  'text',
  'muted',
  'border',
  'accent',
  'green',
  'red',
  'yellow',
] as const;

const THEME_FILE_EXTENSIONS = ['.json', '.itermcolors'];

const clamp = (value: number, min = 0, max = 255) => Math.min(max, Math.max(min, value));

const toHexChannel = (value: number) => clamp(Math.round(value)).toString(16).padStart(2, '0');

const rgbToHex = (r: number, g: number, b: number) =>
  `#${toHexChannel(r)}${toHexChannel(g)}${toHexChannel(b)}`;

const hexToRgb = (hex: string) => {
  const normalized = normalizeHexColor(hex);
  if (!normalized) return null;
  return {
    r: Number.parseInt(normalized.slice(1, 3), 16),
    g: Number.parseInt(normalized.slice(3, 5), 16),
    b: Number.parseInt(normalized.slice(5, 7), 16),
  };
};

const mixColors = (hexA: string, hexB: string, amount: number) => {
  const a = hexToRgb(hexA);
  const b = hexToRgb(hexB);
  if (!a || !b) return hexA;
  const mix = (x: number, y: number) => x + (y - x) * amount;
  return rgbToHex(mix(a.r, b.r), mix(a.g, b.g), mix(a.b, b.b));
};

const luminance = (hex: string) => {
  const rgb = hexToRgb(hex);
  if (!rgb) return 0;
  return (0.2126 * rgb.r + 0.7152 * rgb.g + 0.0722 * rgb.b) / 255;
};

const slugify = (value: string) =>
  value
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/(^-|-$)+/g, '');

const extractBasename = (filePath: string | undefined, extension: RegExp): string | undefined =>
  filePath?.split(/[\\/]/).pop()?.replace(extension, '');

const inferScheme = (background: string): ThemeColorScheme =>
  luminance(background) < 0.5 ? 'dark' : 'light';

const getFallbackTheme = (scheme: ThemeColorScheme) =>
  scheme === 'light' ? defaultLightTheme : defaultDarkTheme;

const normalizeThemeColors = (
  colors: Record<string, unknown>
): ThemeDefinition['colors'] | null => {
  const normalized: Partial<ThemeDefinition['colors']> = {};
  for (const key of REQUIRED_COLOR_KEYS) {
    const raw = colors[key];
    if (typeof raw !== 'string') return null;
    const hex = normalizeHexColor(raw);
    if (!hex) return null;
    normalized[key] = hex;
  }
  return normalized as ThemeDefinition['colors'];
};

export const isThemeDefinition = (value: unknown): value is ThemeDefinition => {
  if (!value || typeof value !== 'object') return false;
  const theme = value as ThemeDefinition;
  if (typeof theme.id !== 'string' || typeof theme.name !== 'string') return false;
  if (theme.colorScheme !== 'dark' && theme.colorScheme !== 'light') return false;
  if (!theme.colors || typeof theme.colors !== 'object') return false;
  return REQUIRED_COLOR_KEYS.every(
    (key) => typeof (theme.colors as Record<string, unknown>)[key] === 'string'
  );
};

export const normalizeThemeDefinition = (
  value: unknown,
  fallbackName?: string,
  fallbackId?: string
): ThemeDefinition | null => {
  if (!value || typeof value !== 'object') return null;
  const raw = value as Partial<ThemeDefinition> & { colors?: Record<string, unknown> };
  const name = typeof raw.name === 'string' ? raw.name.trim() : fallbackName?.trim() || '';
  const idRaw = typeof raw.id === 'string' ? raw.id.trim() : '';
  const id = idRaw || slugify(name || fallbackId || 'theme');
  const colors = raw.colors ? normalizeThemeColors(raw.colors) : null;
  if (!colors || !id) return null;
  const scheme =
    raw.colorScheme === 'dark' || raw.colorScheme === 'light'
      ? raw.colorScheme
      : inferScheme(colors.appDark);
  const author = typeof raw.author === 'string' ? raw.author.trim() : undefined;
  return {
    id,
    name: name || id,
    author: author || undefined,
    colorScheme: scheme,
    colors,
  };
};

const parsePlistDict = (element: Element): Record<string, unknown> => {
  const result: Record<string, unknown> = {};
  const children = Array.from(element.childNodes).filter(
    (node): node is Element => node.nodeType === Node.ELEMENT_NODE
  );
  for (let i = 0; i < children.length; i++) {
    const node = children[i];
    if (node.tagName !== 'key') {
      continue;
    }
    const key = node.textContent?.trim() || '';
    const valueNode = children[i + 1];
    if (!key || !valueNode) {
      continue;
    }
    result[key] = parsePlistValue(valueNode);
    i += 1;
  }
  return result;
};

const parsePlistValue = (element: Element): unknown => {
  switch (element.tagName) {
    case 'dict':
      return parsePlistDict(element);
    case 'string':
      return element.textContent?.trim() ?? '';
    case 'integer':
    case 'real':
      return Number.parseFloat(element.textContent ?? '0');
    case 'true':
      return true;
    case 'false':
      return false;
    default:
      return element.textContent?.trim() ?? '';
  }
};

const parsePlistColor = (value: unknown): string | null => {
  if (!value || typeof value !== 'object') return null;
  const dict = value as Record<string, unknown>;
  const readComponent = (key: string) => {
    const raw = dict[key];
    if (typeof raw !== 'number' || Number.isNaN(raw)) return null;
    const normalized = raw <= 1 ? raw * 255 : raw;
    return clamp(normalized);
  };
  const r = readComponent('Red Component');
  const g = readComponent('Green Component');
  const b = readComponent('Blue Component');
  if (r === null || g === null || b === null) return null;
  return rgbToHex(r, g, b);
};

const buildThemeFromBase = (options: {
  name: string;
  background: string;
  foreground: string;
  accent?: string | null;
  red?: string | null;
  green?: string | null;
  yellow?: string | null;
  author?: string;
}): ThemeDefinition => {
  const scheme = inferScheme(options.background);
  const fallback = getFallbackTheme(scheme);
  const background = normalizeHexColor(options.background) ?? fallback.colors.appDark;
  const foreground = normalizeHexColor(options.foreground) ?? fallback.colors.text;
  const baseAccent = normalizeHexColor(options.accent ?? '') ?? fallback.colors.accent;
  const baseRed = normalizeHexColor(options.red ?? '') ?? fallback.colors.red;
  const baseGreen = normalizeHexColor(options.green ?? '') ?? fallback.colors.green;
  const baseYellow = normalizeHexColor(options.yellow ?? '') ?? fallback.colors.yellow;
  const lightMix = scheme === 'dark' ? '#ffffff' : '#000000';
  const subtleMix = scheme === 'dark' ? 0.14 : 0.08;
  const lighterMix = scheme === 'dark' ? 0.2 : 0.04;
  const darkerMix = scheme === 'dark' ? 0.2 : 0.06;

  return {
    id: slugify(options.name),
    name: options.name,
    author: options.author,
    colorScheme: scheme,
    colors: {
      appDark: background,
      appDarker: mixColors(background, '#000000', darkerMix),
      appGray: mixColors(background, lightMix, subtleMix),
      appLight: mixColors(background, lightMix, lighterMix),
      text: foreground,
      muted: mixColors(foreground, background, 0.5),
      border: mixColors(background, foreground, 0.16),
      accent: baseAccent,
      green: baseGreen,
      red: baseRed,
      yellow: baseYellow,
    },
  };
};

export const parseItermColors = (content: string, fileName?: string): ThemeDefinition | null => {
  if (typeof DOMParser === 'undefined') return null;
  const parser = new DOMParser();
  const doc = parser.parseFromString(content, 'application/xml');
  const rootDict = doc.querySelector('plist > dict');
  if (!rootDict) return null;
  const data = parsePlistDict(rootDict);
  const background = parsePlistColor(data['Background Color']);
  const foreground = parsePlistColor(data['Foreground Color']);
  if (!background || !foreground) return null;
  const nameFromFile = extractBasename(fileName, /\.itermcolors$/i) ?? 'iTerm Theme';
  return buildThemeFromBase({
    name: nameFromFile,
    background,
    foreground,
    accent:
      parsePlistColor(data['Selection Color']) ??
      parsePlistColor(data['Cursor Color']) ??
      parsePlistColor(data['ANSI Blue Color']),
    red: parsePlistColor(data['ANSI Red Color']),
    green: parsePlistColor(data['ANSI Green Color']),
    yellow: parsePlistColor(data['ANSI Yellow Color']),
  });
};

export const parseThemeJson = (content: string, fileName?: string): ThemeDefinition | null => {
  let parsed: unknown;
  try {
    parsed = JSON.parse(content);
  } catch {
    return null;
  }
  const fallbackName = extractBasename(fileName, /\.json$/i);
  return normalizeThemeDefinition(parsed, fallbackName, fallbackName);
};

export const parseThemeFile = (content: string, fileName?: string): ThemeDefinition | null => {
  const lower = fileName?.toLowerCase() ?? '';
  if (lower.endsWith('.itermcolors')) {
    return parseItermColors(content, fileName);
  }
  if (lower.endsWith('.json')) {
    return parseThemeJson(content, fileName);
  }
  return parseThemeJson(content, fileName) ?? parseItermColors(content, fileName);
};

export const applyThemeDefinition = (theme: ThemeDefinition) => {
  if (typeof document === 'undefined') return;
  const root = document.documentElement;
  root.dataset.theme = theme.colorScheme;
  root.style.colorScheme = theme.colorScheme;
  root.style.setProperty('--color-app-dark', theme.colors.appDark);
  root.style.setProperty('--color-app-darker', theme.colors.appDarker);
  root.style.setProperty('--color-app-gray', theme.colors.appGray);
  root.style.setProperty('--color-app-light', theme.colors.appLight);
  root.style.setProperty('--color-app-text', theme.colors.text);
  root.style.setProperty('--color-app-muted', theme.colors.muted);
  root.style.setProperty('--color-app-border', theme.colors.border);
  root.style.setProperty('--color-app-accent', theme.colors.accent);
  root.style.setProperty('--color-app-green', theme.colors.green);
  root.style.setProperty('--color-app-red', theme.colors.red);
  root.style.setProperty('--color-app-yellow', theme.colors.yellow);

  const rgb = hexToRgb(theme.colors.appDark);
  if (rgb) {
    root.style.setProperty('--bg-rgb', `${rgb.r}, ${rgb.g}, ${rgb.b}`);
  }
  root.style.setProperty('--checker-bg', theme.colors.appDarker);
  root.style.setProperty(
    '--checker-overlay',
    theme.colorScheme === 'light' ? 'rgba(0, 0, 0, 0.05)' : 'rgba(255, 255, 255, 0.08)'
  );
};

export const loadThemesFromConfigDir = async (): Promise<ThemeDefinition[]> => {
  try {
    const entries = await readDir('themes', { baseDir: BaseDirectory.AppConfig });
    const files = entries.filter((entry) => {
      if (entry.isDirectory) return false;
      if (!entry.name) return false;
      const lower = entry.name.toLowerCase();
      return THEME_FILE_EXTENSIONS.some((ext) => lower.endsWith(ext));
    });

    const parsed = await Promise.all(
      files.map(async (entry) => {
        try {
          const filePath = `themes/${entry.name}`;
          const content = await readTextFile(filePath, { baseDir: BaseDirectory.AppConfig });
          return parseThemeFile(content, entry.name ?? filePath);
        } catch (error) {
          console.warn('Failed to read theme file:', entry.name, error);
          return null;
        }
      })
    );
    return parsed.filter((theme): theme is ThemeDefinition => Boolean(theme));
  } catch (error) {
    if (import.meta.env.DEV) {
      console.warn('Failed to load themes from config directory:', error);
    }
    return [];
  }
};

export const mergeThemes = (themes: ThemeDefinition[]): ThemeDefinition[] => {
  const byId = new Map<string, ThemeDefinition>();
  themes.forEach((theme) => {
    if (!theme?.id) return;
    byId.set(theme.id, theme);
  });
  return Array.from(byId.values());
};
