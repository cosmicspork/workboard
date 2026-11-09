import { describe, expect, test } from 'bun:test';
import { parseTheme } from './theme';

describe('parseTheme', () => {
  test('keeps explicit choices', () => {
    expect(parseTheme('light')).toBe('light');
    expect(parseTheme('dark')).toBe('dark');
  });
  test('falls back to following the system', () => {
    for (const value of [null, '', 'auto', 'sepia', 1]) expect(parseTheme(value)).toBe('auto');
  });
});
