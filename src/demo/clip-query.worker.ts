import { queryClips, type Clip, type ClipType, type View } from './clips';

// User-supplied regular expressions must never execute on the WebView's UI thread.
self.onmessage = (
  event: MessageEvent<{
    clips: Clip[];
    view: View;
    type: ClipType | 'all';
    query: string;
    oldest: boolean;
  }>,
) => {
  const { clips, view, type, query, oldest } = event.data;
  self.postMessage(queryClips(clips, view, type, query, true, oldest));
};
