use telora_core::{
    mir::*,
    module_resolve::{self, ModuleSpec},
    static_sources, symbol_resolve, type_resolve,
};

fn compile(source: &str) -> Result<String, String> {
    let inventory = std::iter::once("@src/main")
        .chain(static_sources::BUILTINS.iter().map(|(name, _)| *name))
        .map(|name| ModuleSpec {
            name: name.into(),
            kind: ModuleKind::Source,
            native: static_sources::native_module(name),
            implicit_imports: if name == "std/prelude" {
                vec![]
            } else {
                vec!["std/prelude".into()]
            },
        })
        .collect();
    let mut mir = module_resolve::resolve(inventory, &["@src/main".into()], |_, name| {
        if name == "@src/main" {
            Ok(source.to_owned())
        } else {
            static_sources::BUILTINS
                .iter()
                .find(|(module, _)| *module == name)
                .map(|(_, text)| (*text).to_owned())
                .ok_or_else(|| format!("unknown module {name}"))
        }
    });
    symbol_resolve::resolve(&mut mir);
    type_resolve::resolve(&mut mir);
    if !mir.diagnostics.is_empty() {
        return Err(mir
            .diagnostics
            .iter()
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n"));
    }
    let export = mir
        .exports
        .iter()
        .flatten()
        .copied()
        .find(|id| mir.symbols[id.index()].name == "answer")
        .ok_or("missing export")?;
    let executable = mir.seal_export(export).map_err(|ds| {
        ds.iter()
            .map(|d| mir.sources.render(d))
            .collect::<Vec<_>>()
            .join("\n")
    })?;
    telora_js::compile_executable(&executable)
}

fn run(source: &str, assertion: &str) {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let js = compile(source)
        .unwrap()
        .replace("export default ", "const answer = ");
    let mut child = Command::new("node")
        .args(["--input-type=module"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Node.js required");
    write!(child.stdin.take().unwrap(), "{js}\n{assertion}").unwrap();
    let result = child.wait_with_output().unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
#[test]
fn closures_globals_branches_and_exact_ints() {
    run(
        "def offset: Int = 7; pub def answer: Fn(Int) -> Int = fn(x: Int) { let y = x + offset; if y > 10 { y * 2 } else { y - 1 } };",
        "if(answer(5n)!==24n || answer(1n)!==7n) throw Error('result');",
    );
    run(
        "pub def answer: Int = 9007199254740993 + 2;",
        "if(answer!==9007199254740995n) throw Error('precision');",
    );
    run(
        "pub def answer: Fn(Int) -> Fn(Int) -> Int = fn(x: Int) { fn(y: Int) { x + y } };",
        "if(answer(4n)(5n)!==9n) throw Error('capture');",
    );
}
#[test]
fn recursion_and_overflow() {
    run(
        "pub def answer: Fn(Int) -> Int = fn(n: Int) { if n <= 1 { 1 } else { n * answer(n - 1) } };",
        "if(answer(10n)!==3628800n) throw Error('recursion');",
    );
    run(
        "pub def answer: Fn(Int) -> Int = fn(x: Int) { x + 1 };",
        "let failed=false; try { answer(9223372036854775807n); } catch(e) {failed=e instanceof RangeError;} if(!failed) throw Error('overflow');",
    );
    run(
        "pub def answer: Fn(Int) -> Int = fn(x: Int) { let unused = 1 / x; 42 };",
        "let failed=false; try {answer(0n);} catch(e) {failed=e instanceof RangeError;} if(!failed) throw Error('eager binding');",
    );
}
#[test]
fn aggregate_types_and_generic_instances() {
    run(
        "def id: for(T) Fn(T) -> T = fn(x) { x }; pub def answer: (Int, String) = (id(42), id(\"ok\"));",
        "if(answer[0]!==42n || answer[1]!=='ok') throw Error('generic');",
    );
    run(
        "type Pair = struct {a: Int, b: Int}; pub def answer: Int = {a: 2, b: 3}.ty!(Pair).a;",
        "if(answer!==2n) throw Error('record');",
    );
    run(
        "pub def answer: Int = match Some(42) { Some(x) => x, None => 0 };",
        "if(answer!==42n) throw Error('pattern');",
    );
}

#[test]
fn immutable_collections_and_callbacks() {
    run(
        "use std::array as a; pub def answer: Array(Int) = a::map([1,2,3], fn(x) {x * 2});",
        "if(answer.join(',')!=='2,4,6'||!Object.isFrozen(answer)) throw Error('map');",
    );
    run(
        "use std::dict as d; pub def answer: Int = match d::get({x:42}, \"x\") {Some(v)=>v,None=>0};",
        "if(answer!==42n) throw Error('dict');",
    );
}

#[test]
fn metadata_properties_and_type_memoization() {
    run(
        "use std::type_desc as t; type Pair = struct {a:Int,b:String}; pub def answer: Int = t::fields(Pair.type)[0].index;",
        "if(answer!==0n) throw Error('fields');",
    );
    run(
        "use std::type_property as p; @property(PropertyTarget::Type) type Mark = struct {label:String}; def mark: Fn(Type,Option(Mark))->Mark = fn(t,prev){{label:\"ok\"}}; @mark type Test = struct {}; pub def answer: String = match p::get_type_prop(Test.type,Mark.type){Some(x)=>x.label,None=>\"missing\"};",
        "if(answer!=='ok') throw Error('property');",
    );
    run(
        "use std::type_memorizer as m; pub def answer: Int = { let cache=m::type_memorizer(fn(ty){42}); m::computed_with_type(cache,Int.type) };",
        "if(answer!==42n) throw Error('memo');",
    );
}

#[test]
fn json_codecs_preserve_validation() {
    run(
        "use std::json as j; type Query=struct {count:Int,label:Option(String)}; pub def answer: Int = match j::decode@[Query](\"{\\\"count\\\":42}\") {Ok(q)=>q.count,Err(e)=>0};",
        "if(answer!==42n)throw Error('decode');",
    );
    run(
        "use std::json as j; type Query=struct {count:Int}; pub def answer: Bool = match j::decode@[Query](\"{\\\"count\\\":42,\\\"extra\\\":1}\") {Ok(q)=>False,Err(e)=>True};",
        "if(answer!==true)throw Error('unknown field accepted');",
    );
    run(
        "type Id = struct(Int); pub def answer: Int = Id(42).0;",
        "if(answer!==42n)throw Error('newtype');",
    );
}

#[test]
fn pipeline_regex_hash_and_checked_decode() {
    run(
        "use std::pipeline as p; pub def answer: Array(Int) = p::run([1,2,1] |> p::from_array |> p::distinct_by(fn(x){`\\{x}`}) |> p::collect());",
        "if(answer.join(',')!=='1,2')throw Error('pipeline');",
    );
    run(
        "use std::hash as h; pub def answer: String = h::sha256(\"abc\");",
        "if(answer!=='ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad')throw Error('sha256');",
    );
    run(
        "use std::regex as r; use std::string as s; @r::parse_by(r::compile(r\"^(?P<year>[0-9]{4})$\")) type Year=struct{year:Int}; pub def answer: Int = match s::parse@[Year](\"2026\"){Ok(v)=>v.year,Err(e)=>0};",
        "if(answer!==2026n)throw Error('regex');",
    );
    run(
        "use std::json as j; use std::blame::{BlameError}; def positive: Fn(Int)->Result((),BlameError)=fn(v){if v>0{Ok(())}else{Err(blame!(\"expected positive\",v))}}; @check(positive) type Positive=struct(Int); pub def answer: Bool = match j::decode@[Positive](\"0\"){Ok(v)=>False,Err(e)=>True};",
        "if(answer!==true)throw Error('check');",
    );
}

#[test]
fn warnings_remain_successful_diagnostics() {
    run(
        "use std::_rt as rt; use std::array as a; pub def answer: Bool = match rt::with_diagnostics(fn(v:Int){let ignored:Option(Int)=warn!(\"warning only\");v})(42) {Ok((v,ds))=>v==42 && a::length(ds)==1,Err(ds)=>False};",
        "if(answer!==true)throw Error('warning became failure');",
    );
}
