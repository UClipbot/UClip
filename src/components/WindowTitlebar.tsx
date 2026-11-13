import { Icon } from './Icon';

export function WindowTitlebar({
  onDrag,
  onMinimize,
  onToggleMaximized,
  onClose,
}: {
  onDrag: () => void;
  onMinimize: () => void;
  onToggleMaximized: () => void;
  onClose: () => void;
}) {
  return (
    <div className="window-titlebar" aria-label="窗口标题栏">
      <div
        className="window-titlebar-drag"
        onMouseDown={(event) => {
          if (event.button !== 0) return;
          event.preventDefault();
          if (event.detail === 2) onToggleMaximized();
          else onDrag();
        }}
      >
        <Icon name="copy" size={14} />
        <span>UClip</span>
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
