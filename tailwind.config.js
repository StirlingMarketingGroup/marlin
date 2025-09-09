/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  theme: {
    extend: {
      colors: {
        // Discord-inspired color palette
        discord: {
          dark: '#1e1f22',
          darker: '#111214', 
          gray: '#2b2d31',
          light: '#313338',
          accent: '#5865f2',
          green: '#23a55a',
          red: '#f23f43',
          yellow: '#f0b132',
          text: '#dbdee1',
          muted: '#80848e',
          border: '#3f4147'
        }
      },
      fontFamily: {
        sans: ['Inter', 'system-ui', 'sans-serif'],
        mono: ['JetBrains Mono', 'monospace']
      }
    },
  },
  plugins: [],
  darkMode: 'class'
}