// Positions belong to one history query and its database revision.
export function selectionRange(anchor: number, target: number) {
  return { start: Math.min(anchor, target), end: Math.max(anchor, target) };
}
