export type Theme = 'auto' | 'light' | 'dark';

export const THEME_KEY = 'workboard.theme';
export const THEMES: { id: Theme; label: string }[] = [
  { id: 'light', label: 'Light' },
  { id: 'dark', label: 'Dark' },
  { id: 'auto', label: 'Auto' }
];

export function parseTheme(value: unknown): Theme {
  return value === 'light' || value === 'dark' ? value : 'auto';
}

export function loadTheme(): Theme {
  try { return parseTheme(localStorage.getItem(THEME_KEY)); } catch { return 'auto'; }
}

export function applyTheme(theme: Theme) {
  const root = document.documentElement;
  if (theme === 'auto') delete root.dataset.theme;
  else root.dataset.theme = theme;
  try {
    if (theme === 'auto') localStorage.removeItem(THEME_KEY);
    else localStorage.setItem(THEME_KEY, theme);
  } catch { /* storage may be unavailable; the theme still applies for this visit */ }
}
