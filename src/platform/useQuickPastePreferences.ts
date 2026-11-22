import { useCallback, useEffect, useRef, useState } from 'react';
import type { QuickPastePreferences, QuickPasteSettings } from '../generated/contracts';
import { getQuickPasteSettings, updateQuickPasteSettings } from './quickPaste';
import { useSettingsRefresh } from './useSettingsRefresh';

export function useQuickPastePreferences(enabled: boolean) {
  const [snapshot, setSnapshot] = useState<QuickPasteSettings | null>(null);
  const [busy, setBusy] = useState(false);
  const changing = useRef(false);
  const accept = useCallback((next: QuickPasteSettings) => {
    setSnapshot((previous) =>
      !previous || BigInt(next.preferences.revision) >= BigInt(previous.preferences.revision)
        ? next
        : previous,
    );
  }, []);
  const refresh = useCallback(async () => {
    if (!enabled) return;
    try {
      accept(await getQuickPasteSettings());
    } catch {
      // The startup settings notification refreshes the form after initialization.
    }
  }, [accept, enabled]);
  useSettingsRefresh(enabled, refresh);
  useEffect(() => {
    void refresh();
  }, [refresh]);

  async function save(update: Partial<Pick<QuickPastePreferences, 'shortcut' | 'includePinned'>>) {
    if (!snapshot || changing.current) return;
    const { shortcut, includePinned, revision } = snapshot.preferences;
    changing.current = true;
    setBusy(true);
    try {
      const saved = await updateQuickPasteSettings({
        shortcut,
        includePinned,
        ...update,
        expectedRevision: revision,
      });
      accept(saved);
      return saved;
    } catch (error) {
      void refresh();
      throw error;
    } finally {
      changing.current = false;
      setBusy(false);
    }
  }

  return { snapshot, busy, save };
}
