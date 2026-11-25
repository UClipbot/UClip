import { t } from '../i18n';
import { Icon } from './Icon';

export function WindowTitlebar({
  title = 'UClip',
  titleId,
  onDrag,
  onMinimize,
  onToggleMaximized,
  onClose,
}: {
  title?: string;
  titleId?: string;
  onDrag: () => void;
  onMinimize?: () => void;
  onToggleMaximized: () => void;
  onClose: () => void;
}) {
  return (
    <div
      className="window-titlebar"
      aria-label={t('window_title_bar')}
      onMouseDown={(event) => {
        if (event.button !== 0 || (event.target as Element).closest('button')) return;
        event.preventDefault();
        if (event.detail === 2) onToggleMaximized();
        else onDrag();
      }}
    >
      <div className="window-titlebar-brand" title={t('drag_window')}>
        <Icon name="copy" size={16} />
        {titleId ? <h2 id={titleId}>{title}</h2> : <span>{title}</span>}
      </div>
      <div className="window-controls">
        {onMinimize && (
          <button aria-label={t('minimize_window')} title={t('minimize')} onClick={onMinimize}>
            <Icon name="minimize" size={14} />
          </button>
        )}
        <button
          aria-label={t('maximize_or_restore_window')}
          title={t('maximize_or_restore')}
          onClick={onToggleMaximized}
        >
          <Icon name="maximize" size={14} />
        </button>
        <button
          className="window-close"
          aria-label={t('close_window')}
          title={t('close')}
          onClick={onClose}
        >
          <Icon name="close" size={16} />
        </button>
      </div>
    </div>
  );
}
