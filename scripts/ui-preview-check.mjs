// Optional browser verification tool; Playwright is not an application dependency.
import assert from 'node:assert/strict';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
const { version } = JSON.parse(readFileSync(new URL('../package.json', import.meta.url), 'utf8'));
const moduleIndex = process.argv.indexOf('--playwright-module');
const urlIndex = process.argv.indexOf('--url');
const appUrl = urlIndex < 0 ? 'http://localhost:1420' : process.argv[urlIndex + 1];
const { chromium } = await import(moduleIndex < 0 ? 'playwright' : process.argv[moduleIndex + 1]);
mkdirSync('docs/previews', { recursive: true });
const browser = await chromium.launch({ headless: true });
const context = await browser.newContext({
  viewport: { width: 640, height: 650 },
  locale: 'zh-CN',
  colorScheme: 'dark',
  reducedMotion: 'reduce',
  permissions: ['clipboard-read', 'clipboard-write'],
});
const page = await context.newPage();
page.setDefaultTimeout(8000);
const failures = [];
page.on('pageerror', (error) => failures.push(error.message));
const appMenu = async (name) => {
  await page.getByRole('button', { name: '更多选项', exact: true }).click();
  await page.getByRole('menuitem', { name, exact: true }).click();
};
const rowMenu = async (id, name) => {
  await page.locator(`[data-clip-id="${id}"]`).hover();
  await page.locator(`[data-clip-id="${id}"] .row-more`).click();
  await page.getByRole('menuitem', { name, exact: true }).click();
};
const copyRow = async (id) => {
  await page.locator(`[data-clip-id="${id}"] [data-row-main]`).click();
  await page.keyboard.press('Control+c');
};
const selected = () => page.locator('.clip-row.selected').getAttribute('data-clip-id');
const rows = page.locator('.clip-row');
const search = page.locator('#history-search');
const clipboard = async () =>
  (await page.evaluate(() => navigator.clipboard.readText())).replace(/\r\n/g, '\n');
