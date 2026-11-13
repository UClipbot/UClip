// A disposable, headed Google Chrome process that writes the actual X11 clipboard.
import { createInterface } from 'node:readline';
const { chromium } = await import(process.argv[2] || 'playwright');
const browser = await chromium.launch({
  headless: false,
  executablePath: process.env.UCLIP_SOURCE_CHROME || '/usr/bin/google-chrome',
  args: ['--ozone-platform=x11'],
});
const page = await browser.newPage();
await page.setContent(
  '<title>UClip source fixture</title><textarea aria-label="Link to copy"></textarea>',
);
console.log('FIXTURE:' + JSON.stringify({ ok: true, value: { ready: true } }));
try {
  for await (const line of createInterface({ input: process.stdin })) {
    try {
      const request = JSON.parse(line);
      await page.bringToFront();
      await page.locator('textarea').fill(request.text);
      await page.locator('textarea').selectText();
      await page.keyboard.press('Control+c');
      console.log('FIXTURE:' + JSON.stringify({ ok: true, value: { copied: true } }));
    } catch {
      console.log('FIXTURE:' + JSON.stringify({ ok: false }));
    }
  }
} finally {
  await browser.close();
}
