import { describe, expect, it } from 'vitest';
import { selectRange } from './selection';
describe('result selection', () => {
  it('selects inclusive ranges in display order in either direction', () => {
    expect(selectRange(['a', 'b', 'c', 'd'], 'b', 'd')).toEqual(['b', 'c', 'd']);
    expect(selectRange(['a', 'b', 'c', 'd'], 'd', 'b')).toEqual(['b', 'c', 'd']);
  });
  it('does not reuse an anchor from another result page', () => {
    expect(selectRange(['b', 'c'], 'a', 'c')).toEqual(['c']);
  });
});
