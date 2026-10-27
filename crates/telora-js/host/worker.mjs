// Runs only inside the disposable model process, never in the application host.
import {pathToFileURL} from 'node:url';
import {createInterface} from 'node:readline';
const started=performance.now();
const {createService}=await import(pathToFileURL(process.argv[2]).href);
const loaded=performance.now();
const transform=createService();
const initialized=performance.now();
const stats=()=>({memory:process.memoryUsage(),maxRssKiB:process.resourceUsage().maxRSS});
process.stdout.write(JSON.stringify({ready:true,pid:process.pid,loadMs:loaded-started,initializeMs:initialized-loaded,stats:stats()},(_,v)=>typeof v==='bigint'?JSON.rawJSON(v.toString()):v)+'\n');
const reader=createInterface({input:process.stdin});
for await(const line of reader){
  try{
    const parsed=JSON.parse(line,(_,value,context)=>typeof value==='number'&&context?.source&&/^-?\d+$/.test(context.source)?BigInt(context.source):value);
    const {method,input}=parsed,id=Number(parsed.id);
    const start=performance.now();
    const response=transform(method,input);
    process.stdout.write(JSON.stringify({id,response,transformMs:performance.now()-start,stats:stats()},(_,v)=>typeof v==='bigint'?JSON.rawJSON(v.toString()):v)+'\n');
  }catch(error){process.stderr.write(String(error.stack??error)+'\n');process.exitCode=1;break;}
}
