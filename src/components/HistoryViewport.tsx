import { t } from '../i18n';
import type { Clip } from '../demo/clips';
import type { HistoryRange } from '../platform/useHistory';
import { Component, type ReactNode, type RefObject } from 'react';

type Props = {
  children: ReactNode;
  listRef: RefObject<HTMLDivElement | null>;
  viewportRef: RefObject<HistoryViewport | null>;
  resetKey: string;
  grid: boolean;
  pending: boolean;
  hidden: boolean;
  total: number;
  revision?: string;
  items: ReadonlyMap<number, Clip>;
  renderRow: (clip: Clip, index: number) => ReactNode;
  onRangeChange?: (range: HistoryRange) => void;
  onVisibleImages?: (ids: readonly string[]) => void;
};
type Metrics = { top: number; height: number; columns: number };
type Anchor = { id: string; index: number; offset: number };
type Snapshot = {
  reset: boolean;
  anchors: Anchor[];
  focus: HTMLElement | null;
  hidingAt: number | null;
};

const OVERSCAN_ROWS = 8;
const MAX_SCROLL_HEIGHT = 8_000_000;

function geometry(total: number, grid: boolean, { top, height, columns }: Metrics) {
  const rowHeight = grid ? 208 : 56;
  const gap = grid ? 8 : 0;
  const stride = rowHeight + gap;
  const lanes = grid ? columns : 1;
  const rows = Math.ceil(total / lanes);
  const fullHeight = rows ? rows * stride - gap : 0;
  const scrollHeight = Math.min(fullHeight, MAX_SCROLL_HEIGHT);
  const physicalRange = Math.max(0, scrollHeight - height);
  const logicalRange = Math.max(0, fullHeight - height);
  // A very large All History tab can exceed the WebView's element dimensions.
  // Map its scroll range proportionally, keeping the visible rows at their real size.
  const ratio = physicalRange ? logicalRange / physicalRange : 1;
  const logicalTop = Math.min(Math.max(0, top), physicalRange) * ratio;
  const firstRow = Math.min(rows, Math.floor(logicalTop / stride));
  const lastRow = Math.min(rows, Math.ceil((logicalTop + Math.max(height, stride)) / stride));
  const firstRendered = Math.max(0, firstRow - OVERSCAN_ROWS);
  return {
    rowHeight,
    gap,
    lanes,
    stride,
    ratio,
    logicalTop,
    scrollHeight,
    firstVisible: firstRow * lanes,
    visibleEnd: Math.min(total, lastRow * lanes),
    start: firstRendered * lanes,
    end: Math.min(total, (lastRow + OVERSCAN_ROWS) * lanes),
    windowTop: firstRendered * stride + logicalTop / ratio - logicalTop,
  };
}

export class HistoryViewport extends Component<Props, Metrics, Snapshot> {
  state: Metrics = { top: 0, height: 0, columns: 1 };
  private node: HTMLDivElement | null = null;
  private observer: ResizeObserver | null = null;
  private hiddenTop: number | null = null;
  private reportedRange: HistoryRange = { start: 0, end: 0 };
  private images: readonly string[] = [];

