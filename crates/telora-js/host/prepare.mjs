import {pathToFileURL} from 'node:url';
import {writeFileSync} from 'node:fs';
const model=await import(pathToFileURL(process.argv[2]).href);
const service=process.argv[4]==='service';
let runtime=model.default;
if(service){
  if(!Array.isArray(runtime)||typeof runtime[1]!=='function')throw Error('Expected Service plan');
  if(runtime[0]?.length)throw Error('Staged JS rejects runtime Service sources');
  runtime=runtime[1](Object.freeze({sources:Object.freeze({})}));
}
writeFileSync(process.argv[3],JSON.stringify(({schema:"telora.js.archive/v1",image:model.stagePack(runtime),information:model.buildInformation})));
