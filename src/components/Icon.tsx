import type { CSSProperties } from 'react';

export type IconName =
  | 'search'
  | 'history'
  | 'pin'
  | 'trash'
  | 'folder'
  | 'plus'
  | 'chevron'
  | 'settings'
  | 'moon'
  | 'sun'
  | 'monitor'
  | 'text'
  | 'link'
  | 'image'
  | 'file'
  | 'grid'
  | 'list'
  | 'arrow'
  | 'copy'
  | 'more'
  | 'close'
  | 'check'
  | 'info'
  | 'keyboard'
  | 'command'
  | 'sort'
  | 'back'
  | 'restore'
  | 'shield'
  | 'external';
const paths: Record<IconName, React.ReactNode> = {
  search: (
    <>
      <circle cx="10.7" cy="10.7" r="6.7" />
      <path d="m16 16 4.2 4.2" />
    </>
  ),
  history: (
    <>
      <path d="M3 10a9 9 0 1 1 1.7 7M3 4v6h6" />
      <path d="M12 7v5l3 2" />
    </>
  ),
  pin: (
    <>
      <path d="m15 3 6 6-3 1-4 5v3l-3-3-6-6h3l5-4 2-2Z" />
      <path d="m9 15-6 6" />
    </>
  ),
  trash: (
    <>
      <path d="M3 6h18M9 6V3h6v3M5 6l1 15h12l1-15M10 10v7M14 10v7" />
    </>
  ),
  folder: <path d="M3 7V5h6l2 2h10v13H3V7Z" />,
  plus: <path d="M12 5v14M5 12h14" />,
  chevron: <path d="m8 10 4 4 4-4" />,
  settings: (
    <>
      <path
        d="m9 4 1-2h4l1 2 3 2 2 .2 2 3.5-1 1.8v3l1 1.8-2 3.5-2 .2-3 2-1 2h-4l-1-2-3-2-2-.2-2-3.5 1-1.8v-3L2 9.5 4 6l2-.2L9 4Z"
        transform="translate(1 0) scale(.91)"
      />
      <circle cx="12" cy="12" r="3" />
    </>
  ),
  moon: <path d="M20.8 13.1A9 9 0 0 1 10.9 3.2a9 9 0 1 0 9.9 9.9Z" />,
  sun: (
    <>
      <circle cx="12" cy="12" r="4" />
      <path d="M12 2v2M12 20v2M2 12h2M20 12h2M5 5l1.5 1.5M17.5 17.5 19 19M5 19l1.5-1.5M17.5 6.5 19 5" />
    </>
  ),
  monitor: (
    <>
      <rect x="3" y="4" width="18" height="13" rx="2" />
      <path d="M8 21h8M12 17v4" />
    </>
  ),
  text: (
    <>
      <path d="M5 5h14M12 5v15M8 20h8" />
    </>
  ),
  link: (
    <>
      <path d="m10 7 3-3a5 5 0 0 1 7 7l-3 3M14 17l-3 3a5 5 0 0 1-7-7l3-3M8 16l8-8" />
    </>
  ),
  image: (
    <>
      <rect x="3" y="3" width="18" height="18" rx="3" />
      <circle cx="8" cy="8" r="1.5" />
      <path d="m3 17 5-5 4 4 4-6 5 7" />
    </>
  ),
  file: (
    <>
      <path d="M14 3H5v18h14V8l-5-5ZM14 3v6h5M8 13h8M8 17h5" />
    </>
  ),
  grid: (
    <>
      <rect x="3" y="3" width="7" height="7" rx="1" />
      <rect x="14" y="3" width="7" height="7" rx="1" />
      <rect x="3" y="14" width="7" height="7" rx="1" />
      <rect x="14" y="14" width="7" height="7" rx="1" />
    </>
  ),
  list: (
    <>
      <path d="M8 5h13M8 12h13M8 19h13M3 5h.01M3 12h.01M3 19h.01" />
    </>
  ),
  arrow: (
    <>
      <path d="M5 12h14m-5-5 5 5-5 5" />
    </>
  ),
  copy: (
    <>
      <rect x="8" y="8" width="12" height="13" rx="2" />
      <path d="M16 8V3H3v13h5" />
    </>
  ),
  more: (
    <>
      <circle cx="5" cy="12" r="1" />
      <circle cx="12" cy="12" r="1" />
      <circle cx="19" cy="12" r="1" />
    </>
  ),
  close: <path d="m6 6 12 12M6 18 18 6" />,
  check: <path d="m5 12 4 4L19 6" />,
  info: (
    <>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 11v6M12 7h.01" />
    </>
  ),
  keyboard: (
    <>
      <rect x="2" y="5" width="20" height="14" rx="3" />
      <path d="M6 9h.01M10 9h.01M14 9h.01M18 9h.01M6 13h.01M10 13h.01M14 13h.01M18 13h.01M8 16h8" />
    </>
  ),
  command: <path d="M9 9V5a3 3 0 1 0-3 3h12a3 3 0 1 0-3-3v13a3 3 0 1 0 3-3H6a3 3 0 1 0 3 3V9Z" />,
  sort: (
    <>
      <path d="M8 4v16m-4-4 4 4 4-4M15 5h6M15 10h4M15 15h2" />
    </>
  ),
  back: <path d="m14 5-7 7 7 7" />,
  restore: (
    <>
      <path d="M3 10a9 9 0 1 1 2 8M3 4v6h6M12 8v5l3 2" />
    </>
  ),
  shield: (
    <>
      <path d="m12 3 8 3v6c0 5-8 9-8 9s-8-4-8-9V6l8-3Z" />
      <path d="m8 12 3 3 5-6" />
    </>
  ),
  external: (
    <>
      <path d="M14 3h7v7M21 3l-11 11M10 3H3v18h18v-7" />
    </>
  ),
};
export function Icon({
  name,
  size = 18,
  className = '',
  style,
}: {
  name: IconName;
  size?: number;
  className?: string;
  style?: CSSProperties;
}) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className={className}
      style={style}
    >
      {paths[name]}
    </svg>
  );
}
export function BrandMark() {
  return (
    <svg viewBox="0 0 32 32" width="30" height="30" fill="none" aria-hidden="true">
      <rect x="1" y="1" width="30" height="30" rx="9" fill="currentColor" opacity=".14" />
      <path
        d="M20.8 10.5a8 8 0 1 0 0 11"
        stroke="currentColor"
        strokeWidth="2.7"
        strokeLinecap="round"
      />
      <path
        d="M16.8 14.5h6v6"
        stroke="currentColor"
        strokeWidth="2.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}
