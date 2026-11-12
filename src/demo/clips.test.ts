import { expect, test } from 'vitest';
import { initialClips, queryClips } from './clips';

test('regex searches complete multiline content and rejects invalid expressions visibly', () => {
  expect(
    queryClips(initialClips, 'all', 'all', '轻一点[\\s\\S]*清楚一点', true, false).items.map(
      (clip) => clip.id,
    ),
  ).toEqual(['note-1']);
  const invalid = queryClips(initialClips, 'all', 'all', '[', true, false);
  expect(invalid.error).not.toBe('');
  expect(invalid.items).toEqual([]);
});

test('deleted examples leave ordinary views while remaining recoverable in trash', () => {
  const clips = initialClips.map((clip) => ({ ...clip, deleted: clip.id === 'note-1' }));
  expect(
    queryClips(clips, 'all', 'all', '', false, false).items.some((clip) => clip.id === 'note-1'),
  ).toBe(false);
  expect(queryClips(clips, 'trash', 'all', '', false, false).items.map((clip) => clip.id)).toEqual([
    'note-1',
  ]);
  expect(
    queryClips(clips, 'pinned', 'all', '', false, false).items.some((clip) => clip.id === 'note-1'),
  ).toBe(false);
});

test('collection and type filters compose and sorting does not mutate the original examples', () => {
  const before = initialClips.map((clip) => clip.id);
  const filtered = queryClips(initialClips, 'inspiration', 'image', '', false, false);
  expect(filtered.items.map((clip) => clip.id)).toEqual(['image-1']);
  expect(queryClips(initialClips, 'all', 'all', '', false, true).items[0]?.id).toBe('file-2');
  expect(initialClips.map((clip) => clip.id)).toEqual(before);
});
