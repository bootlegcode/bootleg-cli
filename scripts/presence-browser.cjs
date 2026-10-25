// Stateful real Chromium driver. JSON-line commands/results; credentials only on stdin.
// No request routing, synthetic EventSource, snapshots or HTTP responses.
const readline = require('node:readline');
const path = require('node:path');
const assert = require('node:assert/strict');
const { chromium } = require(path.resolve(__dirname, '../../bootleg-web/node_modules/playwright'));
(async () => {
  let browser, context, page;
  for (const signal of ['SIGTERM', 'SIGINT']) {
    process.once(signal, async () => { await browser?.close(); process.exit(1); });
  }
  const input = readline.createInterface({ input: process.stdin });
  try {
    for await (const line of input) {
      const command = JSON.parse(line);
      if (command.action === 'open') {
        browser = await chromium.launch({ headless: true });
        context = await browser.newContext();
        await context.addCookies(command.cookies.map(c => ({ ...c, url: command.origin })));
        page = await context.newPage();
        await page.goto(command.origin + command.path);
        await page.waitForFunction(() => document.querySelector('[data-instances-status]')?.textContent.includes('live updates connected'));
      } else if (command.action === 'rows') {
        await page.waitForFunction(({ count, connected }) => {
          const rows = [...document.querySelectorAll('[data-instances-rows] tr')];
          return rows.length === count && rows.filter(r => r.cells[1].textContent === 'Connected').length === connected;
        }, command, { timeout: 20000 });
        // Markup labels must remain text even in a real DOM.
        assert.equal(await page.locator('[data-instances-rows] b, [data-instances-rows] img').count(), 0);
        if (command.label) assert.ok((await page.locator('[data-instances-rows]').textContent()).includes(command.label));
      } else if (command.action === 'stale') {
        await page.waitForFunction(() => document.querySelector('[data-instances-status]').textContent.includes('Stale data'));
        assert.equal(await page.locator('[data-instances-rows] tr').count(), command.count);
      } else if (command.action === 'live') {
        await page.waitForFunction(() => document.querySelector('[data-instances-status]').textContent.includes('Live updates connected'));
      } else if (command.action === 'denied') {
        await page.waitForFunction(() => document.querySelector('[data-instances-status]').textContent.includes('not accessible'));
        assert.equal(await page.locator('[data-instances-rows] tr').count(), 0);
        assert.equal(await page.locator('[data-instances-results]').isVisible(), false);
      } else if (command.action === 'close') {
        await browser.close();
        browser = undefined;
        console.log(JSON.stringify({ ok: true, action: command.action }));
        break;
      } else {
        throw new Error('Unknown browser command');
      }
      console.log(JSON.stringify({ ok: true, action: command.action }));
    }
  } finally {
    await browser?.close();
    input.close();
  }
})().catch(() => {
  // Never echo command payload, cookies, page HTML or Playwright error traces.
  console.log(JSON.stringify({ ok: false, error: 'browser acceptance assertion failed' }));
  process.exitCode = 1;
});
