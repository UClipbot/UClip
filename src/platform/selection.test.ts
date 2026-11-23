import { describe, expect, it } from 'vitest';
import { selectionRange } from './selection';
describe('result selection', () => {
  it('selects inclusive ranges in display order in either direction', () => {
    expect(selectionRange(23, 30)).toEqual({ start: 23, end: 30 });
    expect(selectionRange(30, 23)).toEqual({ start: 23, end: 30 });
  });
  it('allows a single index as an inclusive range', () => {
    expect(selectionRange(25, 25)).toEqual({ start: 25, end: 25 });
  });
});
