// toLowerCase() can grow a character ('İ' becomes two code units) and shift every later offset.
function lowerKeepingLength(text: string): string {
  const lowered = text.toLowerCase();
  if (lowered.length === text.length) return lowered;
  let out = '';
  for (const ch of text) {
    const next = ch.toLowerCase();
    out += next.length === ch.length ? next : ch;
  }
  return out;
}

/**
 * Where the filter text matched in a file name, as [start, end) ranges in order,
 * or null when the name does not match.
 */
export function filterMatchRanges(name: string, filterText: string): [number, number][] | null {
  if (filterText === '') return [];

  const nameLower = lowerKeepingLength(name);
  const filterLower = lowerKeepingLength(filterText);

  if (!filterLower.includes('*')) {
    const index = nameLower.indexOf(filterLower);
    if (index === -1) return null;
    return [[index, index + filterLower.length]];
  }

  // With a `*`, the pattern must match the whole name: the first part starts it and the last part ends it.
  const parts = filterLower.split('*');
  const first = parts[0];
  const last = parts[parts.length - 1];
  const lastStart = nameLower.length - last.length;
  if (!nameLower.startsWith(first) || !nameLower.endsWith(last) || lastStart < first.length) {
    return null;
  }

  const ranges: [number, number][] = [];
  if (first.length > 0) ranges.push([0, first.length]);

  let pos = first.length;
  for (const part of parts.slice(1, -1)) {
    if (part.length === 0) continue;
    const index = nameLower.indexOf(part, pos);
    if (index === -1 || index + part.length > lastStart) return null;
    ranges.push([index, index + part.length]);
    pos = index + part.length;
  }

  if (last.length > 0) ranges.push([lastStart, nameLower.length]);
  return ranges;
}

export function matchesFilter(name: string, filterText: string): boolean {
  return filterMatchRanges(name, filterText) !== null;
}
