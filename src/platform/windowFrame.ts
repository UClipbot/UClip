import { invoke, isTauri } from '@tauri-apps/api/core';
import { useLayoutEffect } from 'react';
import { latestQuery } from './latest';

type WindowFrame = {
  mode: 'inset' | 'flush' | 'system';
  nativeTitlebar: boolean;
  canUseNativeTitlebar: boolean;
};

export const windowFrame = isTauri()
  ? (window as Window & { __UCLIP_WINDOW_FRAME__?: WindowFrame }).__UCLIP_WINDOW_FRAME__
  : undefined;

export function useWindowFrame(enabled: boolean) {
  useLayoutEffect(() => {
    if (!enabled || !windowFrame) return;
    const mode = windowFrame.mode;
    const classes = document.body.classList;
    const frame = `window-frame-${mode}`;
    classes.add('custom-window-frame', frame);
    if (mode === 'system') {
      return () => classes.remove('custom-window-frame', frame);
    }
    const query = latestQuery();
    let animationFrame = 0;
    const refresh = () => {
      cancelAnimationFrame(animationFrame);
      animationFrame = requestAnimationFrame(() => {
        void query.run(
          () => invoke<boolean>('is_window_maximized'),
          (maximized) => classes.toggle('window-maximized', maximized),
          () => {},
        );
      });
    };
    refresh();
    window.addEventListener('resize', refresh);
    return () => {
      window.removeEventListener('resize', refresh);
      cancelAnimationFrame(animationFrame);
      query.invalidate();
      classes.remove('custom-window-frame', frame, 'window-maximized');
    };
  }, [enabled]);
}
