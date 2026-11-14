import { t } from '../i18n';
import { Component, type ReactNode, type RefObject } from 'react';

type Anchor = { top: boolean; rows: { id: string; offset: number }[] };
type Snapshot = { reset: true } | Anchor | null;
type Props = {
  children: ReactNode;
  listRef: RefObject<HTMLDivElement | null>;
  resetKey: string;
  className: string;
  pending: boolean;
};

// Capture before React mutates the rows, so prepends and mixed row heights preserve
// the record being read. Late image/viewport resizes use the same saved anchor.
export class HistoryViewport extends Component<Props, object, Snapshot> {
  private node: HTMLDivElement | null = null;
  private anchor: Anchor | null = null;
  private observer: ResizeObserver | null = null;
  private sizes = '';
  private restoredTop: number | null = null;

  private rows = () => Array.from(this.node?.querySelectorAll<HTMLElement>('[data-clip-id]') ?? []);
  private visible = () => !!this.node?.getClientRects().length;
  private capture = (): Anchor | null => {
    if (!this.node || !this.visible()) return null;
    const edge = this.node.getBoundingClientRect().top;
    const rows = this.rows().map((row) => ({
      id: row.dataset.clipId!,
      offset: row.getBoundingClientRect().top - edge,
      bottom: row.getBoundingClientRect().bottom - edge,
    }));
    if (!rows.length) return null;
    const first = Math.max(
      0,
      rows.findIndex((row) => row.bottom > 0),
    );
    return {
      top: this.node.scrollTop <= 1,
      rows: [...rows.slice(first), ...rows.slice(0, first).reverse()],
    };
  };
  private restore = (anchor: Anchor | null) => {
    if (!this.node || !this.visible()) {
      // The pre-mutation snapshot can be newer than the last scroll event.
      // Keep it even when this commit hides the list to show a preview.
      this.anchor = anchor;
      return;
    }
    const previousTop = this.node.scrollTop;
    if (anchor?.top) this.node.scrollTop = 0;
    else if (anchor) {
      const current = new Map(this.rows().map((row) => [row.dataset.clipId, row]));
      for (const saved of anchor.rows) {
        const row = current.get(saved.id);
        if (!row) continue;
        this.node.scrollTop +=
          row.getBoundingClientRect().top - this.node.getBoundingClientRect().top - saved.offset;
        break;
      }
    }
    if (previousTop !== this.node.scrollTop) this.restoredTop = this.node.scrollTop;
    this.anchor = this.capture() ?? anchor;
  };
  private remember = () => {
    if (
      this.node &&
      this.restoredTop !== null &&
      Math.abs(this.node.scrollTop - this.restoredTop) < 1
    ) {
      this.restoredTop = null;
      return;
    }
    this.restoredTop = null;
    this.anchor = this.capture() ?? this.anchor;
  };
  private dimensions = () =>
    JSON.stringify([
      this.node?.clientWidth,
      this.node?.clientHeight,
      ...this.rows().map((row) => [
        row.dataset.clipId,
        row.getBoundingClientRect().width,
        row.getBoundingClientRect().height,
      ]),
    ]);
  private observe = () => {
    this.observer?.disconnect();
    if (!this.node || !this.visible()) return;
    this.sizes = this.dimensions();
    this.observer?.observe(this.node);
    for (const row of this.rows()) this.observer?.observe(row);
  };

  componentDidMount() {
    this.remember();
    this.observer = new ResizeObserver(() => {
      if (!this.visible()) return;
      const sizes = this.dimensions();
      if (sizes === this.sizes) return;
      this.sizes = sizes;
      this.restore(this.anchor);
    });
    this.observe();
  }
  getSnapshotBeforeUpdate(previous: Props): Snapshot {
    return previous.resetKey !== this.props.resetKey ? { reset: true } : this.capture();
  }
  componentDidUpdate(_previous: Props, _state: object, snapshot: Snapshot) {
    if (snapshot && 'reset' in snapshot) {
      if (this.node) this.node.scrollTop = 0;
      this.restoredTop = null;
      this.anchor = null;
      this.remember();
    } else this.restore(snapshot ?? this.anchor);
    this.observe();
  }
  componentWillUnmount() {
    this.observer?.disconnect();
  }
  private setRef = (node: HTMLDivElement | null) => {
    this.node = node;
    this.props.listRef.current = node;
  };
  render() {
    return (
      <div
        ref={this.setRef}
        className={this.props.className}
        tabIndex={0}
        aria-label={t('history_list_arrow_keys_select_space_previews')}
        aria-busy={this.props.pending}
        onScroll={this.remember}
      >
        {this.props.children}
      </div>
    );
  }
}
