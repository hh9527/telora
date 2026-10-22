"""Create initialization workloads in /tmp; compile with each isolated compiler."""
import json,pathlib
root=pathlib.Path('/tmp/rfc0314-micro');root.mkdir(exist_ok=True)
(root/'telora-config.json').write_text(json.dumps({'version':1,'members':['.']}))
(root/'telora-crate.json').write_text(json.dumps({'name':'micro','dependencies':[]}))
(root/'telora-lock.json').write_text(json.dumps({'version':1,'packages':{'micro':{'source':{'workspace':''},'dependencies':[]}}}))
(root/'src').mkdir(exist_ok=True)
seed='['+','.join(map(str,range(64)))+']'
for name,pred in [('empty','x < 0'),('single','x == 0'),('many','x >= 0')]:
    expr=f'array::fold(seed, 0, fn(a, outer) {{ a + array::fold(seed, 0, fn(b, inner) {{ b + array::length(array::filter(seed, fn(x) {{ {pred} }})) }}) }})'
    source='''use std::array;
use std::transform_service as service;
use std::codec;
use std::value::{Value};
type Model = struct { total: Int };
impl service::TransformService for Model {
    init: fn(ctx) {
        let seed = SEED;
        { total: EXPR }.ty!(Self)
    },
    transform: fn(self, input) { codec::encode(self.total) },
};
@service::collection
pub type MainService = struct { @service::slot("bench") model: Model };
'''.replace('SEED',seed).replace('EXPR',expr)
    (root/'src'/f'{name}.telora').write_text(source)
# Parse many distinct long strings to exercise raw allocation and tracing.
text=json.dumps([f'long string content number {i:05d} and unique tail' for i in range(4096)])
source='''use std::json;
use std::codec;
use std::transform_service as service;
use std::value::{Value};
type Model = struct { data: Value };
impl service::TransformService for Model {
    init: fn(ctx) { { data: json::parse(TEXT).unwrap!() }.ty!(Self) },
    transform: fn(self, input) { self.data },
};
@service::collection
pub type MainService = struct { @service::slot("bench") model: Model };
'''.replace('TEXT','r#"'+text+'"#')
(root/'src'/'strings.telora').write_text(source)

(root/'src'/'lib.telora').write_text('pub mod empty;\npub mod single;\npub mod many;\npub mod strings;\n')
