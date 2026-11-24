// Optional Chrome/Playwright adapter checks. Native window behavior is verified separately.
import assert from 'node:assert/strict';
import { mkdir, writeFile, rm } from 'node:fs/promises';
const argument = (name, fallback) => {
  const index = process.argv.indexOf(name);
  return index < 0 ? fallback : process.argv[index + 1];
};
const { chromium } = await import(argument('--playwright-module', 'playwright'));
const browser = await chromium.launch({
  headless: true,
  ...(process.argv.includes('--chrome') ? { channel: 'chrome' } : {}),
});
const url = argument('--url', 'http://localhost:1420');
const base = 'docs/previews/detached-preview-fixture';
await mkdir('docs/previews', { recursive: true });
try {
  await writeFile(
    `${base}.html`,
    '<div id="root"></div><script type="module" src="/docs/previews/detached-preview-fixture.tsx"></script>',
  );
  await writeFile(
    `${base}.tsx`,
    `
import { StrictMode, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';
import { mockIPC } from '@tauri-apps/api/mocks';
import { emit } from '@tauri-apps/api/event';
import '/src/style.css';
globalThis.isTauri = true;
const id = '10000000-0000-4000-8000-000000000001';
const secondId = '10000000-0000-4000-8000-000000000002';
window.fixture = { calls: [], requests: [], mode: 'deferred', language: 'en', theme: 'light', emit, id,
 activeToken: undefined,
 async closePreview() {
   const token = this.activeToken;
   this.activeToken = undefined;
   await emit('hover-preview-closed', token);
 },
 current: { token: crypto.randomUUID(), id, kind: 'text', source: 'Fixture source', savedAt: '1794647289000', size: null, anchor: {x:0,y:20,width:600,height:56} } };
mockIPC((command, args) => {
 fixture.calls.push({command, args});
 if(command === 'bootstrap') return { contractVersion:3, appVersion:'test', dataProfile:'test', schemaVersion:13, settings:{theme:fixture.theme,language:fixture.language,showSearchByDefault:true,revision:'1'}, platform:{os:'macos',sessionHint:'native',capabilities:[]} };
 if(command === 'get_hover_preview') return fixture.current;
 if(command === 'clip_detail') {
   if(fixture.mode === 'error') return Promise.reject({code:'not_found', message:'secret'});
   return new Promise((resolve) => fixture.requests.push({ id:args.id, resolve }));
 }
 if(command === 'show_hover_preview') {
   if(args.previousToken && args.previousToken !== fixture.activeToken) return false;
   fixture.activeToken = args.request.token;
   return true;
 }
 if(command === 'hide_hover_preview') {
   if(args.token === fixture.activeToken) return fixture.closePreview();
   return;
 }
 if(command === 'hover_preview_presence') return;
 throw new Error('Unexpected command: ' + command);
}, {shouldMockEvents:true});
const { HoverPreview } = await import('/src/components/HoverPreview');
const { HoverPreviewWindow } = await import('/src/components/HoverPreviewWindow');
function Controller() {
 const list = useRef(null);
 const [enabled,setEnabled]=useState(true);
 fixture.setEnabled=setEnabled;
 const clip={ id, revision:'1', type:'text', title:'Fixture', content:'PRIVATE SUMMARY', source:'Fixture', time:'17:08',day:'today', savedAt:'1794647289000',pinned:false,collection:null };
 const clips = [clip, { ...clip, id: secondId }];
 return <><div className="history-scroll" style={{height:500}} ref={list}>{clips.map((item) => <article key={item.id} className="clip-row" data-clip-id={item.id}><button className="row-main">Fixture row</button></article>)}</div>{enabled && <HoverPreview listRef={list} clips={clips} native onError={(error)=>{fixture.error=error;}} />}</>;
}
createRoot(document.getElementById('root')).render(<StrictMode>{new URLSearchParams(location.search).get('mode')==='controller' ? <Controller/> : <HoverPreviewWindow/>}</StrictMode>);
`,
  );
  const page = await browser.newPage({ viewport: { width: 640, height: 650 } });
  page.setDefaultTimeout(8000);
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.clock.install();
  await page.goto(`${url}/${base}.html?mode=controller`);
  const row = page.locator('.row-main').first();
  const otherRow = page.locator('.row-main').last();
  const shows = () =>
    page.evaluate(() => fixture.calls.filter((call) => call.command === 'show_hover_preview'));
  const showAfterDelay = async (anchor) => {
    const count = (await shows()).length;
    await anchor.hover();
    await page.clock.runFor(2000);
    assert.equal((await shows()).length, count);
    await page.clock.runFor(550);
    const calls = await shows();
    assert.equal(calls.length, count + 1);
    assert.equal(calls.at(-1).args.previousToken, undefined);
    return calls.at(-1).args.request;
  };
  const request = await showAfterDelay(row);
  assert.equal(await page.locator('.hover-preview').count(), 0);
  assert.equal(request.id, await page.evaluate(() => fixture.id));
  assert.equal(request.anchor.height, 54);
  assert.doesNotMatch(JSON.stringify(request), /PRIVATE SUMMARY/);
  await otherRow.hover();
  await page.clock.runFor(180);
  const switched = await shows();
  assert.equal(switched.length, 2);
  assert.equal(switched[1].args.previousToken, request.token);
  assert.notEqual(switched[1].args.request.id, request.id);

  await page.evaluate(() => fixture.closePreview());
  await showAfterDelay(row);

  // Simulate closure reaching the backend before the main WebView receives its event.
  await page.evaluate(() => {
    fixture.activeToken = undefined;
  });
  await otherRow.hover();
  await page.clock.runFor(180);
  assert.equal((await shows()).length, 4);
  assert.ok((await shows())[3].args.previousToken);
  await page.clock.runFor(1800);
  assert.equal((await shows()).length, 4);
  await page.clock.runFor(570);
  assert.equal((await shows()).length, 5);
  assert.equal((await shows())[4].args.previousToken, undefined);

  await page.mouse.move(635, 620);
  await page.clock.runFor(500);
  await page.waitForFunction(() =>
    fixture.calls.some((call) => call.command === 'hide_hover_preview' && call.args.unlessHovered),
  );
  await page.evaluate(() => {
    fixture.calls = [];
  });
  await showAfterDelay(row);
  await page.keyboard.press('Escape');
  await page.waitForFunction(() =>
    fixture.calls.some((c) => c.command === 'hide_hover_preview' && !c.args.unlessHovered),
  );
  await page.evaluate(() => {
    fixture.calls = [];
  });
  await page.mouse.move(635, 620);
  await row.hover();
  await page.clock.runFor(1200);
  await page.mouse.move(635, 620);
  await page.clock.runFor(1500);
  assert.equal((await shows()).length, 0);
  await showAfterDelay(row);
  await page.evaluate(() => fixture.setEnabled(false));
  await page.waitForFunction(() =>
    fixture.calls.some((c) => c.command === 'hide_hover_preview' && !c.args.unlessHovered),
  );

  await page.goto(`${url}/${base}.html?window=hover-preview`);
  await page.locator('.native-hover-preview').waitFor();
  assert.equal(await page.locator('.native-preview-header button').count(), 0);
  await page.locator('.native-preview-footer time').waitFor();
  await page.waitForFunction(() => fixture.requests.length > 0);
  assert.match(await page.locator('.native-preview-scroll').innerText(), /Loading full content/);
  await page.evaluate(async () => {
    fixture.current = {
      ...fixture.current,
      token: crypto.randomUUID(),
      id: '10000000-0000-4000-8000-000000000002',
    };
    await fixture.emit('hover-preview-changed');
  });
  await page.waitForFunction(() => fixture.requests.some((r) => r.id.endsWith('2')));
  await page.evaluate(() => {
    fixture.requests.forEach((r) =>
      r.resolve({
        id: r.id,
        text: r.id.endsWith('2')
          ? '<script>literal</script>\n' + 'Complete text\n'.repeat(2000)
          : 'OLD CONTENT',
        imageDataUrl: null,
      }),
    );
  });
  await page.locator('.native-preview-scroll pre').waitFor();
  assert.match(
    await page.locator('.native-preview-scroll pre').innerText(),
    /^<script>literal<\/script>/,
  );
  assert.doesNotMatch(await page.locator('.native-preview-scroll').innerText(), /OLD CONTENT/);
  assert.equal(await page.locator('.native-preview-scroll script').count(), 0);
  await page.locator('.native-preview-scroll').hover();
  await page.mouse.wheel(0, 400);
  await page.waitForTimeout(100);
  assert(await page.locator('.native-preview-scroll').evaluate((node) => node.scrollTop > 0));
  assert.equal(await page.locator('.app-shell').count(), 0);
  assert.equal(await page.locator('.hover-preview').count(), 0);
  assert.equal(
    await page.evaluate(() => fixture.calls.some((c) => c.command === 'set_ui_language')),
    false,
  );
  await page.screenshot({ path: 'docs/previews/detached-preview-light.png' });
  await page.evaluate(async () => {
    fixture.mode = 'error';
    fixture.current = { ...fixture.current, token: crypto.randomUUID() };
    await fixture.emit('hover-preview-changed');
  });
  await page.locator('.native-preview-scroll [role="alert"]').waitFor();
  assert.doesNotMatch(await page.locator('.native-preview-scroll').innerText(), /secret/);
  await page.evaluate(() => {
    fixture.mode = 'deferred';
  });
  await page.getByRole('button', { name: 'Retry', exact: true }).click();
  await page.locator('.native-preview-scroll [role="alert"]').waitFor({ timeout: 7000 });
  assert.match(await page.locator('.native-preview-scroll').innerText(), /timed out/);
  await page.evaluate(() =>
    fixture.requests
      .at(-1)
      .resolve({ id: fixture.current.id, text: 'TOO LATE', imageDataUrl: null }),
  );
  await page.waitForTimeout(100);
  assert.doesNotMatch(await page.locator('.native-preview-scroll').innerText(), /TOO LATE/);
  const beforeRetry = await page.evaluate(() => fixture.requests.length);
  await page.getByRole('button', { name: 'Retry', exact: true }).click();
  await page.waitForFunction((count) => fixture.requests.length > count, beforeRetry);
  await page.evaluate(() =>
    fixture.requests
      .at(-1)
      .resolve({ id: fixture.current.id, text: 'Retry succeeded', imageDataUrl: null }),
  );
  await page.locator('.native-preview-scroll pre').waitFor();
  assert.equal(await page.locator('.native-preview-scroll pre').innerText(), 'Retry succeeded');
  await page.evaluate(async () => {
    fixture.beforeImage = fixture.requests.length;
    fixture.language = 'zh-CN';
    fixture.theme = 'dark';
    await fixture.emit('settings-changed');
    fixture.current = {
      ...fixture.current,
      token: crypto.randomUUID(),
      kind: 'image',
      size: '1 x 1',
    };
    await fixture.emit('hover-preview-changed');
  });
  await page.waitForFunction(() => document.documentElement.lang === 'zh-CN');
  await page.waitForFunction(() => fixture.requests.length > fixture.beforeImage);
  await page.evaluate(() =>
    fixture.requests.at(-1).resolve({
      id: fixture.current.id,
      text: '',
      imageDataUrl:
        'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLttAAAAABJRU5ErkJggg==',
    }),
  );
  await page.locator('.native-preview-image').waitFor();
  await page.screenshot({ path: 'docs/previews/detached-preview-dark.png' });
  await page.keyboard.press('Escape');
  await page.waitForFunction(() => fixture.calls.some((c) => c.command === 'hide_hover_preview'));
  assert.deepEqual(errors, []);
  console.log(
    'Detached preview adapters passed: delayed opening, fast switching, dismissal, metadata-only requests, standalone layout, full/stale text, safe errors, timeout/retry, image, themes/languages and Escape. IPC mocked; OS windows require native acceptance.',
  );
  await page.close();
} finally {
  await browser.close();
  await Promise.all([rm(`${base}.html`, { force: true }), rm(`${base}.tsx`, { force: true })]);
}