const reports = [];
const contrastSamples = [];
const interactions = [];
try {
  let releaseImage;
  const imageGate = new Promise((resolve) => {
    releaseImage = resolve;
  });
  await page.setViewportSize({ width: 640, height: 440 });
  await page.goto(appUrl, { waitUntil: 'domcontentloaded' });
  await rows.first().waitFor();
  assert.equal(await rows.first().evaluate((e) => e.getBoundingClientRect().height), 56);
  const imageRow = page.locator('[data-clip-id="image-1"]');
  assert.equal(await imageRow.evaluate((e) => e.getBoundingClientRect().height), 56);
  const originalImage = await imageRow.locator('img').getAttribute('src');
  const imageBody = Buffer.from(originalImage.split(',')[1], 'base64').toString('utf8');
  await page.route('**/__image_delay.svg', async (route) => {
    await imageGate;
    await route.fulfill({ contentType: 'image/svg+xml', body: imageBody });
  });
  const imageRequest = page.waitForRequest('**/__image_delay.svg');
  await imageRow.locator('img').evaluate((e) => {
    e.src = '/__image_delay.svg';
  });
  await imageRequest;
  const beforeImage = await page.locator('.history-scroll').evaluate((e) => {
    e.scrollTop = 300;
    const row = [...e.querySelectorAll('[data-clip-id]')].find(
      (r) => r.getBoundingClientRect().bottom > e.getBoundingClientRect().top,
    );
    return {
      id: row.dataset.clipId,
      offset: row.getBoundingClientRect().top - e.getBoundingClientRect().top,
    };
  });
  releaseImage();
  await imageRow.locator('img').evaluate((e) => e.decode());
  assert.equal(await imageRow.evaluate((e) => e.getBoundingClientRect().height), 56);
  await page.waitForFunction(({ id, offset }) => {
    const e = document.querySelector('.history-scroll'),
      r = [...e.querySelectorAll('[data-clip-id]')].find((r) => r.dataset.clipId === id);
    return Math.abs(r.getBoundingClientRect().top - e.getBoundingClientRect().top - offset) <= 1;
  }, beforeImage);
  await page.locator('.history-scroll').evaluate((e) => {
    e.scrollTop = 0;
  });
  await page.setViewportSize({ width: 640, height: 650 });
  await search.focus();
  interactions.push('Fixed 56px rows; delayed image decode preserves reserved height and viewport');
  assert.equal(await search.evaluate((e) => e === document.activeElement), true);
  assert.equal(await page.locator('.sidebar, .detail-panel').count(), 0);
  // Select a record before invoking the default action from search.
  await rows.first().locator('[data-row-main]').click();
  await search.press('Enter');
  assert.match(await clipboard(), /好的工具/);
  await copyRow('link-1');
  assert.equal(await clipboard(), 'https://linear.app/features');
  await page.evaluate(() => navigator.clipboard.writeText('double-click sentinel'));
  await page.locator('[data-clip-id="link-1"] [data-row-main]').dblclick();
  assert.equal(await clipboard(), 'https://linear.app/features');
  assert.equal(await selected(), 'link-1');
  await search.focus();
  await search.press('Enter');
  assert.equal(await clipboard(), 'https://linear.app/features');
  await page.evaluate(() => navigator.clipboard.writeText('IME sentinel'));
  await search.evaluate((e) =>
    e.dispatchEvent(
      new KeyboardEvent('keydown', {
        key: 'Enter',
        isComposing: true,
        bubbles: true,
        cancelable: true,
      }),
    ),
  );
  assert.equal(await clipboard(), 'IME sentinel');
  await search.press('Space');
  assert.equal(await search.inputValue(), ' ');
  assert.equal(await page.locator('.detail-panel').count(), 0);
  await search.fill('');
  interactions.push(
    'Direct copy, correct row binding, keyboard selection, IME guard, search space',
  );

  // Preview must preserve the original list node, scroll position, query and trigger focus.
  await search.fill('设计');
  const previewRow = rows.first();
  const previewId = await previewRow.getAttribute('data-clip-id');
  await previewRow.locator('[data-row-main]').click();
  const position = await page.locator('.history-scroll').evaluate((e) => e.scrollTop);
  await page.keyboard.press('Space');
  await page.getByRole('button', { name: '返回历史列表' }).waitFor();
  await page.keyboard.press('Escape');
  assert.equal(await search.inputValue(), '设计');
  await page.waitForFunction(
    (id) =>
      document.activeElement === document.querySelector(`[data-clip-id="${id}"] [data-row-main]`),
    previewId,
  );
  assert.equal(await selected(), previewId);
  assert.equal(await page.locator('.history-scroll').evaluate((e) => e.scrollTop), position);
  assert.equal(
    await page
      .locator(`[data-clip-id="${previewId}"] [data-row-main]`)
      .evaluate((e) => e === document.activeElement),
    true,
  );
  await search.fill('');
  const lastId = await rows.last().getAttribute('data-clip-id');
  await rows.last().locator('[data-row-main]').click();
  const scrolled = await page.locator('.history-scroll').evaluate((e) => e.scrollTop);
  assert.ok(scrolled > 0, 'Preview restoration must also cover a scrolled list');
  await page.keyboard.press('Space');
  await page.getByRole('button', { name: '返回历史列表' }).waitFor();
  await page.keyboard.press('Escape');
  await page.waitForFunction(
    (id) =>
      document.activeElement === document.querySelector(`[data-clip-id="${id}"] [data-row-main]`),
    lastId,
  );
  assert.equal(await page.locator('.history-scroll').evaluate((e) => e.scrollTop), scrolled);
  await page.locator('[aria-label="历史范围"]').click();
  await page.locator('.action-menu button[value="inspiration"]').click();
  await page.locator('[aria-label="内容类型"]').click();
  await page.locator('.action-menu button[value="image"]').click();
  assert.equal(await rows.count(), 1);
  assert.equal(await rows.first().getAttribute('data-clip-id'), 'image-1');
  await copyRow('image-1');
  assert.match(await page.locator('.toast').textContent(), /图片复制尚未接入/);
  await page.locator('[aria-label="历史范围"]').click();
  await page.locator('.action-menu button[value="all"]').click();
  await page.locator('[aria-label="内容类型"]').click();
  await page.locator('.action-menu button[value="all"]').click();
  await search.fill('设计');
  await page.locator('[aria-label="历史范围"]').click();
  await page.locator('.action-menu button[value="work"]').click();
  assert.equal(await search.inputValue(), '设计');
  await page.locator('[aria-label="历史范围"]').click();
  await page.locator('.action-menu button[value="all"]').click();
  await search.fill('');
  interactions.push(
    'Preview state/scroll/focus restoration, range/type composition, honest unsupported image copy',
  );

  // Keyboard menus and settings restore focus to a live trigger after closing.
  await page.getByRole('button', { name: '更多选项', exact: true }).click();
  await page.keyboard.press('End');
  assert.equal(await page.evaluate(() => document.activeElement.textContent), '快捷键帮助');
  await page.keyboard.press('Escape');
  assert.equal(
    await page
      .getByRole('button', { name: '更多选项', exact: true })
      .evaluate((e) => e === document.activeElement),
    true,
  );
  await appMenu('设置');
  await page.locator('#theme').focus();
  await page.keyboard.press('Tab');
  assert.equal(await page.evaluate(() => document.activeElement.id), 'show-search-by-default');
  await page.keyboard.press('Tab');
  assert.equal(await page.evaluate(() => document.activeElement.tagName), 'SUMMARY');
  await page.keyboard.press('Enter');
  const about = page.locator('.about-section');
  assert.equal(await about.evaluate((e) => e.open), true);
  assert.equal(await about.locator('.about-version').innerText(), `版本 ${version}`);
  assert.doesNotMatch(await about.innerText(), /待验证|Unverified/);
  await page.keyboard.press('Tab');
  const releases = about.getByRole('link', { name: '发布记录' });
  assert.equal(await releases.evaluate((e) => e === document.activeElement), true);
  assert.equal(await releases.getAttribute('href'), 'https://github.com/UClipbot/UClip/releases');
  await page.keyboard.press('Escape');
  await page.waitForFunction(() => document.activeElement === document.querySelector('.app-more'));
  assert.equal(
    await page
      .getByRole('button', { name: '更多选项', exact: true })
      .evaluate((e) => e === document.activeElement),
    true,
  );
  await rowMenu('note-1', '取消置顶');
  await page.locator('[aria-label="历史范围"]').click();
  await page.locator('.action-menu button[value="pinned"]').click();
  assert.equal(await rows.count(), 2);
  await page.locator('[aria-label="历史范围"]').click();
  await page.locator('.action-menu button[value="all"]').click();
  await rowMenu('note-1', '置顶');
  await appMenu('切换网格视图');
  assert.equal(await page.locator('.clip-grid').count(), 1);
  await appMenu('切换列表视图');
  await appMenu('新建示例');
  assert.equal(
    await page.locator('#clip-title').evaluate((e) => e === document.activeElement),
    true,
  );
  await page.locator('#clip-title').fill('交互测试');
  const content = '  中文多行\n\n' + '长路径'.repeat(100) + '\n结尾空白  ';
  await page.locator('#clip-content').fill(content);
  await page.locator('.new-form .primary-button').click();
  const createdId = await rows.first().getAttribute('data-clip-id');
  await copyRow(createdId);
  assert.equal(await clipboard(), content);
  await rowMenu(createdId, '移入回收站');
  await page.getByRole('button', { name: '撤销', exact: true }).click();
  assert.equal(await page.locator(`[data-clip-id="${createdId}"]`).count(), 1);
  await rowMenu(createdId, '移入回收站');
  await page.locator('[aria-label="历史范围"]').click();
  await page.locator('.action-menu button[value="trash"]').click();
  await page.locator(`[data-clip-id="${createdId}"] [data-row-main]`).click();
  await search.focus();
  await search.press('Enter');
  assert.equal(await rows.count(), 0);
  assert.equal(await clipboard(), content);
  await page.getByRole('button', { name: '清除筛选', exact: true }).click();
  interactions.push(
    'Keyboard menus, modal focus, pinning, grid, new example, exact multiline copy, delete/undo/restore',
  );

  // Pathological expressions must not block the UI or copy stale results.
  await appMenu('新建示例');
  await page.locator('#clip-title').fill('Regex timeout fixture');
  await page.locator('#clip-content').fill('a'.repeat(1000) + '!');
  await page.locator('.new-form .primary-button').click();
  await search.fill('(a+)+$');
  await page.getByRole('button', { name: '正则表达式搜索', exact: true }).click();
  await page.evaluate(() => navigator.clipboard.writeText('Query sentinel'));
  await search.press('Enter');
  assert.equal(await clipboard(), 'Query sentinel');
  await page.locator('#search-error').waitFor();
  assert.match(await page.locator('#search-error').textContent(), /耗时过长/);
  await page.keyboard.press(process.platform === 'darwin' ? 'Meta+,' : 'Control+,');
  await page.getByRole('dialog').waitFor();
  await page.keyboard.press('Escape');
  await search.fill('[');
  await page.locator('#search-error').waitFor();
  assert.match(await page.locator('#search-error').textContent(), /无效/);
  await search.fill('linear');
  await search.fill('tauri');
  await page.locator('[data-clip-id="link-3"]').waitFor();
  assert.equal(await rows.count(), 1);
  await page.getByRole('button', { name: '正则表达式搜索', exact: true }).click();
  await search.fill('');
  interactions.push(
    'Worker timeout, pending-query action guard, invalid regex, rapid query replacement, responsive settings',
  );

  // Reload resets only demo records. Layout evidence covers compact, minimum and fallback sizes.
  await page.reload();
  await rows.first().waitFor();
  assert.equal(await search.isVisible(), true);
  await rows.first().locator('[data-row-main]').click();
  for (const theme of ['dark', 'light']) {
    await appMenu('设置');
    await page.locator('#theme').selectOption(theme);
    await page.getByRole('button', { name: '关闭面板', exact: true }).click();
    for (const [width, height] of [
      [640, 650],
      [480, 390],
      [900, 670],
      [375, 650],
      [812, 345],
    ]) {
      await page.setViewportSize({ width, height });
      const report = await page.evaluate(() => {
        const area = document.querySelector('.history-scroll').getBoundingClientRect();
        return {
          viewport: [innerWidth, innerHeight],
          document: [document.documentElement.scrollWidth, document.documentElement.scrollHeight],
          visibleRecords: [...document.querySelectorAll('.clip-row')].filter((e) => {
            const r = e.getBoundingClientRect();
            return r.top >= area.top && r.bottom <= area.bottom;
          }).length,
          overflow: [...document.querySelectorAll('button, input, select')]
            .filter(
              (e) =>
                e.getClientRects().length &&
                (e.getBoundingClientRect().right > innerWidth + 1 ||
                  e.getBoundingClientRect().left < -1),
            )
            .map((e) => e.textContent),
          toolbar: ['.search-box', '.filter-bar'].map((selector) => {
            const r = document.querySelector(selector).getBoundingClientRect();
            return { left: r.left, right: r.right, top: r.top };
          }),
        };
      });
      assert.ok(report.document[0] <= width, JSON.stringify(report));
      assert.equal(report.overflow.length, 0, JSON.stringify(report));
      assert.ok(report.toolbar[0].right <= report.toolbar[1].left, JSON.stringify(report));
      assert.ok(
        Math.abs(report.toolbar[0].top - report.toolbar[1].top) <= 1,
        JSON.stringify(report),
      );
      if (width === 640) assert.ok(report.visibleRecords >= 5);
      if (width === 640) {
        const samples = await page.evaluate(() => {
          const rgb = (value) => value.match(/[\d.]+/g)?.map(Number) ?? [];
          const luminance = (color) =>
            color
              .slice(0, 3)
              .map((v) => {
                const c = v / 255;
                return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
              })
              .reduce((sum, c, i) => sum + c * [0.2126, 0.7152, 0.0722][i], 0);
          return [
            '.clip-row.selected .clip-excerpt',
            '.clip-row.selected .clip-row-meta',
            '.statusbar',
            '.capture-toggle span',
            '.scope-control .filter-trigger',
            '.type-control .filter-trigger',
          ].map((selector) => {
            const e = document.querySelector(selector);
            const foreground = getComputedStyle(e).color;
            let parent = e;
            let background;
            while (parent) {
              const value = getComputedStyle(parent).backgroundColor;
              const components = rgb(value);
              if (components.length === 3 || components[3] === 1) {
                background = value;
                break;
              }
              parent = parent.parentElement;
            }
            const a = luminance(rgb(foreground));
            const b = luminance(rgb(background));
            return {
              selector,
              foreground,
              background,
              ratio: (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05),
            };
          });
        });
        for (const sample of samples)
          assert.ok(sample.ratio >= 4.5, JSON.stringify({ theme, ...sample }));
        contrastSamples.push(...samples.map((sample) => ({ theme, ...sample })));
      }

      reports.push({ theme, ...report });
      if (width === 640 || width === 375) {
        const dismiss = page.getByRole('button', { name: '关闭提示', exact: true });
        if (await dismiss.count()) await dismiss.click();
        await page.screenshot({
          path: `docs/previews/simple-${width === 375 ? 'narrow-' : ''}${theme}.png`,
        });
      }
      await rowMenu('note-1', '查看完整内容');
      assert.equal(await page.getByRole('button', { name: '返回历史列表' }).isVisible(), true);
      assert.ok(await page.locator('.detail-footer .primary-button').isVisible());
      await page.keyboard.press('Escape');
    }
  }
  await page.setViewportSize({ width: 640, height: 650 });
  await appMenu('设置');
  await page.screenshot({ path: 'docs/previews/simple-settings.png' });
  await page.keyboard.press('Escape');
  await rowMenu('image-1', '查看完整内容');
  await page.screenshot({ path: 'docs/previews/simple-image.png' });
  await page.keyboard.press('Escape');
  console.log(JSON.stringify({ viewports: reports.length, interactions, failures }, null, 2));
  assert.equal(failures.length, 0);
} catch (error) {
  failures.push(error.stack);
  await page.screenshot({ path: '/tmp/uclip-simple-failure.png' });
  throw error;
} finally {
  writeFileSync(
    'docs/previews/simple-layout-check.json',
    JSON.stringify({ reports, interactions, failures }, null, 2),
  );
  writeFileSync(
    'docs/previews/simple-contrast-check.json',
    JSON.stringify({ samples: contrastSamples }, null, 2),
  );
  await browser.close();
}
