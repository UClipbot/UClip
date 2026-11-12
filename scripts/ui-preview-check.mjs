// Optional browser verification tool; Playwright is not an application dependency.
const moduleIndex = process.argv.indexOf('--playwright-module');
const { chromium } = await import(moduleIndex < 0 ? 'playwright' : process.argv[moduleIndex + 1]);
import { mkdirSync, writeFileSync } from 'node:fs';
mkdirSync('docs/previews', { recursive: true });
const browser = await chromium.launch({ headless: true });
const page = await browser.newPage({
  viewport: { width: 1180, height: 790 },
  colorScheme: 'dark',
  reducedMotion: 'reduce',
});
const failures = [];
page.on('pageerror', (error) => failures.push(error.message));
await page.goto('http://localhost:1420');
await page.locator('.clip-row').first().waitFor();
const reports = [];
for (const theme of ['dark', 'light']) {
  await page.getByTestId('open-settings').click();
  await page.locator('#theme').selectOption(theme);
  await page.getByRole('button', { name: '关闭面板', exact: true }).click();
  for (const size of [
    { width: 1440, height: 900 },
    { width: 1180, height: 790 },
    { width: 900, height: 680 },
    { width: 760, height: 700 },
    { width: 375, height: 812 },
    { width: 812, height: 375 },
  ]) {
    await page.setViewportSize(size);
    const report = await page.evaluate(() => ({
      viewport: [innerWidth, innerHeight],
      document: [document.documentElement.scrollWidth, document.documentElement.scrollHeight],
      shell: document.querySelector('.app-shell').getBoundingClientRect().toJSON(),
      overflow: [...document.querySelectorAll('button, input, h1, .record-title')]
        .filter(
          (e) => e.getClientRects().length && e.getBoundingClientRect().right > innerWidth + 1,
        )
        .map((e) => e.textContent),
    }));
    if (report.document[0] > size.width || report.overflow.length)
      failures.push({ theme, ...report });
    reports.push({ theme, ...report });
    if (size.width === 375) {
      await page.locator('[data-clip-id="note-1"]').click();
      await page.getByRole('button', { name: '返回历史列表' }).waitFor({ state: 'visible' });
      if (!(await page.locator('.record-title').isVisible()))
        failures.push('mobile selected preview not visible');
      const dismissToast = page.getByRole('button', { name: '关闭提示', exact: true });
      if (await dismissToast.count()) await dismissToast.click();
      await page.screenshot({ path: `docs/previews/moss-mobile-${theme}.png` });
      await page.getByRole('button', { name: '返回历史列表' }).click();
      await page.getByTestId('open-settings').click();
      await page.keyboard.press('Escape');
      if (await page.getByRole('dialog').count()) failures.push('Escape did not close settings');
      if (!(await page.getByTestId('open-settings').evaluate((e) => e === document.activeElement)))
        failures.push('Modal did not restore focus');
    }
  }
}
await page.setViewportSize({ width: 1180, height: 790 });
await page.keyboard.press('Control+k');
if (!(await page.locator('#history-search').evaluate((e) => e === document.activeElement)))
  failures.push('Control+K did not focus search');
await page.locator('#history-search').fill('');
await page.keyboard.press('ArrowDown');
if ((await page.locator('.record-title').textContent()) !== 'Linear — Plan and build products')
  failures.push('ArrowDown did not select next record');
await page.locator('#history-search').fill('somewhere');
if ((await page.locator('.clip-row').count()) !== 1) failures.push('Search result count wrong');
await page.keyboard.press('Escape');
if ((await page.locator('#history-search').inputValue()) !== '')
  failures.push('Escape did not clear search');
// A pathological expression must time out without blocking keyboard or modal interaction.
await page.getByRole('button', { name: '新建片段', exact: true }).click();
await page.locator('#clip-title').fill('Regex timeout fixture');
await page.locator('#clip-content').fill('a'.repeat(1000) + '!');
await page.locator('.new-form .primary-button').click();
await page.locator('#history-search').fill('(a+)+$');
await page.getByRole('button', { name: '正则表达式搜索', exact: true }).click();
await page.locator('#search-error').waitFor({ state: 'visible' });
if (!(await page.locator('#search-error').textContent()).includes('耗时过长'))
  failures.push('Pathological regex was not terminated');
await page.keyboard.press('Control+,');
await page.getByRole('dialog').waitFor({ state: 'visible' });
await page.keyboard.press('Escape');
await page.getByRole('button', { name: '正则表达式搜索', exact: true }).click();
await page.getByRole('button', { name: '清空搜索', exact: true }).click();
writeFileSync('docs/previews/layout-check.json', JSON.stringify({ reports, failures }, null, 2));
console.log(JSON.stringify({ viewports: reports.length, failures }, null, 2));
await browser.close();
if (failures.length) process.exit(1);
