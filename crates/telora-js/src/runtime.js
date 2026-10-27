// Private runtime helpers linked into the single generated module.
const none = () => variant(0), some = x => variant(1,x), err = x => variant(0,x), ok = x => variant(1,x);
const immutable = x => Object.freeze(x);
const dictKeys = x => Object.keys(x).sort();
const natives = {
  '1:equal': (s,a,b) => equal(a,b),
  '5:length': (s,a) => BigInt(a.length),
  '5:get': (s,a,i) => i>=0n && i<BigInt(a.length) ? some(a[Number(i)]) : none(),
  '5:enumerate': (s,a) => immutable(a.map((v,i)=>immutable([BigInt(i),v]))),
  '5:push': (s,a,v) => immutable([...a,v]),
  '5:concat': (s,a) => immutable(a.flat()),
  '5:zip': (s,a,b) => a.length===b.length ? some(immutable(a.map((v,i)=>immutable([v,b[i]])))) : none(),
  '5:map': (s,a,f) => immutable(a.map(v=>f(v))),
  '5:filter': (s,a,f) => immutable(a.filter(v=>f(v))),
  '5:flat_map': (s,a,f) => immutable(a.flatMap(v=>f(v))),
  '5:fold': (s,a,z,f) => a.reduce((r,v)=>f(r,v),z),
  '5:fold_control': (s,a,z,f) => { for(const v of a){const r=f(z,v); if(r.tag===1)return r; z=r.value;} return variant(0,z); },
  '5:any': (s,a,f) => a.some(v=>f(v)),
  '5:all': (s,a,f) => a.every(v=>f(v)),
  '5:find': (s,a,f) => {for(const v of a)if(f(v))return some(v); return none();},
  '6:keys': (s,a) => immutable(dictKeys(a)),
  '6:values': (s,a) => immutable(dictKeys(a).map(k=>a[k])),
  '6:pairs': (s,a) => immutable(dictKeys(a).map(k=>immutable([k,a[k]]))),
  '6:from_pairs': (s,a) => immutable(Object.fromEntries(a)),
  '6:merge': (s,a,b) => immutable({...a,...b}),
  '6:map_values': (s,a,f) => immutable(Object.fromEntries(dictKeys(a).map(k=>[k,f(a[k])]))),
  '6:filter': (s,a,f) => immutable(Object.fromEntries(dictKeys(a).filter(k=>f(a[k])).map(k=>[k,a[k]]))),
  '6:fold': (s,a,z,f) => dictKeys(a).reduce((r,k)=>f(r,k,a[k]),z),
  '6:get': (s,a,k) => Object.hasOwn(a,k)?some(a[k]):none(),
  '7:length': (s,a) => BigInt(new TextEncoder().encode(a).length),
  '7:join': (s,a,sep) => a.join(sep),
  '7:join_lines': (s,a) => a.join('\n'),
  '7:split': (s,a,sep) => immutable(sep===''?['',...a,'']:a.split(sep)),
  '7:lines': (s,a) => immutable(a.split(/\r?\n/).filter((v,i,x)=>i<x.length-1||v!=='')),
  '7:starts_with': (s,a,b) => a.startsWith(b),
  '7:ends_with': (s,a,b) => a.endsWith(b),
  '7:contains': (s,a,b) => a.includes(b),
  '7:replace': (s,a,b,c) => a.replaceAll(b,c),
  '7:indent': (s,a,n) => a.split('\n').map(x=>' '.repeat(Number(n))+x).join('\n'),
  '7:ensure_trailing_newline': (s,a) => a.endsWith('\n')?a:a+'\n',
  '7:trim_margin': (s,a,m) => a.split('\n').map(x=>{const i=x.indexOf(m);return i<0?x:x.slice(i+m.length);}).join('\n'),
};
const native = (key,signature) => {
  const impl = natives[key];
  if(!impl) throw Error(`Telora JS native not implemented: ${key}`);
  return (...args) => impl(signature,...args);
};
class TeloraFailure extends Error {}
const failure = (message,locs=[]) => { const e=new TeloraFailure(typeof message==='string'?message:(message?.message??String(message))); e.locs=message?.locs??locs; throw e; };
const diagnosticFrames=[];
const warning=(message,locs=[])=>{diagnosticFrames.at(-1)?.push(immutable({message:typeof message==='string'?message:message.message,severity:variant(2),locs:immutable(message?.locs??locs)}));return none();};
const escape = (value,scope) => {throw {teloraReturn:true,value,scope};};
const propagate = (value,scope) => value.tag===1 ? value.value : escape(value,scope);
const boundary = (scope,f) => {try{return f();}catch(e){if(e?.teloraReturn && e.scope===scope)return e.value;throw e;}};
const bodyType = ty => {const t=types[ty]; return t.kind==='Ref' && t.body!==null ? t.body : ty;};
const typeMembers = ty => types[ty].fields.length ? types[ty].fields : types[bodyType(ty)].fields;
const typeVariants = ty => types[ty].variants.length ? types[ty].variants : types[bodyType(ty)].variants;
const enumNamed = (ty,name,value) => {
  const index=typeVariants(ty).findIndex(v=>v.name===name);
  if(index<0)throw Error(`Telora missing variant ${name} in type ${ty}`);
  return variant(index,value);
};
const returnType = signature => types[signature].args.at(-1);
const property = (owner,site,prop,required=false) => {
  const value=properties[`${owner}:${site}:${prop}`];
  if(!value){if(required)failure('Required property absent');return none();}
  return required?value():some(value());
};
const propertyContext = (owner,index,isVariant) => {
  const member=(isVariant?typeVariants(owner):typeMembers(owner))[index];
  return immutable({owner,index:BigInt(index),name:member.name,ty:member.ty,payload:member.ty===null?none():some(member.ty)});
};
const check = (owner,site,value) => {
  for(const getter of checkers[`${owner}:${site}`]??[]) {
    const result=getter()(value);
    if(result.tag===0)failure(result.value??'Construction check failed');
  }
  return value;
};
Object.assign(natives,{
  '3:kind': (s,ty) => enumNamed(returnType(s),types[ty].kind),
  '3:children': (s,ty) => immutable(types[ty].args.slice()),
  '3:fields': (s,ty) => immutable(typeMembers(ty).map((f,i)=>immutable({index:BigInt(i),name:f.name,ty:f.ty}))),
  '3:variants': (s,ty) => immutable(typeVariants(ty).map((v,i)=>immutable({index:BigInt(i),name:v.name,payload:v.ty===null?none():some(v.ty)}))),
  '3:name': (s,ty) => types[ty].name===null?none():some(types[ty].name),
  '3:opaque_name': (s,ty) => types[ty].name===null?none():some(types[ty].name),
  '3:resolve_raw': (s,ty) => types[ty].body===null?err('Type has no structural body'):ok(types[ty].body),
  '25:get_type_prop': (s,owner,p) => property(owner,'type',p),
  '25:get_field_prop': (s,owner,index,p) => property(owner,`field:${index}`,p),
  '25:get_variant_prop': (s,owner,index,p) => property(owner,`variant:${index}`,p),
  '25:evidence': (s,owner,p) => property(owner,'type',p,true),
  '18:property': (s,target) => (owner,prior) => immutable({bits:BigInt([4,16,8,2,1,32][target.tag]) | (prior.tag===1?prior.value.bits:0n)}),
  '38:new_slots': () => new Map(),
  '38:lookup': (s,slots,ty,calc) => {if(!slots.has(ty))slots.set(ty,memo(()=>calc(ty)));return slots.get(ty)();},
  '39:new_run': () => immutable({}),
  '39:new_slots': () => new WeakMap(),
  '39:memo': (s,slots,run,calc) => {if(!slots.has(run))slots.set(run,memo(calc));return slots.get(run)();},
  '39:create_mut_builder': (s,plan) => ({value:Array.isArray(plan.initial)?[...plan.initial]:new Map(Object.entries(plan.initial)),sealed:false}),
  '39:extend': (s,b,item) => {
    if(b.sealed)failure('Builder already finished');
    if(Array.isArray(b.value)){b.value.push(item);return true;}
    const [key,value]=item.value;
    if(item.tag===0){if(b.value.has(key))return false;b.value.set(key,value());}
    else b.value.set(key,value);
    return true;
  },
  '39:finish': (s,b) => {if(b.sealed)failure('Builder already finished');b.sealed=true;return immutable(Array.isArray(b.value)?b.value.slice():Object.fromEntries(b.value));},
  '2:pack': (s,ty,value) => immutable({ty,value}),
  '2:project_with': (s,ty,dyn) => ty===dyn.ty?some(dyn.value):none(),
  '2:desc': (s,dyn) => dyn.ty,
  '2:get_field_value': (s,dyn,i) => {const field=typeMembers(dyn.ty)[Number(i)];if(!field)failure('Invalid field index');return immutable({ty:field.ty,value:dyn.value[field.name]});},
  '2:get_variant_index': (s,dyn) => BigInt(dyn.value.tag),
  '2:get_variant_payload': (s,dyn,index) => {
    const v=typeVariants(dyn.ty)[Number(index)];
    return dyn.value.tag===Number(index)&&v?.ty!==null?some(immutable({ty:v.ty,value:dyn.value.value})):none();
  },
  '2:from_dyn_fields': (s,ty,fields) => {
    const members=typeMembers(ty);
    if(members.length!==fields.length)return err(enumNamed(types[returnType(s)].args[1],'Count',immutable([BigInt(members.length),BigInt(fields.length)])));
    const out={};for(let i=0;i<members.length;i++){if(members[i].ty!==fields[i].ty)return err(enumNamed(types[returnType(s)].args[1],'Field',immutable([BigInt(i),members[i].ty,fields[i].ty])));out[members[i].name]=fields[i].value;}
    return ok(check(ty,'type',immutable(out)));
  },
  '26:build_time_active': () => true,
  '26:publish_service_metadata': (s,key,text) => {buildInformation[key]=JSON.parse(text);return true;},
  '26:publish_build_information': (s,key,text) => {if(!key||Object.hasOwn(buildInformation,key))return false;buildInformation[key]=JSON.parse(text);return true;},
  '26:call_with_diagnostics': (s,f,a) => {const diagnostics=[];diagnosticFrames.push(diagnostics);try{return ok(immutable([f(a),immutable(diagnostics)]));}catch(e){if(!(e instanceof TeloraFailure))throw e;diagnostics.push(immutable({message:e.message,severity:variant(0),locs:immutable(e.locs??[])}));return err(immutable(diagnostics));}finally{diagnosticFrames.pop();}},
  '26:service_reply': (s,text,failed,diagnostics) => jsonText({schema:'telora.service/v1',ok:failed?null:JSON.parse(text,(_,v,c)=>typeof v==='number'&&c?.source&&/^-?\d+$/.test(c.source)?BigInt(c.source):v),error:failed,diagnostics:encodeTyped(types[s].args[2],diagnostics)}),
  '34:message': (s,error) => error.message,
  '34:with_message': (s,error,message) => immutable({...error,message}),
});
const buildInformation=Object.create(null);
const jsonText = value => {
  if(typeof value==='bigint')return value.toString();
  if(value===null || typeof value!=='object')return JSON.stringify(value);
  if(Array.isArray(value))return '['+value.map(jsonText).join(',')+']';
  return '{'+dictKeys(value).map(k=>JSON.stringify(k)+':'+jsonText(value[k])).join(',')+'}';
};
const valueFromJS = (ty,value) => {
  if(value===null)return enumNamed(ty,'None');
  if(typeof value==='boolean')return enumNamed(ty,value?'True':'False');
  if(typeof value==='bigint')return enumNamed(ty,'Int',i64(value));
  if(typeof value==='number')return enumNamed(ty,'Float',value);
  if(typeof value==='string')return enumNamed(ty,'String',value);
  if(Array.isArray(value))return enumNamed(ty,'Array',immutable(value.map(v=>valueFromJS(ty,v))));
  return enumNamed(ty,'Object',immutable(Object.fromEntries(dictKeys(value).map(k=>[k,valueFromJS(ty,value[k])]))));
};
const valueToJS = (ty,value) => {
  const name=typeVariants(ty)[value.tag].name;
  switch(name){case 'None':return null;case 'True':return true;case 'False':return false;
    case 'Array':return value.value.map(v=>valueToJS(ty,v));
    case 'Object':return Object.fromEntries(dictKeys(value.value).map(k=>[k,valueToJS(ty,value.value[k])]));
    default:return value.value;
  }
};
const findType = name => types.findIndex(t=>t.name===name);
const optionalProp = (ty,name) => {
  const prop=findType(name);if(prop<0)return null;
  const value=property(ty,'type',prop);return value.tag===1?value.value:null;
};
const camel = text => {let upper=false;return [...text].map((c,i)=>{if(c==='_'){upper=true;return '';}if(upper){upper=false;return c.toUpperCase();}return i===0?c.toLowerCase():c;}).join('');};
const codecNames = (ty,members) => {
  const rename=optionalProp(ty,'std/_codec::CodecProp')?.rename_all.tag===1;
  const names=members.map(m=>rename?camel(m.name):m.name);
  if(new Set(names).size!==names.length)failure('Duplicate external codec name');
  return names;
};
const parseTarget = (ty,text) => {
  const t=types[ty];
  if(t.constructor==='String')return text;
  if(t.constructor==='Int'){if(!/^[+-]?\d+$/.test(text))failure('Invalid Int');return i64(BigInt(text));}
  if(t.constructor==='Float'){if(text.trim()!==text||text==='')failure('Invalid Float');const n=Number(text);if(Number.isNaN(n))failure('Invalid Float');return n;}
  if(t.constructor==='Option')return text===''?none():some(parseTarget(t.args[0],text));
  const capability=optionalProp(ty,'std/regex::ParseBy');
  if(capability){
    const matched=capability.regex.exec(text);if(!matched)failure('Text does not match ParseBy');
    const fields=typeMembers(ty),out={};
    for(const field of fields){const capture=matched.groups?.[field.name];
      if(capture===undefined && types[field.ty].constructor!=='Option')failure('Missing capture '+field.name);
      out[field.name]=capture===undefined?none():parseTarget(field.ty,capture);
    }
    return check(ty,'type',immutable(out));
  }
  failure('Type has no parse capability');
};
const encodeTyped = (ty,value) => {
  if(value===undefined)throw Error(`Undefined value encoding type ${ty} (${types[ty].name??types[ty].constructor})`);
  const t=types[ty];
  if(t.name==='std/value::Value')return valueToJS(ty,value);
  if(optionalProp(ty,'std/string::EncodeByDisplay'))return displayTyped(ty,value);
  switch(t.constructor){
    case 'Int':case 'Float':case 'String':case 'Bool':return value;
    case 'Option':return value.tag===0?null:encodeTyped(t.args[0],value.value);
    case 'Array':return value.map(v=>encodeTyped(t.args[0],v));
    case 'Dict':return Object.fromEntries(dictKeys(value).map(k=>[k,encodeTyped(t.args[0],value[k])]));
    case 'Tuple':return t.args.length===0?null:value.map((v,i)=>encodeTyped(t.args[i],v));
  }
  if(types[bodyType(ty)].kind==='Newtype')return encodeTyped(types[bodyType(ty)].args[0],value);
  const members=typeMembers(ty),variants=typeVariants(ty);
  if(variants.length){const v=variants[value.tag],names=codecNames(ty,variants);if(optionalProp(ty,'std/_codec::CodecProp')?.untagged)return v.ty===null?null:encodeTyped(v.ty,value.value);
    return v.ty===null?names[value.tag]:{[names[value.tag]]:encodeTyped(v.ty,value.value)};
  }
  if(members.length||t.kind==='Struct'||types[bodyType(ty)].kind==='Struct'){
    const names=codecNames(ty,members);return Object.fromEntries(members.map((m,i)=>[names[i],encodeTyped(m.ty,value[m.name])]));
  }
  if(t.body!==null)return encodeTyped(t.body,value);
  failure('Unsupported encode type '+ty);
};
const decodeTyped = (ty,value,path='$') => {
  const t=types[ty],reject=message=>failure(path+': '+message);
  if(t.name==='std/value::Value')return valueFromJS(ty,value);
  if(optionalProp(ty,'std/string::DecodeByParse')){if(typeof value!=='string')reject('expected String');return parseTarget(ty,value);}
  switch(t.constructor){
    case 'Int':if(typeof value!=='bigint')reject('expected Int');return i64(value);
    case 'Float':if(typeof value!=='number'&&typeof value!=='bigint')reject('expected Float');return Number(value);
    case 'String':if(typeof value!=='string')reject('expected String');return value;
    case 'Bool':if(typeof value!=='boolean')reject('expected Bool');return value;
    case 'Option':return value===null?none():some(decodeTyped(t.args[0],value,path));
    case 'Array':if(!Array.isArray(value))reject('expected Array');return immutable(value.map((v,i)=>decodeTyped(t.args[0],v,path+'['+i+']')));
    case 'Dict':if(value===null||typeof value!=='object'||Array.isArray(value))reject('expected Object');return immutable(Object.fromEntries(dictKeys(value).map(k=>[k,decodeTyped(t.args[0],value[k],path+'.'+k)])));
    case 'Tuple':if(t.args.length===0){if(value!==null)reject('expected None');return immutable([]);}if(!Array.isArray(value)||value.length!==t.args.length)reject('expected Tuple');return immutable(value.map((v,i)=>decodeTyped(t.args[i],v,path+'['+i+']')));
  }
  if(types[bodyType(ty)].kind==='Newtype')return check(ty,'type',decodeTyped(types[bodyType(ty)].args[0],value,path));
  const members=typeMembers(ty),variants=typeVariants(ty);
  if(variants.length){
    if(optionalProp(ty,'std/_codec::CodecProp')?.untagged){for(const v of variants){try{return check(ty,'type',variant(v.index,v.ty===null?(value===null?undefined:reject('expected None')):decodeTyped(v.ty,value,path)));}catch(e){if(!(e instanceof TeloraFailure))throw e;}}reject('no untagged variant matches');}
    const names=codecNames(ty,variants);let i;
    if(typeof value==='string'){i=names.indexOf(value);if(i<0||variants[i].ty!==null)reject('expected declared enum variant');return check(ty,'type',variant(i));}
    if(value===null||typeof value!=='object'||Array.isArray(value)||Object.keys(value).length!==1)reject('expected declared enum variant');
    const key=Object.keys(value)[0];i=names.indexOf(key);if(i<0||variants[i].ty===null)reject('expected declared enum variant');
    return check(ty,'type',variant(i,check(ty,'variant:'+i,decodeTyped(variants[i].ty,value[key],path+'.'+key))));
  }
  if(members.length||t.kind==='Struct'||types[bodyType(ty)].kind==='Struct'){
    if(value===null||typeof value!=='object'||Array.isArray(value))reject('expected Object');
    const names=codecNames(ty,members),out={};
    for(const key of Object.keys(value))if(!names.includes(key))failure(path+'.'+key+': unknown field');
    for(let i=0;i<members.length;i++){const m=members[i],key=names[i];if(!Object.hasOwn(value,key)&&types[m.ty].constructor!=='Option')failure(path+'.'+key+': missing required field');out[m.name]=check(ty,'field:'+i,decodeTyped(m.ty,Object.hasOwn(value,key)?value[key]:null,path+'.'+key));}
    return check(ty,'type',immutable(out));
  }
  if(t.body!==null)return check(ty,'type',decodeTyped(t.body,value,path));
  reject('unsupported decode type '+ty);
};
const displayTyped = (ty,value) => {
  if(['Int','Float','String'].includes(types[ty].constructor))return String(value);
  const display=optionalProp(ty,'std/fmt::DisplayBy');if(display)return display.display(immutable({ty,value}));
  failure('No Display capability for '+ty);
};
Object.assign(natives,{
  '19:compile': (s,source) => new RegExp(source.replaceAll('(?P<','(?<'),'u'),
  '19:is_match': (s,regex,text) => regex.test(text),
  '19:parse_by': (s,regex) => (owner,prior) => immutable({regex}),
  '27:parse_with': (s,ty,text) => {try{return ok(parseTarget(ty,text));}catch(e){if(e instanceof TeloraFailure||e instanceof RangeError)return err(e.message);throw e;}},
  '17:parse_raw': (s,ty,text) => {try{return ok(valueFromJS(ty,JSON.parse(text,(key,v,ctx)=>typeof v==='number'&&ctx?.source&&/^-?\d+$/.test(ctx.source)?BigInt(ctx.source):v)));}catch(e){return err(immutable({message:e.message}));}},
  '17:stringify': (s,value) => jsonText(valueToJS(types[s].args[0],value)),
  '17:stringify_pretty': (s,indent) => value => JSON.stringify(valueToJS(types[returnType(s)].args[0],value),(_,v)=>typeof v==='bigint'?JSON.rawJSON(v.toString()):v,Number(indent)),
  '13:encode_with': (s,prop,ty,value) => valueFromJS(ty,encodeTyped(types[s].args[2],value)),
  '13:decode_with': (s,prop,ty,value) => {try{return ok(decodeTyped(ty,valueToJS(types[s].args[2],value)));}catch(e){if(e instanceof TeloraFailure||e instanceof RangeError)return err(immutable({message:e.message}));throw e;}},
  '20:from_string': (s,x) => x,
  '20:from_int': (s,x) => String(x),
  '20:from_float': (s,x) => String(x),
  '20:render': (s,x) => x,
  '20:concat': (s,strings,fields) => strings.map((v,i)=>v+(fields[i]??'')).join(''),
  '20:prepare': (s,source) => {const strings=[],fields=[];let start=0;const regex=/\{([^{}]+)\}/g;for(const match of source.matchAll(regex)){strings.push(source.slice(start,match.index));fields.push(match[1]);start=match.index+match[0].length;}strings.push(source.slice(start));return immutable([immutable(strings),immutable(fields)]);},
});
const dynFields = dyn => {
  const t=types[bodyType(dyn.ty)];
  if(t.constructor==='Dict')return ok(immutable(dictKeys(dyn.value).map(k=>immutable([k,immutable({ty:t.args[0],value:dyn.value[k]})]))));
  if(t.kind!=='Struct')return err('Dyn field access expects Struct');
  return ok(immutable(typeMembers(dyn.ty).map(f=>immutable([f.name,immutable({ty:f.ty,value:dyn.value[f.name]})]))));
};
Object.assign(natives,{
  '35:encode_component': (s,text) => encodeURIComponent(text).replace(/[!'()*]/g,c=>'%'+c.charCodeAt(0).toString(16).toUpperCase()),
  '35:decode_component': (s,text) => {try{return some(decodeURIComponent(text));}catch{return none();}},
  '2:kind': (s,dyn) => {
    const t=types[bodyType(dyn.ty)];
    let kind=t.kind;
    if(kind==='Struct')kind='Dict';if(kind==='Newtype')kind='Tuple';if(kind==='TypeOf')kind='Type';
    if(kind==='Enum')kind=typeVariants(dyn.ty)[typeof dyn.value==='boolean'?Number(dyn.value):dyn.value.tag]?.ty===null?'Atom':'Tagged';
    return enumNamed(returnType(s),kind);
  },
  '2:check_int': (s,dyn) => types[dyn.ty].constructor==='Int'?some(dyn.value):none(),
  '2:check_float': (s,dyn) => types[dyn.ty].constructor==='Float'?some(dyn.value):none(),
  '2:check_string': (s,dyn) => types[dyn.ty].constructor==='String'?some(dyn.value):none(),
  '2:check_bytes': (s,dyn) => types[dyn.ty].constructor==='Bytes'?some(dyn.value):none(),
  '2:fields_raw': (s,dyn) => dynFields(dyn),
  '2:field_raw': (s,dyn,name) => {const fields=dynFields(dyn);if(fields.tag===0)return fields;const found=fields.value.find(f=>f[0]===name);return found?ok(found[1]):err('Missing Dyn field');},
  '2:array_items_raw': (s,dyn) => types[dyn.ty].constructor==='Array'?ok(immutable(dyn.value.map(v=>immutable({ty:types[dyn.ty].args[0],value:v})))):err('Dyn sequence access has the wrong type'),
  '2:tuple_items_raw': (s,dyn) => {
    const t=types[bodyType(dyn.ty)];
    if(t.constructor==='Tuple')return ok(immutable(dyn.value.map((v,i)=>immutable({ty:t.args[i],value:v}))));
    if(t.kind==='Newtype')return ok(immutable([immutable({ty:t.args[0],value:dyn.value})]));
    return err('Dyn tuple access has the wrong type');
  },
  '2:tag_raw': (s,dyn) => {const t=types[bodyType(dyn.ty)];if(t.kind!=='Enum')return err('Dyn tag expects Enum');return ok(typeVariants(dyn.ty)[typeof dyn.value==='boolean'?Number(dyn.value):dyn.value.tag].name);},
  '2:payload_raw': (s,dyn) => {const t=types[bodyType(dyn.ty)];if(t.kind!=='Enum')return err('Dyn payload expects Enum');const v=typeVariants(dyn.ty)[typeof dyn.value==='boolean'?Number(dyn.value):dyn.value.tag];return ok(v.ty===null?none():some(immutable({ty:v.ty,value:dyn.value.value})));},
});
const sha256Bytes = input => {
  const k=[0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2];
  const h=[0x6a09e667,0xbb67ae85,0x3c6ef372,0xa54ff53a,0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19];
  const bytes=Array.from(input),length=BigInt(bytes.length)*8n;bytes.push(128);while(bytes.length%64!==56)bytes.push(0);for(let i=7;i>=0;i--)bytes.push(Number(length>>BigInt(i*8)&255n));
  const r=(x,n)=>(x>>>n)|(x<<(32-n));
  for(let offset=0;offset<bytes.length;offset+=64){const w=new Int32Array(64);for(let i=0;i<16;i++)w[i]=(bytes[offset+4*i]<<24)|(bytes[offset+4*i+1]<<16)|(bytes[offset+4*i+2]<<8)|bytes[offset+4*i+3];
    for(let i=16;i<64;i++){const a=w[i-15],b=w[i-2];w[i]=w[i-16]+(r(a,7)^r(a,18)^(a>>>3))+w[i-7]+(r(b,17)^r(b,19)^(b>>>10));}
    let [a,b,c,d,e,f,g,z]=h;
    for(let i=0;i<64;i++){const t1=(z+(r(e,6)^r(e,11)^r(e,25))+((e&f)^(~e&g))+k[i]+w[i])|0,t2=((r(a,2)^r(a,13)^r(a,22))+((a&b)^(a&c)^(b&c)))|0;z=g;g=f;f=e;e=(d+t1)|0;d=c;c=b;b=a;a=(t1+t2)|0;}
    [a,b,c,d,e,f,g,z].forEach((v,i)=>h[i]=(h[i]+v)|0);
  }
  return immutable(h.flatMap(v=>[(v>>>24)&255,(v>>>16)&255,(v>>>8)&255,v&255]));
};
Object.assign(natives,{
  '16:sha256': (s,text) => sha256Bytes(new TextEncoder().encode(text)).map(v=>v.toString(16).padStart(2,'0')).join(''),
  '16:new': () => immutable([]),
  '16:update_bytes': (s,state,bytes) => immutable([...state,...bytes]),
  '16:update_string': (s,state,text) => immutable([...state,...new TextEncoder().encode(text)]),
  '16:update_int': (s,state,value) => immutable([...state,...Array.from({length:8},(_,i)=>Number(BigInt.asUintN(64,value)>>BigInt(i*8)&255n))]),
  '16:finish': (s,state) => sha256Bytes(state),
});
