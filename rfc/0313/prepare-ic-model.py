# Creates isolated variants only; the source workspace is read-only.
from pathlib import Path
import argparse, shutil, difflib, json
parser = argparse.ArgumentParser()
parser.add_argument("--source", type=Path, required=True)
parser.add_argument("--output", type=Path, required=True)
args = parser.parse_args()
source = args.source.resolve()
root = args.output.resolve()
if root == source or root.is_relative_to(source) or source.is_relative_to(root):
    parser.error("source and output must be separate directory trees")
root.mkdir(parents=True, exist_ok=True)
ignore = shutil.ignore_patterns('.git', 'bin', 'target', 'node_modules', '.artifacts')
shutil.copytree(source, root/'baseline', ignore=ignore)
shutil.copytree(root/'baseline', root/'seq')

def closing(s, start):
    depth=0; quote=None; comment=False; escape=False
    for i in range(start,len(s)):
        c=s[i]
        if comment:
            if c=='\n': comment=False
            continue
        if quote:
            if escape: escape=False
            elif c=='\\': escape=True
            elif c==quote: quote=None
            continue
        if c=='#': comment=True; continue
        if c in '\"`': quote=c; continue
        if c=='(': depth+=1
        elif c==')':
            depth-=1
            if depth==0: return i
    raise ValueError('unclosed call')

def arguments(s):
    depth=0; quote=None; escape=False
    for i,c in enumerate(s):
        if quote:
            if escape: escape=False
            elif c=='\\': escape=True
            elif c==quote: quote=None
            continue
        if c in '\"`': quote=c; continue
        if c in '([{': depth+=1
        elif c in ')]}': depth-=1
        elif c==',' and depth==0: return s[:i],s[i+1:]
    raise ValueError('missing comma')

def rewrite(s, outer):
    prefix=f'array::{outer}(array::filter('
    n=0
    while (pos:=s.find(prefix))>=0:
        outer_open=pos+len(f'array::{outer}')
        inner_open=pos+len(prefix)-1
        inner_close=closing(s,inner_open)
        outer_close=closing(s,outer_open)
        inp,pred=arguments(s[inner_open+1:inner_close])
        filtered=f'seq::filter(seq::from_array({inp}),{pred})'
        tail=s[inner_close+1:outer_close]
        if outer=='length':
            assert not tail.strip()
            replacement=f'seq::count({filtered})'
        else:
            assert tail.lstrip().startswith(',')
            replacement=f'seq::collect(seq::map({filtered}{tail}))'
        s=s[:pos]+replacement+s[outer_close+1:]; n+=1
    return s,n

stats={}
for p in (root/'seq'/'ontology'/'src').glob('*.telora'):
    s=p.read_text(); old=s
    s,ncount=rewrite(s,'length');s,nmap=rewrite(s,'map')
    if s!=old:
        s='use std::seq as seq;\n'+s
        p.write_text(s);stats[p.name]={'count_filter':ncount,'map_filter':nmap}
shutil.copytree(root/'seq',root/'application')
p=root/'application'/'ontology'/'src'/'ontology.telora';s=p.read_text()
s=s.replace('value_type_brief(ty).id == target.id','match property::get_type_prop(ty, ValueTypeDef.type) { Some(spec) => spec.id == target.id, None => False }')
s=s.replace('let spec = value_type_brief(ty);\n            spec.id == target.owner && array::any(spec.values, fn(value) { value.id == target.id })', '''let spec = match property::get_type_prop(ty, ValueTypeDef.type) { Some(spec) => spec, None => fail!("value type declaration is missing", ty) };
            spec.id == target.owner && array::any(value_type_mapping(ty), fn(value) { value.id == target.id })''')
s=s.replace('let spec = value_type_brief(ty);\n        knowledge_target_declared(payload, target)', '''let spec = match property::get_type_prop(ty, ValueTypeDef.type) { Some(spec) => spec, None => fail!("value type declaration is missing", ty) };
        knowledge_target_declared(payload, target)''')
p.write_text(s)
p=root/'application'/'ontology'/'src'/'knowledge.telora';s=p.read_text()
a=s.index('        let summaries = array::fold(');b=s.index('        let related =',a)
s=s[:a]+'''        let summaries = dict::from_pairs(array::map(array::flat_map(payload.entities, fn(entity) {
            match property::get_type_prop(entity.entity_type, edsl::KnowledgeDocs.type) {
                Some(declared) => declared.entries, None => [],
            }
        }), fn(note) { (target_key(note.target), note.text) }));
'''+s[b:]
p.write_text(s)
for variant in ['seq','application']:
    patch=[]
    for base in (root/'baseline'/'ontology'/'src').glob('*.telora'):
        other=root/variant/'ontology'/'src'/base.name
        patch.extend(difflib.unified_diff(base.read_text().splitlines(True),other.read_text().splitlines(True),fromfile=f'a/ontology/src/{base.name}',tofile=f'b/ontology/src/{base.name}'))
    (root/f'{variant}.patch').write_text(''.join(patch))
(root/'rewrite-counts.json').write_text(json.dumps(stats,indent=2)+'\n')
print(json.dumps(stats))
