// Run against a local HTTP server for this directory; Playwright is optional tooling.
import assert from 'node:assert/strict';
const { chromium } = await import(process.env.TELORA_PLAYWRIGHT_MODULE ?? 'playwright');
const [base, aggregateArtifact, inputArtifact, entryArtifact] = process.argv.slice(2);
if (!base || !aggregateArtifact || !inputArtifact) throw Error('expected base URL and two artifact filenames');
const browser = await chromium.launch({ headless: true });
try {
  const page = await browser.newPage();
  const errors = [];
  page.on('pageerror', error => errors.push(String(error)));
  await page.goto(base + '/scalar.html');
  await page.getByLabel('Wasm 文件').setInputFiles(aggregateArtifact);
  await page.locator('#run').click();
  await page.waitForFunction(() => document.querySelector('pre').textContent.startsWith('['));
  assert.deepEqual(JSON.parse(await page.locator('pre').textContent()), [42, [
    { label: '短文本', score: 19 },
    { label: 'a longer string stored in the string table', score: 23 },
  ], null]);
  await page.getByLabel('Wasm 文件').setInputFiles(inputArtifact);
  await page.getByLabel('参数数组').fill(JSON.stringify([{ name: '浏览器输入', values: [22] }]));
  await page.locator('#run').click();
  await page.waitForFunction(() => document.querySelector('pre').textContent.startsWith('{'));
  assert.deepEqual(JSON.parse(await page.locator('pre').textContent()), { name: '浏览器输入', total: 42 });
  if (entryArtifact) {
    await page.getByLabel('Wasm 文件').setInputFiles(entryArtifact);
    await page.getByLabel('参数数组').fill(JSON.stringify({args: ['浏览器参数'], env: {}, sources: {input: {number: 42, nested: [true, null]}}}));
    await page.locator('#run').click();
    await page.waitForFunction(() => document.querySelector('pre').textContent.startsWith('{'));
    assert.deepEqual(JSON.parse(await page.locator('pre').textContent()), {arg: '浏览器参数', input: {number: 42, nested: [true, null]}});
  }
  assert.deepEqual(errors, []);
  console.log('Chromium: independent artifact eval, typed call' + (entryArtifact ? ' and Eval context' : '') + ' passed');
} finally {
  await browser.close();
}
