import { describe, expect, it } from 'vitest';
import { omitHistoryRows, selectionRange } from './selection';
describe('result selection', () => {
  it('selects inclusive ranges in display order in either direction', () => {
    expect(selectionRange(23, 30)).toEqual({ start: 23, end: 30 });
    expect(selectionRange(30, 23)).toEqual({ start: 23, end: 30 });
  });
  it('allows a single index as an inclusive range', () => {
    expect(selectionRange(25, 25)).toEqual({ start: 25, end: 25 });
  });
  it('shifts a sparse history window for removals both inside and outside the cache', () => {
    const rows = new Map([
      [0, 'head'],
      [10, 'visible'],
      [11, 'removed'],
      [12, 'next'],
    ]);
    const projected = omitHistoryRows(rows, [11, 3, 11, 16, 20], 20);
    expect([...projected.rows]).toEqual([
      [0, 'head'],
      [9, 'visible'],
      [10, 'next'],
    ]);
    expect(projected.total).toBe(17);
    expect(rows.get(11)).toBe('removed');
  });
});
