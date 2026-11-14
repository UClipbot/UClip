// Optional browser verification; Playwright remains outside application dependencies.
import assert from 'node:assert/strict';
import { mkdirSync } from 'node:fs';
const argument = (flag, fallback) => {
  const index = process.argv.indexOf(flag);
  return index < 0 ? fallback : process.argv[index + 1];
};
const { chromium } = await import(argument('--playwright-module', 'playwright'));
const browser = await chromium.launch({ headless: true });
const url = argument('--url', 'http://localhost:1420');
const failures = [];
mkdirSync('docs/previews/languages', { recursive: true });
try {
  for (const systemLanguage of ['zh-CN', 'en-US', 'fr-FR']) {
    const context = await browser.newContext({ locale: systemLanguage, reducedMotion: 'reduce' });
    const page = await context.newPage();
    page.on('pageerror', (error) => failures.push(error.message));
    await page.goto(url);
    const systemLocale = systemLanguage === 'zh-CN' ? 'zh-CN' : 'en';
    await page.waitForFunction((locale) => document.documentElement.lang === locale, systemLocale);
    const contents = await page.locator('.clip-excerpt').allTextContents();
    await page.getByTestId('footer-settings').click();
    for (const locale of ['en', 'zh-CN']) {
      await page.locator('#language').selectOption(locale);
      await page.waitForFunction((locale) => document.documentElement.lang === locale, locale);
      assert.equal(
        await page.locator('#modal-title').textContent(),
        locale === 'en' ? 'Settings' : '\u8bbe\u7f6e',
      );
      assert.equal(
        await page.locator('#history-search').getAttribute('placeholder'),
        locale === 'en' ? 'Search history…' : '\u641c\u7d22\u5386\u53f2…',
      );
      await page.reload();
      await page.waitForFunction((locale) => document.documentElement.lang === locale, locale);
      assert.deepEqual(await page.locator('.clip-excerpt').allTextContents(), contents);
      await page.getByTestId('footer-settings').click();
    }
    await page.locator('#language').selectOption('system');
    await page.waitForFunction((locale) => document.documentElement.lang === locale, systemLocale);
    await page.locator('.modal-heading .icon-button').click();
    await page.locator('#history-search').fill('[');
    await page
      .getByRole('button', {
        name:
          systemLocale === 'en'
            ? 'Regular expression search'
            : '\u6b63\u5219\u8868\u8fbe\u5f0f\u641c\u7d22',
        exact: true,
      })
      .click();
    await page.locator('#search-error').waitFor();
    assert.match(
      await page.locator('#search-error').textContent(),
      systemLocale === 'en'
        ? /Invalid regular expression/
        : /\u6b63\u5219\u8868\u8fbe\u5f0f\u65e0\u6548/,
    );
    await context.close();
  }
  const context = await browser.newContext({ locale: 'en-US', reducedMotion: 'reduce' });
  const page = await context.newPage();
  page.on('pageerror', (error) => failures.push(error.message));
  await page.goto(url);
  for (const theme of ['dark', 'light'])
    for (const width of [375, 640, 1200]) {
      await page.setViewportSize({ width, height: 740 });
      await page.getByTestId('footer-settings').click();
      await page.locator('#theme').selectOption(theme);
      assert.equal(
        await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth),
        true,
      );
      await page.screenshot({ path: `docs/previews/languages/en-${theme}-${width}.png` });
      await page.locator('.modal-heading .icon-button').click();
      await page.getByRole('button', { name: 'More options', exact: true }).click();
      assert.equal(
        await page.evaluate(() => {
          const r = document.querySelector('.action-menu').getBoundingClientRect();
          return r.left >= 0 && r.right <= innerWidth;
        }),
        true,
      );
      await page.keyboard.press('Escape');
    }
  await context.close();
  assert.deepEqual(failures, []);
  console.log(
    'PASS languages: system detection/fallback, live switching, reload persistence, preserved sample content, worker errors, English menus/settings at 375/640/1200px in both themes.',
  );
} finally {
  await browser.close();
}
