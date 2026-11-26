// Browser demo regression; Playwright remains an external test dependency.
import assert from 'node:assert/strict';
const { chromium } = await import(process.argv[2] || 'playwright');
const browser = await chromium.launch({ headless: true });
try {
  const context = await browser.newContext({
    permissions: ['clipboard-read', 'clipboard-write'],
    locale: 'en-US',
  });
  const page = await context.newPage();
  const errors = [];
  page.on('pageerror', (error) => errors.push(error.message));
  await page.goto('http://127.0.0.1:1420');
  const rows = page.locator('[data-row-main]');
  await rows.first().waitFor();
  for (const theme of ['light', 'dark']) {
    await page.evaluate((theme) => {
      document.documentElement.dataset.theme = theme;
    }, theme);
    await rows.nth(0).click();
    await rows.nth(1).click({ modifiers: ['Control'] });
    assert.equal(await page.locator('[data-row-main][aria-pressed=true]').count(), 2);
    await rows.nth(1).click({ modifiers: ['Control'] });
    assert.equal(await page.locator('[data-row-main][aria-pressed=true]').count(), 1);
    await rows.nth(3).click({ modifiers: ['Shift'] });
    assert.equal(await page.locator('[data-row-main][aria-pressed=true]').count(), 3);
    await rows.nth(0).click();
    await page.keyboard.press('Shift+ArrowDown');
    assert.equal(await page.locator('[data-row-main][aria-pressed=true]').count(), 2);
    await page.keyboard.press('Control+a');
    assert.equal(
      await page.locator('[data-row-main][aria-pressed=true]').count(),
      await rows.count(),
    );
  }
  const textRows = page.locator('[data-clip-type=text] [data-row-main]');
  await textRows.nth(0).click();
  await textRows.nth(1).click({ modifiers: ['Control'] });
  const expected = await page.locator('.clip-row.selected .clip-excerpt').allTextContents();
  assert.equal(await page.locator('.selection-toolbar').count(), 0);
  await page.keyboard.press('Control+c');
  const copied = await page.evaluate(() => navigator.clipboard.readText());
  assert.equal(copied.replace(/\n\s*\n/g, '\n'), expected.join('\n'));
  assert.ok(copied.includes('\n\n'), 'original blank lines are preserved');
  const before = await rows.count();
  await page.keyboard.press('Delete');
  await page.waitForFunction(
    (before) => document.querySelectorAll('[data-row-main]').length === before - 2,
    before,
  );
  await page.locator('.toast button').first().click();
  await page.waitForFunction(
    (before) => document.querySelectorAll('[data-row-main]').length === before,
    before,
  );
  await rows.nth(0).click();
  await rows.nth(1).click({ modifiers: ['Control'] });
  // Blank space clears every selected row, and Delete then does nothing.
  await page.locator('.history-scroll').evaluate((element) => element.click());
  assert.equal(await page.locator('.clip-row.selected').count(), 0);
  await page.keyboard.press('Delete');
  assert.equal(await rows.count(), before);
  await page.keyboard.press('Space');
  assert.equal(await page.locator('.detail-panel').count(), 0);
  await rows.nth(0).click();
  await page.keyboard.press('Space');
  await page.locator('.detail-panel').waitFor();
  await page.keyboard.press('Escape');
  await page.locator('.detail-panel').waitFor({ state: 'detached' });
  await rows.nth(0).click({ modifiers: ['Control'] });
  assert.equal(await page.locator('.clip-row.selected').count(), 0);
  const search = page.locator('#history-search');
  const toolbar = page.locator('#history-search-panel');
  assert.equal(await toolbar.isVisible(), true);
  await page.keyboard.press('Control+f');
  await toolbar.waitFor({ state: 'hidden' });
  assert.equal(await page.locator('.filter-bar').count(), 0);
  await page.keyboard.press('Control+f');
  await search.waitFor();
  await page.waitForFunction(() => document.activeElement?.id === 'history-search');
  await search.fill('no-such-item-123');
  await rows.first().waitFor({ state: 'detached' });
  assert.equal(await page.getByRole('button', { name: /^(Reset|Clear search)$/ }).count(), 0);
  await page.keyboard.press('Control+f');
  await toolbar.waitFor({ state: 'hidden' });
  assert.equal(await page.locator('.filter-bar').count(), 0);
  await page.waitForFunction(() => document.activeElement?.matches('.history-scroll'));
  await rows.first().waitFor();
  await page.keyboard.press('Control+f');
  await search.waitFor();
  assert.equal(await search.inputValue(), '');
  // Ctrl+C in the search field retains the browser's normal text-copy behavior.
  await page.locator('#history-search').fill('search text');
  await page.locator('#history-search').selectText();
  await page.keyboard.press('Control+c');
  assert.equal(await page.evaluate(() => navigator.clipboard.readText()), 'search text');
  await page.locator('#history-search').fill('');
  await rows.first().waitFor();
  await textRows.nth(0).click();
  await textRows.nth(1).click({ modifiers: ['Control'] });
  await textRows.nth(0).click({ button: 'right' });
  assert.equal(await page.locator('.clip-row.selected').count(), 2);
  assert.equal(await page.getByRole('menuitem').count(), 1);
  await page.getByTestId('delete-history-item').click();
  await page.waitForFunction(
    (before) => document.querySelectorAll('[data-row-main]').length === before - 2,
    before,
  );
  await page.locator('.toast button').first().click();
  await page.waitForFunction(
    (before) => document.querySelectorAll('[data-row-main]').length === before,
    before,
  );
  await rows.nth(0).click();
  await rows.nth(1).click({ modifiers: ['Control'] });
  await page.locator('#history-search').fill('no-such-item-123');
  await page.waitForFunction(() => !document.querySelector('.clip-row.selected'));
  await page.getByTestId('footer-settings').click();
  const showByDefault = page.getByRole('checkbox', { name: 'Show search and filters by default' });
  assert.equal(await showByDefault.isChecked(), true);
  await showByDefault.uncheck();
  await page.reload();
  await rows.first().waitFor();
  assert.equal(await toolbar.isVisible(), false);
  await page.locator('.search-toggle').click();
  await toolbar.waitFor();
  await page.reload();
  await rows.first().waitFor();
  assert.equal(await toolbar.isVisible(), false);
  await page.getByTestId('footer-settings').click();
  await showByDefault.check();
  await page.reload();
  await toolbar.waitFor();
  assert.deepEqual(errors, []);
  console.log(
    'PASS multi-selection: light/dark, toggle/range/arrows/select-all, Ctrl+C merged copy, Delete/undo, minimal context menu, blank deselection, native input copy, query reset',
  );
} finally {
  await browser.close();
}
