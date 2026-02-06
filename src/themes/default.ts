import type { ThemeDefinition } from '@/types';

export const defaultDarkTheme: ThemeDefinition = {
  id: 'default-dark',
  name: 'Default',
  colorScheme: 'dark',
  colors: {
    appDark: '#1e1e1e',
    appDarker: '#121212',
    appGray: '#262626',
    appLight: '#2e2e2e',
    text: '#e6e6e7',
    muted: '#a1a1aa',
    border: '#3a3a3a',
    accent: '#3584e4',
    green: '#23a55a',
    red: '#e01b24',
    yellow: '#f6c84c',
  },
};

export const defaultLightTheme: ThemeDefinition = {
  id: 'default-light',
  name: 'Default',
  colorScheme: 'light',
  colors: {
    appDark: '#f7f7f8',
    appDarker: '#eceef1',
    appGray: '#e5e7eb',
    appLight: '#ffffff',
    text: '#111827',
    muted: '#6b7280',
    border: '#d1d5db',
    accent: '#3584e4',
    green: '#23a55a',
    red: '#e01b24',
    yellow: '#f6c84c',
  },
};
