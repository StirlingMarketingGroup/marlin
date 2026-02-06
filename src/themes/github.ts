import type { ThemeDefinition } from '@/types';

export const githubDarkTheme: ThemeDefinition = {
  id: 'github-dark',
  name: 'GitHub Dark',
  author: 'GitHub',
  colorScheme: 'dark',
  colors: {
    appDark: '#0d1117',
    appDarker: '#010409',
    appGray: '#161b22',
    appLight: '#21262d',
    text: '#c9d1d9',
    muted: '#8b949e',
    border: '#30363d',
    accent: '#58a6ff',
    green: '#3fb950',
    red: '#f85149',
    yellow: '#d29922',
  },
};

export const githubDarkColorblindTheme: ThemeDefinition = {
  id: 'github-dark-colorblind',
  name: 'GitHub Dark Colorblind',
  author: 'GitHub',
  colorScheme: 'dark',
  colors: {
    appDark: '#0d1117',
    appDarker: '#010409',
    appGray: '#161b22',
    appLight: '#21262d',
    text: '#c9d1d9',
    muted: '#8b949e',
    border: '#30363d',
    accent: '#79c0ff',
    green: '#79c0ff',
    red: '#ff7b72',
    yellow: '#e3b341',
  },
};

export const githubLightTheme: ThemeDefinition = {
  id: 'github-light',
  name: 'GitHub Light',
  author: 'GitHub',
  colorScheme: 'light',
  colors: {
    appDark: '#f6f8fa',
    appDarker: '#eaeef2',
    appGray: '#eaeef2',
    appLight: '#ffffff',
    text: '#24292f',
    muted: '#57606a',
    border: '#d0d7de',
    accent: '#0969da',
    green: '#1a7f37',
    red: '#cf222e',
    yellow: '#9a6700',
  },
};

export const githubLightColorblindTheme: ThemeDefinition = {
  id: 'github-light-colorblind',
  name: 'GitHub Light Colorblind',
  author: 'GitHub',
  colorScheme: 'light',
  colors: {
    appDark: '#f6f8fa',
    appDarker: '#eaeef2',
    appGray: '#eaeef2',
    appLight: '#ffffff',
    text: '#24292f',
    muted: '#57606a',
    border: '#d0d7de',
    accent: '#0550ae',
    green: '#0969da',
    red: '#d1242f',
    yellow: '#bf8700',
  },
};
