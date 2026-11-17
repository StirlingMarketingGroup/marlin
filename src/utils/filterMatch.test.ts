import { describe, it, expect } from 'vitest';
import { filterMatchRanges, matchesFilter } from './filterMatch';

describe('matchesFilter', () => {
  it('contains the text when there is no star, ignoring case', () => {
    expect(matchesFilter('TestDocument.pdf', 'doc')).toBe(true);
    expect(matchesFilter('TestDocument.pdf', 'DOC')).toBe(true);
    expect(matchesFilter('notes.ai.bak', '.ai')).toBe(true);
    expect(matchesFilter('TestDocument.pdf', 'xyz')).toBe(false);
  });

  it('matches *.ai only as a whole-name suffix', () => {
    expect(matchesFilter('Logo.AI', '*.ai')).toBe(true);
    expect(matchesFilter('notes.ai.bak', '*.ai')).toBe(false);
    expect(matchesFilter('my.aiff', '*.ai')).toBe(false);
  });

  it('matches report* only as a prefix', () => {
    expect(matchesFilter('Report-2026.pdf', 'report*')).toBe(true);
    expect(matchesFilter('my report.pdf', 'report*')).toBe(false);
  });

  it('matches *2026*.pdf across the name', () => {
    expect(matchesFilter('a-2026-b.pdf', '*2026*.pdf')).toBe(true);
  });

  it('does not let the two sides of a*a share one character', () => {
    expect(matchesFilter('a', 'a*a')).toBe(false);
    expect(matchesFilter('aa', 'a*a')).toBe(true);
  });

  it('does not let a middle part use the characters of the last part', () => {
    expect(matchesFilter('x.pdf', '*pdf*.pdf')).toBe(false);
    expect(matchesFilter('pdf-x.pdf', '*pdf*.pdf')).toBe(true);
  });

  it('matches middle parts in order', () => {
    expect(matchesFilter('a-b-c.txt', '*a*b*c*')).toBe(true);
    expect(matchesFilter('b-a-c.txt', '*a*b*c*')).toBe(false);
  });

  it('matches every name for * and ** and returns no ranges', () => {
    expect(matchesFilter('anything', '*')).toBe(true);
    expect(matchesFilter('anything', '**')).toBe(true);
    expect(filterMatchRanges('anything', '*')).toEqual([]);
    expect(filterMatchRanges('anything', '**')).toEqual([]);
  });

  it('matches every name for empty text and returns no ranges', () => {
    expect(matchesFilter('anything', '')).toBe(true);
    expect(filterMatchRanges('anything', '')).toEqual([]);
  });

  it('treats ? as a plain character', () => {
    expect(matchesFilter('file?.txt', '?')).toBe(true);
    expect(matchesFilter('fileX.txt', 'file?.txt')).toBe(false);
  });

  it('matches a literal * in a name against a * pattern', () => {
    expect(matchesFilter('notes*.txt', '*.txt')).toBe(true);
    expect(matchesFilter('notes*.txt', 'notes*.txt')).toBe(true);
  });
});

describe('filterMatchRanges', () => {
  it('returns one range for a contains match', () => {
    expect(filterMatchRanges('TestDocument.pdf', 'doc')).toEqual([[4, 7]]);
  });

  it('returns the suffix range for *.ai', () => {
    expect(filterMatchRanges('Logo.AI', '*.ai')).toEqual([[4, 7]]);
  });

  it('returns the prefix range for report*', () => {
    expect(filterMatchRanges('Report-2026.pdf', 'report*')).toEqual([[0, 6]]);
  });

  it('returns both literal ranges for *2026*.pdf', () => {
    const name = 'a-2026-b.pdf';
    expect(filterMatchRanges(name, '*2026*.pdf')).toEqual([
      [2, 6],
      [name.length - 4, name.length],
    ]);
  });

  it('keeps offsets in the original name when İ precedes the match', () => {
    const name = 'İstanbul report.pdf';
    expect(filterMatchRanges(name, 'report')).toEqual([[9, 15]]);
    expect(name.slice(9, 15)).toBe('report');
    expect(matchesFilter(name, 'İstanbul')).toBe(true);
  });
});
