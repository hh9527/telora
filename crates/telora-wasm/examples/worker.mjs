import { load } from './host.mjs';
self.onmessage = async ({ data: { bytes, arguments: args } }) => {
  let session;
  try {
    session = await load(bytes);
    session.initialize();
    const result = args === null ? session.eval() : Array.isArray(args) ? session.call(args) : session.evalWith(args);
    self.postMessage({ result, diagnostics: session.diagnostics() });
  } catch (error) {
    self.postMessage({ error: String(error), diagnostics: session?.diagnostics() ?? [] });
  }
};
