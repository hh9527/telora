import { load } from './host.mjs';
self.onmessage = async ({ data: { bytes, arguments: args } }) => {
  try {
    const session = await load(bytes);
    session.initialize();
    self.postMessage({ result: args === null ? session.eval() : session.call(args) });
  } catch (error) {
    self.postMessage({ error: String(error) });
  }
};
