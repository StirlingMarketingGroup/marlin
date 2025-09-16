/** @type {import('tailwindcss').Config} */
export default {
  content: ['./index.html', './src/**/*.{js,ts,jsx,tsx}'],
  theme: {
    extend: {
      colors: {
        // Modern dark theme color palette
        app: {
          dark: '#1e1e1e',
          darker: '#121212',
          gray: '#262626',
          light: '#2e2e2e',
          accent: '#3584e4',
          green: '#23a55a',
          red: '#e01b24',
          yellow: '#f6c84c',
          text: '#e6e6e7',
          muted: '#a1a1aa',
          border: '#3a3a3a',
        },
      },
      fontFamily: {
        // Prefer macOS system font to match Finder appearance
        sans: [
          'ui-sans-serif',
          '-apple-system',
          'BlinkMacSystemFont',
          'SF Pro Text',
          'SF Pro Display',
          'Segoe UI',
          'Helvetica Neue',
          'Helvetica',
          'Arial',
          'Noto Sans',
          'Liberation Sans',
          'sans-serif',
        ],
        mono: ['JetBrains Mono', 'ui-monospace', 'SFMono-Regular', 'Menlo', 'monospace'],
      },
    },
  },
  plugins: [],
  darkMode: 'class',
};
