import { t } from '../i18n';
import { useEffect, useRef, type ComponentProps, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { Icon } from './Icon';
import { WindowTitlebar } from './WindowTitlebar';

export function Modal({
  title,
  description,
  onClose,
  children,
  wide = false,
  showCloseButton = true,
  titlebar,
}: {
  title: string;
  description?: string;
  onClose: () => void;
  children: ReactNode;
  wide?: boolean;
  showCloseButton?: boolean;
  titlebar?: Omit<ComponentProps<typeof WindowTitlebar>, 'title' | 'titleId' | 'onClose'>;
}) {
  const panel = useRef<HTMLDivElement>(null);
  const closeRef = useRef(onClose);
  closeRef.current = onClose;
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    const element = panel.current;
    const targets = () =>
      Array.from(
        element?.querySelectorAll<HTMLElement>(
          'button:not(:disabled), input:not(:disabled), textarea:not(:disabled), select:not(:disabled), summary, a[href], [tabindex="0"]',
        ) ?? [],
      ).filter((target) => target.getClientRects().length);
    (element?.querySelector<HTMLElement>('[data-autofocus]') ?? targets()[0] ?? element)?.focus();
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !event.isComposing && event.keyCode !== 229) {
        event.preventDefault();
        event.stopPropagation();
        closeRef.current();
      }
      if (event.key === 'Tab') {
        const elements = targets();
        const first = elements[0];
        const last = elements[elements.length - 1];
        if (!first) {
          event.preventDefault();
          return;
        }
        if (event.shiftKey && document.activeElement === first) {
          event.preventDefault();
          last?.focus();
        }
        if (!event.shiftKey && document.activeElement === last) {
          event.preventDefault();
          first.focus();
        }
      }
    };
    element?.addEventListener('keydown', onKey);
    return () => {
      element?.removeEventListener('keydown', onKey);
      previous?.focus();
    };
  }, []);
  return createPortal(
    <div
      className="modal-scrim"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        className={`modal ${wide ? 'modal-wide' : ''}`}
        role="dialog"
        aria-modal="true"
        aria-labelledby="modal-title"
        aria-describedby={description ? 'modal-description' : undefined}
        ref={panel}
        tabIndex={-1}
      >
        {titlebar ? (
          <WindowTitlebar {...titlebar} title={title} titleId="modal-title" onClose={onClose} />
        ) : (
          <div className="modal-heading">
            <div>
              <h2 id="modal-title">{title}</h2>
            </div>
            {showCloseButton && (
              <button className="icon-button" aria-label={t('close_panel')} onClick={onClose}>
                <Icon name="close" />
              </button>
            )}
          </div>
        )}
        {description && (
          <p className="modal-description" id="modal-description">
            {description}
          </p>
        )}
        {children}
      </div>
    </div>,
    document.body,
  );
}
