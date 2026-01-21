export const DEFAULT_ACCENT = '#3584e4';

export const normalizeHexColor = (value?: string | null): string | null => {
  if (!value) return null;
  const trimmed = value.trim().replace(/^#/, '').toLowerCase();
  if (!/^[0-9a-f]{6}$/.test(trimmed)) return null;
  return `#${trimmed}`;
};

export const applyAccentVariables = (hexColor: string) => {
  if (typeof document === 'undefined') return;
  const normalized = normalizeHexColor(hexColor);
  if (!normalized) return;
  const r = parseInt(normalized.slice(1, 3), 16);
  const g = parseInt(normalized.slice(3, 5), 16);
  const b = parseInt(normalized.slice(5, 7), 16);
  document.documentElement.style.setProperty('--accent', normalized);
  document.documentElement.style.setProperty('--accent-soft', `rgba(${r}, ${g}, ${b}, 0.15)`);
  document.documentElement.style.setProperty('--accent-selected', `rgba(${r}, ${g}, ${b}, 0.28)`);
  document.documentElement.style.setProperty('--accent-rgb', `${r}, ${g}, ${b}`);
  document.documentElement.style.setProperty('--color-app-accent', normalized);
};
