import { Icon } from './Icon';
import type { ReactNode } from 'react';

export function WindowTitlebar({
  onDrag,
  onMinimize,
  onToggleMaximized,
  onClose,
  search,
  listening,
  listeningDisabled,
  listeningLabel,
  onToggleListening,
  blocked = false,
}: {
  onDrag: () => void;
  onMinimize: () => void;
  onToggleMaximized: () => void;
  onClose: () => void;
  search: ReactNode;
  listening: boolean;
  listeningDisabled: boolean;
  listeningLabel: string;
  onToggleListening: () => void;
  blocked?: boolean;
}) {
  return (
    <div
      className="window-titlebar"
      aria-label="窗口标题栏"
      onMouseDown={(event) => {
        if (event.button !== 0 || (event.target as Element).closest('button, input, .search-box'))
          return;
        event.preventDefault();
        if (event.detail === 2) onToggleMaximized();
        else onDrag();
      }}
    >
      <div className="window-titlebar-brand" title="拖动窗口">
        <Icon name="copy" size={16} />
      </div>
      <button
        className={`titlebar-listening ${listening ? 'active' : ''}`}
        aria-label={listeningLabel}
        title={listeningLabel}
        aria-pressed={listening}
        disabled={listeningDisabled || blocked}
        onClick={onToggleListening}
      >
        <span className="listening-dot" />
      </button>
      <div className="titlebar-search" inert={blocked ? true : undefined}>
        {search}
      </div>
      <div className="window-controls">
        <button aria-label="最小化窗口" title="最小化" onClick={onMinimize}>
          <Icon name="minimize" size={14} />
        </button>
        <button aria-label="最大化或还原窗口" title="最大化或还原" onClick={onToggleMaximized}>
          <Icon name="maximize" size={14} />
        </button>
        <button className="window-close" aria-label="关闭窗口" title="关闭" onClick={onClose}>
          <Icon name="close" size={16} />
        </button>
      </div>
    </div>
  );
}
