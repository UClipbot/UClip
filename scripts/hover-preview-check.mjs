// Optional browser checks; use a running Vite server and an external Playwright install.
import assert from 'node:assert/strict';
import { mkdir } from 'node:fs/promises';
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
await mkdir('docs/previews', { recursive: true });
const errors = [];
const track = (page) => page.on('pageerror', (error) => errors.push(error.message));
const hidden = (page) => page.locator('.hover-preview').waitFor({ state: 'detached' });
const hover = async (page, id) => {
  const row = page.locator(`[data-clip-id="${id}"] .row-main`);
  await page.mouse.move(page.viewportSize().width - 2, 2);
  await row.scrollIntoViewIfNeeded();
  await page.waitForTimeout(100);
  await row.hover();
  await page.locator('.hover-preview').waitFor();
};
try {
  const context = await browser.newContext({ locale: 'en-US', reducedMotion: 'reduce' });
  const page = await context.newPage();
  track(page);
  page.setDefaultTimeout(8000);
  for (const [width, height, theme, language] of [
    [640, 650, 'dark', 'en'],
    [375, 520, 'light', 'zh-CN'],
    [1200, 800, 'light', 'en'],
    [420, 360, 'dark', 'zh-CN'],
  ]) {
    await page.setViewportSize({ width, height });
    await page.goto(url);
    await page.evaluate(
      ({ theme, language }) => {
        localStorage.setItem('uclip-preview-theme', theme);
        localStorage.setItem('uclip-preview-language', language);
      },
      { theme, language },
    );
    await page.reload();
    await page.locator('.row-main').first().waitFor();
    await page.locator('#history-search').focus();
    await hover(page, 'note-1');
    assert.equal(await page.evaluate(() => document.activeElement.id), 'history-search');
    assert.match(
      await page.locator('.hover-preview-footer').innerText(),
      /Modified|\u4fee\u6539\u65f6\u95f4/,
    );
    const box = await page.locator('.hover-preview').boundingBox();
    assert(
      box.x >= 7 &&
        box.y >= 7 &&
        box.x + box.width <= width - 7 &&
        box.y + box.height <= height - 7,
    );
    await page.locator('.hover-preview-content').hover();
    await page.mouse.wheel(0, 160);
    await page.waitForTimeout(250);
    assert(await page.locator('.hover-preview').isVisible());
    await page.screenshot({ path: `docs/previews/hover-${width}-${theme}.png` });
    await page.keyboard.press('Escape');
    await hidden(page);
    assert.equal(
      await page.locator('[data-clip-id="note-1"] .row-main').getAttribute('aria-describedby'),
      null,
    );
    await hover(page, 'image-1');
    assert(await page.locator('.hover-preview img').isVisible());
    await page.mouse.move(width - 3, 2);
    await hidden(page);
    await hover(page, 'file-1');
    assert.match(
      await page.locator('.hover-preview pre').innerText(),
      /\/example\/design\/brand-assets.zip/,
    );
    await page.keyboard.press('Escape');
    await hidden(page);
  }
  await page.setViewportSize({ width: 640, height: 650 });
  await page.locator('#history-search').fill('debounce');
  await page.waitForTimeout(150);
  await hover(page, 'code-1');
  await page.keyboard.press('Escape');
  await hidden(page);
  assert.equal(await page.locator('#history-search').inputValue(), 'debounce');
  await hover(page, 'code-1');
  await page.locator('#history-search').fill('no-matching-record');
  await hidden(page);
  await context.close();

  assert.deepEqual(errors, []);
  console.log(
    'Browser fallback hover previews passed: 4 viewports, both themes/languages, text/image/files, focus, dismissal and scrolling.',
  );
} finally {
  await browser.close();
}
