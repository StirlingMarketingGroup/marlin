import type { ThemeDefinition } from '@/types';
import { defaultDarkTheme, defaultLightTheme } from './default';
import {
  githubDarkColorblindTheme,
  githubDarkTheme,
  githubLightColorblindTheme,
  githubLightTheme,
} from './github';
import { monokaiProTheme } from './monokai-pro';
import { monokaiProOctagonTheme } from './monokai-pro-octagon';
import { gruvboxTheme } from './gruvbox';
import { solarizedDarkTheme, solarizedLightTheme } from './solarized';
import { draculaTheme } from './dracula';
import { catppuccinLatteTheme, catppuccinMochaTheme } from './catppuccin';
import { synthwaveTheme } from './synthwave';
import { nordTheme } from './nord';
import { ubuntuTheme } from './ubuntu';
import { windowsHotdogLightTheme, windowsHotdogTheme } from './windows-hotdog';
import { windowsXpTheme } from './windows-xp';

export const BUILT_IN_THEMES: ThemeDefinition[] = [
  defaultDarkTheme,
  defaultLightTheme,
  githubDarkTheme,
  githubDarkColorblindTheme,
  githubLightTheme,
  githubLightColorblindTheme,
  monokaiProTheme,
  monokaiProOctagonTheme,
  gruvboxTheme,
  solarizedDarkTheme,
  solarizedLightTheme,
  draculaTheme,
  catppuccinMochaTheme,
  catppuccinLatteTheme,
  nordTheme,
  ubuntuTheme,
  synthwaveTheme,
  windowsHotdogTheme,
  windowsHotdogLightTheme,
  windowsXpTheme,
];

export const DEFAULT_THEME_IDS = {
  dark: defaultDarkTheme.id,
  light: defaultLightTheme.id,
} as const;
