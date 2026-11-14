// Selection is restricted to the current result page, in display order.
export function selectRange(ids: string[], anchor: string, target: string): string[] {
  const start = ids.indexOf(anchor);
  const end = ids.indexOf(target);
  if (start < 0 || end < 0) return [target];
  return ids.slice(Math.min(start, end), Math.max(start, end) + 1);
}
