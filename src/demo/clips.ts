import alpineStudy from '../assets/alpine-study.svg';

export type ClipType = 'text' | 'link' | 'image' | 'file';
export type Collection = 'work' | 'inspiration' | 'later';
export type View = 'all' | 'pinned' | 'trash' | Collection;
export interface Clip {
  id: string;
  type: ClipType;
  title: string;
  content: string;
  source: string;
  time: string;
  day: '今天' | '昨天';
  pinned: boolean;
  collection: Collection | null;
  deleted?: boolean;
  image?: string;
  language?: string;
  size?: string;
}
export const collectionNames: Record<Collection, string> = {
  work: '工作',
  inspiration: '灵感',
  later: '稍后再看',
};
export const typeNames: Record<ClipType, string> = {
  text: '文本',
  link: '链接',
  image: '图片',
  file: '文件',
};
export const initialClips: Clip[] = [
  {
    id: 'note-1',
    type: 'text',
    title: '让灵感，随手可得。',
    source: '备忘录',
    time: '10:42',
    day: '今天',
    pinned: true,
    collection: 'work',
    content:
      '好的工具，让注意力回到事情本身。\n\n一个念头、一段文字、一张刚好用得上的图片。把工作里的细小片刻收在一起，需要时，随手找回。\n\n关于 UClip 的下一版\n\n01  轻一点\n打开、搜索、复制，动作之间不需要等待。\n\n02  清楚一点\n每一条内容都有自己的位置，每一次操作都有明确的回应。\n\n03  安静一点\n留住有用的信息，把空间还给正在做的事。',
  },
  {
    id: 'link-1',
    type: 'link',
    title: 'Linear — Plan and build products',
    content: 'https://linear.app/features',
    source: '浏览器',
    time: '10:38',
    day: '今天',
    pinned: false,
    collection: 'inspiration',
  },
  {
    id: 'image-1',
    type: 'image',
    title: 'Somewhere, slower.',
    content: '一份关于山野、留白和慢节奏的视觉练习。',
    image: alpineStudy,
    source: '设计工具',
    time: '10:26',
    day: '今天',
    pinned: false,
    collection: 'inspiration',
    size: '1200 × 780',
  },
  {
    id: 'code-1',
    type: 'text',
    title: '一个简单的防抖函数',
    content:
      'function debounce<T extends (...args: never[]) => void>(\n  callback: T,\n  delay = 200,\n) {\n  let timer: ReturnType<typeof setTimeout>;\n\n  return (...args: Parameters<T>) => {\n    clearTimeout(timer);\n    timer = setTimeout(() => callback(...args), delay);\n  };\n}',
    source: '代码编辑器',
    time: '10:12',
    day: '今天',
    pinned: true,
    collection: 'work',
    language: 'TypeScript',
  },
  {
    id: 'text-2',
    type: 'text',
    title: '周五产品评审',
    content:
      '周五 14:30 · 产品与设计评审\n\n— 确认第一版交互流程\n— 走查深色与浅色主题\n— 验证搜索和键盘操作\n— 整理下一轮反馈',
    source: '团队文档',
    time: '09:56',
    day: '今天',
    pinned: false,
    collection: 'work',
  },
  {
    id: 'file-1',
    type: 'file',
    title: 'brand-assets.zip',
    content: '/example/design/brand-assets.zip',
    source: '文件管理器',
    time: '09:41',
    day: '今天',
    pinned: false,
    collection: 'work',
    size: '4.8 MB',
  },
  {
    id: 'text-3',
    type: 'text',
    title: '好设计的留白',
    content:
      '设计不是添加更多，而是留下恰好的东西。\n\n把信息放在触手可及的位置，把不需要的东西留在视线之外。',
    source: '阅读笔记',
    time: '09:24',
    day: '今天',
    pinned: false,
    collection: 'inspiration',
  },
  {
    id: 'link-2',
    type: 'link',
    title: 'The beauty of building small',
    content: 'https://github.com/collections/design-essentials',
    source: '浏览器',
    time: '18:32',
    day: '昨天',
    pinned: true,
    collection: 'later',
  },
  {
    id: 'text-4',
    type: 'text',
    title: '一抹山野的颜色',
    content: '#B5CDA3\n\nMoss / 苔序\n带一点温度的鼠尾草绿。',
    source: '设计工具',
    time: '17:45',
    day: '昨天',
    pinned: false,
    collection: 'inspiration',
  },
  {
    id: 'text-5',
    type: 'text',
    title: '明天的第一件小事',
    content: '先做好一件小事，再开始下一件。\n\n关掉不需要的标签页，留一点时间给自己。',
    source: '备忘录',
    time: '16:08',
    day: '昨天',
    pinned: false,
    collection: 'later',
  },
  {
    id: 'link-3',
    type: 'link',
    title: 'Tauri — Create small, fast applications',
    content: 'https://v2.tauri.app/',
    source: '浏览器',
    time: '15:20',
    day: '昨天',
    pinned: false,
    collection: 'later',
  },
  {
    id: 'file-2',
    type: 'file',
    title: '交互方案_v2.fig',
    content: '/example/design/交互方案_v2.fig',
    source: '文件管理器',
    time: '14:02',
    day: '昨天',
    pinned: false,
    collection: 'later',
    size: '12.4 MB',
  },
];

export function queryClips(
  clips: Clip[],
  view: View,
  type: ClipType | 'all',
  query: string,
  regex: boolean,
  oldest: boolean,
) {
  let matcher: (clip: Clip) => boolean;
  try {
    const expression = regex ? new RegExp(query, 'iu') : null;
    const needle = query.toLocaleLowerCase();
    matcher = (clip) => {
      const haystack = `${clip.title}\n${clip.content}\n${clip.source}`;
      return expression ? expression.test(haystack) : haystack.toLocaleLowerCase().includes(needle);
    };
  } catch {
    return { items: [], error: '正则表达式无效，请检查后重试。' };
  }
  const items = clips.filter((clip) => {
    if (view === 'trash' ? !clip.deleted : clip.deleted) return false;
    if (view === 'pinned' && !clip.pinned) return false;
    if (['work', 'inspiration', 'later'].includes(view) && clip.collection !== view) return false;
    return (type === 'all' || clip.type === type) && matcher(clip);
  });
  return { items: oldest ? [...items].reverse() : items, error: '' };
}
