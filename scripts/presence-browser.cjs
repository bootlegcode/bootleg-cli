// Stateful real Chromium driver. JSON-line commands/results; credentials only on stdin.
// No request routing, synthetic EventSource, snapshots or HTTP responses.
const readline = require('node:readline');
const { spawn } = require('node:child_process');
const path = require('node:path');
const assert = require('node:assert/strict');
const { chromium } = require(path.resolve(__dirname, '../../bootleg-web/node_modules/playwright'));
let failureStage = 'command';
(async () => {
  let browser, chrome, context, page, cover, closing;
  let streamStarts = 0;
  const streams = new Set();
  async function shutdown() {
    if (!closing) closing = (async () => {
      try { await browser?.close(); } finally {
        if (chrome && chrome.exitCode === null && chrome.signalCode === null) {
          await new Promise(resolve => { chrome.once('exit', resolve); chrome.kill('SIGTERM'); });
        }
      }
    })();
    return closing;
  }
  async function waitFor(predicate) {
    const deadline = Date.now() + 10000;
    while (!predicate()) {
      assert.ok(Date.now() < deadline, 'stream lifecycle deadline');
      await new Promise(resolve => setTimeout(resolve, 50));
    }
  }
  for (const signal of ['SIGTERM', 'SIGINT']) {
    process.once(signal, async () => { await shutdown(); process.exit(1); });
  }
  const input = readline.createInterface({ input: process.stdin });
  try {
    for await (const line of input) {
      const command = JSON.parse(line);
      if (command.action === 'open') {
        // Playwright's usual focus emulation keeps background pages visible. Connect to
        // our own full Chromium with noDefaults so tab visibility is native, not mocked.
        failureStage = 'chromium_start';
        // Ubuntu hosted runners restrict unprivileged user namespaces. Match
        // Playwright's default sandbox policy, but only on disposable GHA Linux.
        const ciFlags = process.platform === 'linux' && process.env.GITHUB_ACTIONS === 'true'
          ? ['--no-sandbox'] : [];
        chrome = spawn(chromium.executablePath(), [...ciFlags, '--headless=new', '--remote-debugging-port=0',
          '--user-data-dir=' + command.profile, '--no-first-run', '--no-default-browser-check'],
          { stdio: ['ignore', 'ignore', 'pipe'] });
        const endpoint = await new Promise((resolve, reject) => {
          const timer = setTimeout(() => reject(new Error('Chromium startup deadline')), 15000);
          let text = '';
          chrome.stderr.on('data', chunk => {
            text = (text + chunk).slice(-16384);
            const match = text.match(/DevTools listening on (ws:\/\/\S+)/);
            if (match) { clearTimeout(timer); resolve(match[1]); }
          });
          chrome.once('error', () => { clearTimeout(timer); reject(new Error('Chromium launch failed')); });
          chrome.once('exit', () => { clearTimeout(timer); reject(new Error('Chromium exited')); });
        });
        failureStage = 'cdp_connect';
        browser = await chromium.connectOverCDP(endpoint, { noDefaults: true });
        context = browser.contexts()[0];
        await context.addCookies(command.cookies.map(c => ({ ...c, url: command.origin })));
        page = await context.newPage();
        const network = await context.newCDPSession(page);
        await network.send('Network.enable');
        network.on('Network.requestWillBeSent', event => {
          if (event.request.url.endsWith('/instances/events')) { streams.add(event.requestId); streamStarts++; }
        });
        network.on('Network.loadingFailed', event => streams.delete(event.requestId));
        network.on('Network.loadingFinished', event => streams.delete(event.requestId));
        await page.bringToFront();
        failureStage = 'page_navigation';
        await page.goto(command.origin + command.path);
        failureStage = 'initial_live';
        await page.waitForFunction(() => document.querySelector('[data-instances-status]')?.textContent.includes('live updates connected'));
      } else if (command.action === 'rows') {
        await page.waitForFunction(({ count, connected }) => {
          const rows = [...document.querySelectorAll('[data-instances-rows] tr')];
          return rows.length === count && rows.filter(r => r.cells[1].textContent === 'Connected').length === connected;
        }, command, { timeout: 20000 });
        // Markup labels must remain text even in a real DOM.
        assert.equal(await page.locator('[data-instances-rows] b, [data-instances-rows] img').count(), 0);
        if (command.label) assert.ok((await page.locator('[data-instances-rows]').textContent()).includes(command.label));
        if (command.baselines) {
          const text = await page.locator('[data-instances-rows]').textContent();
          assert.ok(text.includes('Unknown commit · cleanliness unknown'));
          assert.ok(text.includes('a'.repeat(40) + ' · dirty'));
        }
      } else if (command.action === 'hide') {
        cover = await context.newPage();
        await cover.bringToFront();
        await page.waitForFunction(() => document.hidden && document.querySelector('[data-instances-status]').textContent.includes('paused'), null, { polling: 100 });
        await waitFor(() => streams.size === 0);
      } else if (command.action === 'show') {
        const previous = streamStarts;
        await page.bringToFront();
        await page.waitForFunction(() => !document.hidden && document.querySelector('[data-instances-status]').textContent.includes('Live updates connected'));
        await waitFor(() => streamStarts > previous && streams.size === 1);
        await cover.close();
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
        await shutdown();
        console.log(JSON.stringify({ ok: true, action: command.action }));
        break;
      } else {
        throw new Error('Unknown browser command');
      }
      failureStage = 'command';
      console.log(JSON.stringify({ ok: true, action: command.action }));
    }
  } finally {
    await shutdown();
    input.close();
  }
})().catch(() => {
  // Never echo command payload, cookies, page HTML or Playwright error traces.
  console.log(JSON.stringify({ ok: false, error: 'browser acceptance assertion failed', stage: failureStage }));
  process.exitCode = 1;
});
