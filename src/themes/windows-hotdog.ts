import type { ThemeDefinition } from '@/types';

export const windowsHotdogTheme: ThemeDefinition = {
  id: 'windows-3-1-hotdog',
  name: 'Windows 3.1 Hotdog Stand',
  author: 'Microsoft',
  colorScheme: 'dark',
  colors: {
    appDark: '#000000',
    appDarker: '#000000',
    appGray: '#1a1a1a',
    appLight: '#2a2a2a',
    text: '#ffff00',
    muted: '#ffea00',
    border: '#ff0000',
    accent: '#ff00ff',
    green: '#00ff00',
    red: '#ff0000',
    yellow: '#ffff00',
  },
};

export const windowsHotdogLightTheme: ThemeDefinition = {
  id: 'windows-3-1-hotdog-light',
  name: 'Windows 3.1 Hotdog Stand (Light)',
  author: 'Microsoft',
  colorScheme: 'light',
  colors: {
    appDark: '#ffff00',
    appDarker: '#ff0000',
    appGray: '#ff0000',
    appLight: '#ff4d4d',
    text: '#000000',
    muted: '#1a1a1a',
    border: '#000000',
    accent: '#0000ff',
    green: '#00cc66',
    red: '#ff0000',
    yellow: '#ffff00',
  },
};
