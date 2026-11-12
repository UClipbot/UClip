import { expect, test, vi } from 'vitest';
import { latestQuery } from './latest';

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

test('late query results cannot replace newer state', async () => {
  const gate = latestQuery();
  const old = deferred<string>();
  const apply = vi.fn();
  const fail = vi.fn();
  const first = gate.run(() => old.promise, apply, fail);
  await gate.run(async () => 'new', apply, fail);
  old.resolve('old');
  await first;
  expect(apply.mock.calls).toEqual([['new']]);
  expect(fail).not.toHaveBeenCalled();
});

test('cleanup invalidates in-flight errors and reports current errors', async () => {
  const gate = latestQuery();
  const old = deferred<string>();
  const fail = vi.fn();
  const task = gate.run(() => old.promise, vi.fn(), fail);
  gate.invalidate();
  old.reject(new Error('old'));
  await task;
  expect(fail).not.toHaveBeenCalled();
  await gate.run(
    async () => {
      throw new Error('current');
    },
    vi.fn(),
    fail,
  );
  expect(fail).toHaveBeenCalledOnce();
});
