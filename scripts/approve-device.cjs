// Real Chromium approval against the disposable web service. Secrets enter on stdin only.
const fs = require('node:fs');
const path = require('node:path');
const { chromium } = require(path.resolve(__dirname, '../../bootleg-web/node_modules/playwright'));
(async () => {
  const input = JSON.parse(fs.readFileSync(0, 'utf8'));
  const browser = await chromium.launch({ headless: true });
  try {
    const context = await browser.newContext();
    await context.addCookies(input.cookies.map(c => ({ ...c, url: input.origin })));
    const page = await context.newPage();
    await page.goto(input.origin + input.path);
    await page.locator('input[name="name"]').fill(input.name);
    await page.locator(`button[name="decision"][value="${input.decision}"]`).click();
    await page.waitForLoadState('networkidle');
  } finally {
    await browser.close();
  }
})().catch(() => { console.error('Chromium device approval failed (details suppressed to protect credentials)'); process.exitCode = 1; });
