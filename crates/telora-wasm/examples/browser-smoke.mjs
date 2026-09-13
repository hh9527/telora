// Run against a local HTTP server for this directory; Playwright is optional tooling.
import assert from 'node:assert/strict';
const { chromium } = await import(process.env.TELORA_PLAYWRIGHT_MODULE ?? 'playwright');
const [base, aggregateArtifact, inputArtifact, entryArtifact, rejectedArtifact, capturedArtifact] = process.argv.slice(2);
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
  assert.deepEqual(JSON.parse(await page.locator('pre[role=status]').textContent()), [42, [
    { label: '短文本', score: 19 },
    { label: 'a longer string stored in the string table', score: 23 },
  ], null]);
  await page.getByLabel('Wasm 文件').setInputFiles(inputArtifact);
  await page.getByLabel('参数数组').fill(JSON.stringify([{ name: '浏览器输入', values: [22] }]));
  await page.locator('#run').click();
  await page.waitForFunction(() => document.querySelector('pre').textContent.startsWith('{'));
  assert.deepEqual(JSON.parse(await page.locator('pre[role=status]').textContent()), { name: '浏览器输入', total: 42 });
  if (entryArtifact) {
    await page.getByLabel('Wasm 文件').setInputFiles(entryArtifact);
    await page.getByLabel('参数数组').fill(JSON.stringify({args: ['浏览器参数'], env: {}, sources: {input: {number: 42, nested: [true, null]}}}));
    await page.locator('#run').click();
    await page.waitForFunction(() => document.querySelector('pre').textContent.startsWith('{'));
    assert.deepEqual(JSON.parse(await page.locator('pre[role=status]').textContent()), {arg: '浏览器参数', input: {number: 42, nested: [true, null]}});
  }
  if (rejectedArtifact) {
    await page.getByLabel('Wasm 文件').setInputFiles(rejectedArtifact);
    await page.getByLabel('参数数组').fill('');
    await page.locator('#run').click();
    await page.waitForFunction(() => document.querySelector('pre[role=status]').textContent.includes('positive required'));
    const diagnostics = await page.locator('#diagnostics').textContent();
    assert.match(diagnostics, /warning: checker initialized/);
    assert.match(diagnostics, /error: positive required/);
    assert.equal(diagnostics.split('\n').length, 2);
  }
  if (capturedArtifact) {
    await page.getByLabel('Wasm 文件').setInputFiles(capturedArtifact);
    await page.getByLabel('参数数组').fill('');
    await page.locator('#run').click();
    await page.waitForFunction(() => document.querySelector('pre[role=status]').textContent.startsWith('['));
    const reports = JSON.parse(await page.locator('pre[role=status]').textContent());
    assert.deepEqual(reports.map(report => report.message), ['before failure', 'captured failure']);
    assert.equal(reports[1].labels.length, 2);
    assert.equal(reports[1].labels[1].source, '@src/main');
    assert.equal(reports[1].labels[1].primary, false);
    assert.equal((await page.locator('#diagnostics').textContent()).trim(), '');
  }
  assert.deepEqual(errors, []);
  console.log('Chromium: independent artifact eval, typed call' + (entryArtifact ? ', Eval context' : '') + (rejectedArtifact ? ', check diagnostics' : '') + (capturedArtifact ? ', captured diagnostics' : '') + ' passed');
} finally {
  await browser.close();
}
