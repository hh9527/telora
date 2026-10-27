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
fn unsupported_constructs_fail_at_compile_time() {
    assert!(
        compile("pub def answer: Array(Int) = [1,2];")
            .unwrap_err()
            .contains("unsupported")
    );
}