  private measure = () => {
    const node = this.node;
    if (!node?.clientHeight) return;
    const style = getComputedStyle(node);
    const height = Math.max(
      0,
      node.clientHeight - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom),
    );
    const top = node.scrollTop;
    const columns = window.innerWidth <= 480 ? 1 : 2;
    this.setState((previous) =>
      previous.top === top && previous.height === height && previous.columns === columns
        ? null
        : { top, height, columns },
    );
  };

  private onScroll = () => {
    const top = this.node?.scrollTop;
    if (top !== undefined) this.setState((previous) => (previous.top === top ? null : { top }));
  };

  private layout = () => geometry(this.props.total, this.props.grid, this.state);

  firstVisible() {
    return this.layout().firstVisible;
  }

  isRendered(index: number) {
    const { start, end } = this.layout();
    return index >= start && index < end;
  }

  scrollToIndex(index: number) {
    const node = this.node;
    if (!node) return;
    const { stride, lanes, ratio, logicalTop, rowHeight } = this.layout();
    const start = Math.floor(index / lanes) * stride;
    if (start < logicalTop) node.scrollTop = start / ratio;
    else if (start + rowHeight > logicalTop + this.state.height)
      node.scrollTop = (start + rowHeight - this.state.height) / ratio;
    this.measure();
  }

  private report = () => {
    const visible = !!this.node?.clientHeight;
    const { start, end, firstVisible, visibleEnd, lanes } = this.layout();
    if (visible && (start !== this.reportedRange.start || end !== this.reportedRange.end)) {
      this.reportedRange = { start, end };
      this.props.onRangeChange?.(this.reportedRange);
    }
    if (!this.props.onVisibleImages) return;
    const ids: string[] = [];
    if (visible) {
      for (
        let index = Math.max(0, firstVisible - lanes);
        index < Math.min(end, visibleEnd + lanes);
        index++
      ) {
        const clip = this.props.items.get(index);
        if (clip?.type === 'image') ids.push(clip.id);
      }
    }
    if (ids.length === this.images.length && ids.every((id, index) => id === this.images[index]))
      return;
    this.images = ids;
    this.props.onVisibleImages(ids);
  };

  componentDidMount() {
    this.measure();
    this.report();
    this.observer = new ResizeObserver(this.measure);
    if (this.node) this.observer.observe(this.node);
    window.addEventListener('resize', this.measure);
  }

  getSnapshotBeforeUpdate(previous: Props, metrics: Metrics): Snapshot {
    const reset = previous.resetKey !== this.props.resetKey;
    const focus = document.activeElement;
    const anchors: Anchor[] = [];
    const node = this.node;
    if (!reset && node?.clientHeight && previous.revision !== this.props.revision) {
      const layout = geometry(previous.total, previous.grid, metrics);
      for (let index = layout.firstVisible; index < layout.visibleEnd; index++) {
        const clip = previous.items.get(index);
        if (clip)
          anchors.push({
            id: clip.id,
            index,
            offset: layout.logicalTop - Math.floor(index / layout.lanes) * layout.stride,
          });
      }
    }
    return {
      reset,
      anchors,
      focus: focus instanceof HTMLElement && node?.contains(focus) ? focus : null,
      hidingAt: !previous.hidden && this.props.hidden ? (node?.scrollTop ?? null) : null,
    };
  }

  componentDidUpdate(previous: Props, metrics: Metrics, snapshot: Snapshot) {
    const node = this.node;
    if (!node) return;
    const showing = previous.hidden && !this.props.hidden;
    if (snapshot.reset) {
      this.hiddenTop = null;
      node.scrollTop = 0;
    } else if (snapshot.hidingAt !== null) this.hiddenTop = snapshot.hidingAt;
    else if (showing && this.hiddenTop !== null) {
      node.scrollTop = this.hiddenTop;
      this.hiddenTop = null;
    } else if (
      snapshot.anchors.length &&
      metrics.top > 1 &&
      Math.abs(node.scrollTop - metrics.top) < 1 &&
      node.clientHeight
    ) {
      const layout = this.layout();
      for (const anchor of snapshot.anchors) {
        let index = layout.start;
        while (index < layout.end && this.props.items.get(index)?.id !== anchor.id) index++;
        if (index === layout.end) continue;
        if (index !== anchor.index)
          node.scrollTop =
            (Math.floor(index / layout.lanes) * layout.stride + anchor.offset) / layout.ratio;
        break;
      }
    }
    if (
      snapshot.focus &&
      !snapshot.focus.isConnected &&
      node.clientHeight &&
      document.activeElement === document.body
    )
      node.focus({ preventScroll: true });
    if (snapshot.reset || showing || previous.total !== this.props.total || snapshot.anchors.length)
      this.measure();
    this.report();
  }

  componentWillUnmount() {
    this.observer?.disconnect();
    window.removeEventListener('resize', this.measure);
  }

  private setRef = (node: HTMLDivElement | null) => {
    this.node = node;
    this.props.listRef.current = node;
    this.props.viewportRef.current = node ? this : null;
  };

  render() {
    const { grid, pending, items, renderRow, children, total } = this.props;
    const { start, end, scrollHeight, windowTop, rowHeight, gap, lanes } = this.layout();
    const windowHeight = Math.ceil((end - start) / lanes) * (rowHeight + gap) - gap;
    // Keep the full-height spacer out of WebKit's overflow clipping layers.
    // Only the small row window needs a clip if scaled overscan crosses the end.
    const availableHeight = Math.max(0, scrollHeight - windowTop);
    const clipWindow = windowHeight > availableHeight;
    const rows = [];
    for (let index = start; index < end; index++) {
      const clip = items.get(index);
      rows.push(
        clip ? (
          renderRow(clip, index)
        ) : (
          <div key={`placeholder-${index}`} className="history-placeholder" aria-hidden="true" />
        ),
      );
    }
    return (
      <div
        ref={this.setRef}
        className={`history-scroll ${grid ? 'clip-grid' : ''}`}
        tabIndex={0}
        aria-label={t('history_list_arrow_keys_select_space_previews')}
        aria-busy={pending}
        onScroll={this.onScroll}
      >
        {total ? (
          <div className="history-space" style={{ height: scrollHeight }}>
            <div
              className="history-window"
              style={{
                top: windowTop,
                maxHeight: clipWindow ? availableHeight : undefined,
                overflow: clipWindow ? 'hidden' : undefined,
                gridAutoRows: rowHeight,
                gridTemplateColumns: `repeat(${lanes}, minmax(0, 1fr))`,
                gap,
              }}
            >
              {rows}
            </div>
          </div>
        ) : (
          children
        )}
      </div>
    );
  }
}
