import { useEffect, useLayoutEffect, useRef, type ReactNode } from 'react';
import { createPortal } from 'react-dom';

export function ActionMenu({
  anchor,
  onClose,
  children,
}: {
  anchor: HTMLElement;
  onClose: () => void;
  children: ReactNode;
}) {
  const panel = useRef<HTMLDivElement>(null);
  const close = useRef(onClose);
  close.current = onClose;
  useLayoutEffect(() => {
    const element = panel.current!;
    const box = anchor.getBoundingClientRect();
    const width = element.offsetWidth;
    const height = element.offsetHeight;
    element.style.left = `${Math.max(8, Math.min(box.right - width, innerWidth - width - 8))}px`;
    element.style.top = `${Math.max(8, Math.min(box.bottom + 4, innerHeight - height - 8))}px`;
  }, [anchor]);
  useEffect(() => {
    const element = panel.current!;
    const items = () =>
      Array.from(element.querySelectorAll<HTMLButtonElement>('button:not(:disabled)'));
    items()[0]?.focus();
    const dismiss = (event: MouseEvent) => {
      if (!element.contains(event.target as Node) && !anchor.contains(event.target as Node))
        close.current();
    };
    const resize = () => close.current();
    const scroll = (event: Event) => {
      if (!element.contains(event.target as Node)) close.current();
    };
    document.addEventListener('mousedown', dismiss);
    window.addEventListener('resize', resize);
    window.addEventListener('scroll', scroll, true);
    return () => {
      document.removeEventListener('mousedown', dismiss);
      window.removeEventListener('resize', resize);
      window.removeEventListener('scroll', scroll, true);
      if (anchor.isConnected) anchor.focus({ preventScroll: true });
    };
  }, [anchor]);
  return createPortal(
    <div
      className="action-menu"
      ref={panel}
      role="menu"
      aria-label="操作菜单"
      onKeyDown={(event) => {
        event.stopPropagation();
        const items = Array.from(
          panel.current!.querySelectorAll<HTMLButtonElement>('button:not(:disabled)'),
        );
        const index = items.indexOf(document.activeElement as HTMLButtonElement);
        if (event.key === 'Escape' || event.key === 'Tab') {
          event.preventDefault();
          onClose();
        }
        if (['ArrowDown', 'ArrowUp', 'Home', 'End'].includes(event.key)) {
          event.preventDefault();
          const next =
            event.key === 'Home'
              ? 0
              : event.key === 'End'
                ? items.length - 1
                : (index + (event.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length;
          items[next]?.focus();
        }
      }}
    >
      {children}
    </div>,
    document.body,
  );
}
