/** Prevent an earlier query from replacing a newer result after input/focus changes. */
export function latestQuery() {
  let generation = 0;
  return {
    invalidate: () => {
      generation += 1;
    },
    async run<T>(
      query: () => Promise<T>,
      apply: (value: T) => void,
      fail: (error: unknown) => void,
    ) {
      const ownGeneration = ++generation;
      try {
        const value = await query();
        if (ownGeneration === generation) apply(value);
      } catch (error) {
        if (ownGeneration === generation) fail(error);
      }
    },
  };
}
