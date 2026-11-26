// Positions belong to one history query and its database revision.
export function selectionRange(anchor: number, target: number) {
  return { start: Math.min(anchor, target), end: Math.max(anchor, target) };
}

// Rows are ordered by position and may be sparse. Removed rows can be outside the cache.
export function omitHistoryRows<T>(
  rows: ReadonlyMap<number, T>,
  removedPositions: Iterable<number>,
  total: number,
) {
  const removed = [...new Set(removedPositions)]
    .filter((index) => index >= 0 && index < total)
    .sort((a, b) => a - b);
  const remaining = new Map<number, T>();
  let skipped = 0;
  for (const [index, item] of rows) {
    while (skipped < removed.length && removed[skipped]! < index) skipped++;
    if (removed[skipped] !== index) remaining.set(index - skipped, item);
  }
  return { rows: remaining, total: total - removed.length };
}
