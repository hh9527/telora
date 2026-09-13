import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {load} from './host.mjs';

const [file, browserUrl] = process.argv.slice(2);
if (!file) throw Error('expected tail-calls artifact and optional browser base URL');
const bytes = await readFile(file);
const expected = [42,42,42,42,42,42,42,true,40];
const session = await load(bytes);
session.setDebugEnabled(true);
session.initialize();
session.initialize();
assert.deepEqual(JSON.parse(session.call([])), expected);
assert.deepEqual(session.debugEvents().map(event => event.repr), ['40']);
assert.deepEqual(session.diagnostics(), []);

if (browserUrl) {
  const {chromium} = await import(process.env.TELORA_PLAYWRIGHT_MODULE ?? 'playwright');
  const browser = await chromium.launch({headless: true});
  try {
    const page = await browser.newPage();
    await page.goto(browserUrl + '/scalar.html');
    const result = await page.evaluate(async encoded => {
      const {load} = await import('./host.mjs');
      const session = await load(Uint8Array.from(atob(encoded), ch => ch.charCodeAt(0)));
      session.setDebugEnabled(true);
      session.initialize();
      return {value: session.call([]), debug: session.debugEvents(), diagnostics: session.diagnostics()};
    }, bytes.toString('base64'));
    assert.deepEqual(JSON.parse(result.value), expected);
    assert.deepEqual(result.debug.map(event => event.repr), ['40']);
    assert.deepEqual(result.diagnostics, []);
  } finally { await browser.close(); }
}
console.log(browserUrl ? 'Node and Chromium: 20,000 tail calls passed' : 'Node: 20,000 tail calls passed');
