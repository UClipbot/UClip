// Headed Chromium: OS clipboard integration, not the isolated headless clipboard used by UI tests.
import { createInterface } from 'node:readline';
const { chromium } = await import(process.argv[2] || 'playwright');
const browser = await chromium.launch({ headless: false });
const page = await browser.newPage();
await page.goto(
  'data:text/html,<title>UClip browser paste receiver</title><textarea autofocus style="width:90vw;height:70vh"></textarea><script>window.lastPaste=null;document.addEventListener("paste",async(e)=>{const f=e.clipboardData.files[0];if(f){const b=await createImageBitmap(f);const c=document.createElement("canvas");c.width=b.width;c.height=b.height;const x=c.getContext("2d");x.drawImage(b,0,0);window.lastPaste={kind:"image",width:b.width,height:b.height,rgba:Array.from(x.getImageData(0,0,b.width,b.height).data)};}else window.lastPaste={kind:"text",text:e.clipboardData.getData("text/plain")};});</script>',
);
console.log('FIXTURE:' + JSON.stringify({ ok: true, value: { ready: true } }));
for await (const line of createInterface({ input: process.stdin })) {
  try {
    const request = JSON.parse(line);
    let value;
    if (request.op === 'paste') {
      await page.bringToFront();
      await page.locator('textarea').focus();
      await page.evaluate(() => {
        window.lastPaste = null;
      });
      await page.keyboard.press('Control+v');
      await page.waitForFunction(() => window.lastPaste !== null);
      value = await page.evaluate(() => window.lastPaste);
    } else if (request.op === 'copy') {
      await page.bringToFront();
      await page.locator('textarea').fill(request.text);
      await page.locator('textarea').selectText();
      await page.keyboard.press('Control+c');
      value = { copied: true };
    } else value = {};
    console.log('FIXTURE:' + JSON.stringify({ ok: true, value }));
  } catch {
    console.log('FIXTURE:' + JSON.stringify({ ok: false }));
  }
}
await browser.close();
