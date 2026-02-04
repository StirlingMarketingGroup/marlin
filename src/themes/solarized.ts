import type { ThemeDefinition } from '@/types';

export const solarizedDarkTheme: ThemeDefinition = {
  id: 'solarized-dark',
  name: 'Solarized Dark',
  author: 'Ethan Schoonover',
  colorScheme: 'dark',
  colors: {
    appDark: '#002b36',
    appDarker: '#001f27',
    appGray: '#073642',
    appLight: '#0f3b4a',
    text: '#93a1a1',
    muted: '#657b83',
    border: '#073642',
    accent: '#268bd2',
    green: '#859900',
    red: '#dc322f',
    yellow: '#b58900',
  },
};

export const solarizedLightTheme: ThemeDefinition = {
  id: 'solarized-light',
  name: 'Solarized Light',
  author: 'Ethan Schoonover',
  colorScheme: 'light',
  colors: {
    appDark: '#fdf6e3',
    appDarker: '#eee8d5',
    appGray: '#eee8d5',
    appLight: '#ffffff',
    text: '#586e75',
    muted: '#93a1a1',
    border: '#d7d2c0',
    accent: '#268bd2',
    green: '#859900',
    red: '#dc322f',
    yellow: '#b58900',
  },
};
