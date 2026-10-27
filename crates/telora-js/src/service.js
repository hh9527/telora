// The host may instantiate multiple isolated service states from one module.
export function createService(sources = {}) {
  if (!Array.isArray(entry) || typeof entry[1] !== 'function')
    throw new TypeError('Selected export is not a Telora Service plan');
  const valueType = findType('std/value::Value');
  if(valueType<0)throw new TypeError('Service Value type is missing');
  const context=immutable({sources:immutable(Object.fromEntries(Object.entries(sources).map(([k,v])=>[k,valueFromJS(valueType,v)])))});
  const handler=entry[1](context);
  return (method,input) => JSON.parse(handler(valueFromJS(valueType,hostJSON({method,input}))));
}

function hostJSON(value) {
  if(typeof value==='number' && Number.isSafeInteger(value)) return BigInt(value);
  if(Array.isArray(value))return value.map(hostJSON);
  if(value!==null && typeof value==='object')return Object.fromEntries(Object.entries(value).map(([k,v])=>[k,hostJSON(v)]));
  return value;
}
