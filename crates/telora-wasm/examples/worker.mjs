import { load } from './host.mjs';
self.onmessage = async ({ data: { bytes, arguments: args } }) => {
  let session;
  try {
    session = await load(bytes);
    session.setDebugEnabled(true);
    session.initialize();
    if (args !== null && !Array.isArray(args)) throw Error('参数必须是数组');
    const result = args === null ? session.eval() : session.call(args);
    self.postMessage({ result, diagnostics: session.diagnostics(), debug: session.debugEvents() });
  } catch (error) {
    self.postMessage({ error: String(error), diagnostics: session?.diagnostics() ?? [], debug: session?.debugEvents() ?? [] });
  }
};
