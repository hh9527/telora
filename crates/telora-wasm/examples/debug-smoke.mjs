import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {load} from './host.mjs';

const [file, browserUrl] = process.argv.slice(2);
if (!file) throw Error('expected debug-publish artifact filename and optional browser base URL');
const bytes = await readFile(file);
const expected = [
  '42', '{name: "é🦀", value: 42}', '<fn>',
  "('True, (), b\"\\x61\\x62\", [1, 2], {a: 1, b: 2}, 'Some(3), -0.0, <dyn>)",
];
const session = await load(bytes);
session.setDebugEnabled(true);
session.initialize();
const initial = session.debugEvents();
assert.equal(initial.length, 1);
assert.equal(initial[0].repr, '41');
assert.equal(initial[0].message, 'initialize');
assert.deepEqual(JSON.parse(session.call([])), [true, true, true]);
assert.deepEqual(session.debugEvents().map(event => event.repr), expected);
assert.deepEqual(session.debugEvents(), []);
assert.deepEqual(session.diagnostics(), []);
session.setDebugEnabled(false);
session.call([]);
assert.deepEqual(session.debugEvents(), []);

if (browserUrl) {
  const {chromium} = await import(process.env.TELORA_PLAYWRIGHT_MODULE ?? 'playwright');
  const browser = await chromium.launch({headless: true});
  try {
    const page = await browser.newPage();
    await page.goto(browserUrl + '/scalar.html');
    const result = await page.evaluate(async encoded => {
      const {load} = await import('./host.mjs');
      const bytes = Uint8Array.from(atob(encoded), ch => ch.charCodeAt(0));
      const session = await load(bytes);
      session.setDebugEnabled(true);
      session.initialize();
      const initial = session.debugEvents();
      const value = session.call([]);
      return {initial, value, events: session.debugEvents(), diagnostics: session.diagnostics()};
    }, bytes.toString('base64'));
    assert.deepEqual(result.initial, initial);
    assert.deepEqual(JSON.parse(result.value), [true, true, true]);
    assert.deepEqual(result.events.map(event => event.repr), expected);
    assert.deepEqual(result.diagnostics, []);
  } finally { await browser.close(); }
}
console.log(browserUrl ? 'Node and Chromium debug transport passed' : 'Node debug transport passed');
