use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn fixture_dir() -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("telora-test-{unique}"));
    fs::create_dir(&path).unwrap();
    path
}

fn telora() -> Command {
    Command::new(env!("CARGO_BIN_EXE_telora"))
}

#[test]
fn help_exposes_the_lsp_subcommand() {
    let output = telora().arg("help").output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("telora lsp"));
}

#[test]
fn check_run_and_types_cover_the_closed_world_loop() {
    let directory = fixture_dir();
    fs::write(directory.join("data.json"), r#"{"name":"Ada","age":36}"#).unwrap();
    fs::write(
        directory.join("main.telora"),
        "import \"./data.json\" as data;\
         @struct type User = {name: String, age: Int};\
         let user: User = data;\
         export let output = validate(User, user);",
    )
    .unwrap();

    let check = telora()
        .args(["check", directory.join("main.telora").to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    assert!(String::from_utf8_lossy(&check.stdout).contains("2 dependencies"));

    let types = telora()
        .args(["types", directory.join("main.telora").to_str().unwrap()])
        .output()
        .unwrap();
    assert!(types.status.success());
    assert!(String::from_utf8_lossy(&types.stdout).contains("type User ="));

    let run = telora()
        .args(["run", directory.join("main.telora").to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(String::from_utf8_lossy(&run.stdout).contains("'Ok("));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn run_accepts_external_json_and_failures_are_nonzero() {
    let directory = fixture_dir();
    fs::write(directory.join("main.telora"), "export { input as output };").unwrap();
    fs::write(directory.join("input.json"), "[1, 2, 3]").unwrap();

    let run = telora()
        .args([
            "run",
            directory.join("main.telora").to_str().unwrap(),
            "--input",
            directory.join("input.json").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(run.status.success());
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "[1, 2, 3]");

    fs::write(directory.join("bad.telora"), "export let output = 1 / 0;").unwrap();
    let failure = telora()
        .args(["run", directory.join("bad.telora").to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!failure.status.success());
    let stderr = String::from_utf8_lossy(&failure.stderr);
    assert!(stderr.contains("division by zero"));
    assert!(stderr.contains("bad.telora:1:"));
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn run_reports_blame_without_subject_locations_instead_of_panicking() {
    let directory = fixture_dir();
    let main = directory.join("main.telora");
    fs::write(
        &main,
        r#"let error = blame!("missing capability", "subject");
let reported = report('Error, error);
export let output = reported;"#,
    )
    .unwrap();

    let output = telora()
        .args(["run", main.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("missing capability"), "{stderr}");
    assert!(stderr.contains("main.telora:1:"), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn run_selects_output_from_explicit_main_exports() {
    let directory = fixture_dir();
    let main = directory.join("explicit.telora");
    fs::write(
        &main,
        "let private = 1; let result = private + 41; export { result as output };",
    )
    .unwrap();
    let run = telora()
        .args(["run", main.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "42");

    fs::write(&main, "export let other = 1;").unwrap();
    let missing = telora()
        .args(["run", main.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(
        String::from_utf8_lossy(&missing.stderr)
            .contains("telora run requires the explicit export \"output\"")
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn show_reports_ordered_independent_runtime_failures_while_run_stays_strict() {
    let directory = fixture_dir();
    let main = directory.join("main.telora");
    fs::write(
        &main,
        "let first = 1 / 0;\nlet blocked = first + 1;\nlet second = 2 / 0;\nexport let output = 0;",
    )
    .unwrap();

    let show = telora()
        .args(["show", main.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(show.status.success());
    let stdout = String::from_utf8_lossy(&show.stdout);
    assert_eq!(stdout.matches("division by zero").count(), 2, "{stdout}");
    assert!(stdout.find(":1:").unwrap() < stdout.find(":3:").unwrap());

    let run = telora()
        .args(["run", main.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!run.status.success());
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(!stderr.trim().is_empty());
    assert_eq!(stderr.matches("division by zero").count(), 1, "{stderr}");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn run_evaluates_structured_string_interpolation() {
    let directory = fixture_dir();
    fs::write(
        directory.join("interpolation.telora"),
        r#"let name = "Ada"; let count = 2; export let output = `hi, \{name} x\{count}`;"#,
    )
    .unwrap();

    let run = telora()
        .args([
            "run",
            directory.join("interpolation.telora").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&run.stdout).trim(),
        "\"hi, Ada x2\""
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn run_writes_debug_events_only_to_stderr() {
    let directory = fixture_dir();
    fs::write(
        directory.join("debug.telora"),
        r#"import "std/debug" as debug;
           export let output: Int = 42 |> debug.dbg_with\("answer\nlabel", _);"#,
    )
    .unwrap();

    let run = telora()
        .args(["run", directory.join("debug.telora").to_str().unwrap()])
        .output()
        .unwrap();
    assert!(run.status.success());
    assert_eq!(String::from_utf8_lossy(&run.stdout).trim(), "42");
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        stderr
            .lines()
            .all(|line| line == r#"[debug] "answer\nlabel": 42"#),
        "{stderr}"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn exec_dry_run_invokes_explicit_pure_entry() {
    let directory = fixture_dir();
    let cache = directory.join("cache");
    let main = directory.join("exec.telora");
    fs::write(
        &main,
        r#"#!/usr/bin/env -S telora exec --dry-run
import "std/array" as arrays;
import "std/rt-types/exec.telora" { ExecFn };
import "std/hash" as hash;
option "exec.capture-envs" ["TELORA_EXEC_TEST", "TELORA_EXEC_TEST"];
export def exec: ExecFn = fn(settings, request) {
    let platform = `\{settings.platform.os}-\{settings.platform.arch}`;
    let compiler_url = `https://example.invalid/gcc-\{platform}.tar.gz`;
    let sysroot_url = `https://example.invalid/sysroot-\{platform}.tar.gz`;
    let compiler = `\{settings.install_prefix}/\{hash.sha256(compiler_url)}`;
    let sysroot = `\{settings.install_prefix}/\{hash.sha256(sysroot_url)}`;
    let compiler_file = `\{settings.download_prefix}/\{hash.sha256(compiler_url)}`;
    let sysroot_file = `\{settings.download_prefix}/\{hash.sha256(sysroot_url)}`;
    {
        install: [
            'Unpack({dest: compiler, file: compiler_file, ty: 'TarGzip, src: compiler_url, strip: 1, digest: 'None}),
            'Unpack({dest: sysroot, file: sysroot_file, ty: 'TarGzip, src: sysroot_url, strip: 1, digest: 'None}),
        ],
        cwd: 'Some(request.cwd),
        bin: `\{compiler}/bin/gcc`,
        args: arrays.flat_map([[`--sysroot=\{sysroot}`], request.args], fn(part) { part }),
        env: {
            clear: 'True,
            update: {HIDDEN: 'None, VISIBLE: 'Some(request.env.TELORA_EXEC_TEST)},
        },
    }
};"#,
    )
    .unwrap();

    let output = telora()
        .env("TELORA_CACHE_HOME", &cache)
        .env("TELORA_EXEC_TEST", "visible")
        .args([
            "exec",
            "--dry-run",
            main.to_str().unwrap(),
            "--",
            "one",
            "two words",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains(r#""args":["--sysroot="#) && stdout.contains(r#"","one","two words"]"#),
        "{stdout}"
    );
    assert!(
        stdout.contains(r#""bin":"#) && stdout.contains("/bin/gcc"),
        "{stdout}"
    );
    assert!(
        stdout.contains(r#""env":{"clear":true,"update":{"HIDDEN":null,"VISIBLE":"visible"}}"#),
        "{stdout}"
    );
    assert_eq!(stdout.matches(r#""Unpack""#).count(), 2, "{stdout}");
    assert!(stdout.contains(r#""ty":"TarGzip""#), "{stdout}");
    assert!(stdout.contains(r#""file":"#), "{stdout}");
    assert!(
        stdout.contains(&format!("{}/telora/exec/downloads/", cache.display())),
        "{stdout}"
    );
    assert!(
        stdout.contains(&format!("{}/telora/exec/installs/", cache.display())),
        "{stdout}"
    );
    assert!(!cache.exists());
    let repeated = telora()
        .env("TELORA_CACHE_HOME", &cache)
        .env("TELORA_EXEC_TEST", "visible")
        .args([
            "exec",
            "--dry-run",
            main.to_str().unwrap(),
            "--",
            "one",
            "two words",
        ])
        .output()
        .unwrap();
    assert!(repeated.status.success());
    assert_eq!(repeated.stdout, stdout.as_bytes());
    assert!(!cache.exists());

    let shown = telora()
        .args(["show", main.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(shown.status.success());
    assert!(
        String::from_utf8_lossy(&shown.stdout).contains("Dict<String>"),
        "{}",
        String::from_utf8_lossy(&shown.stdout)
    );
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn exec_captures_only_declared_environment_names() {
    let directory = fixture_dir();
    let main = directory.join("capture.telora");
    fs::write(
        &main,
        r#"option "exec.capture-envs" ["TELORA_CAPTURED", "TELORA_MISSING"];
option "exec.capture-envs" ["TELORA_CAPTURED"];
import "std/rt-types/exec.telora" { ExecFn };
export def exec: ExecFn = fn(settings, request) {
    {install: [], cwd: 'None, bin: "true", args: [], env: {
        clear: 'False,
        update: {TELORA_CAPTURED: 'Some(request.env.TELORA_CAPTURED)},
    }}
};"#,
    )
    .unwrap();

    let output = telora()
        .env("TELORA_CAPTURED", "visible")
        .env("TELORA_UNDECLARED", "secret")
        .env_remove("TELORA_MISSING")
        .args(["exec", "--dry-run", main.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains(r#""env":{"clear":false,"update":{"TELORA_CAPTURED":"visible"}}"#),
        "{stdout}"
    );
    assert!(!stdout.contains("TELORA_UNDECLARED"), "{stdout}");
    assert!(!stdout.contains("TELORA_MISSING"), "{stdout}");

    fs::write(
        &main,
        "option \"exec.capture-envs\" [\"GOOD\", 1]; export def exec = fn(a, b) { {} };",
    )
    .unwrap();
    let malformed = telora()
        .args(["exec", "--dry-run", main.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!malformed.status.success());
    let stderr = String::from_utf8_lossy(&malformed.stderr);
    assert!(stderr.contains(":1:1:"), "{stderr}");
    assert!(stderr.contains("Array(String)"), "{stderr}");
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn gcc_wrapper_fixture_builds_reusable_deterministic_plans() {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/gcc-wrapper/app/bin-src");
    let directory = fixture_dir();
    let cache = directory.join("cache");
    let invoke =
        |cache: &std::path::Path, entry: &str, arguments: &[&str], target: Option<&str>| {
            let mut command = telora();
            command.env("TELORA_CACHE_HOME", cache);
            command.env_remove("TARGET");
            if let Some(target) = target {
                command.env("TARGET", target);
            }
            command.args(["exec", "--dry-run"]);
            command.arg(fixture.join(entry));
            if !arguments.is_empty() {
                command.arg("--").args(arguments);
            }
            command.output().unwrap()
        };

    let gcc = invoke(
        &cache,
        "gcc.telora",
        &["-c", "main.c"],
        Some("x86_64-linux-gnu"),
    );
    assert!(
        gcc.status.success(),
        "{}",
        String::from_utf8_lossy(&gcc.stderr)
    );
    let gcc_stdout = String::from_utf8(gcc.stdout).unwrap();
    assert_eq!(gcc_stdout.matches(r#""Unpack""#).count(), 2, "{gcc_stdout}");
    assert!(gcc_stdout.contains("/bin/gcc"), "{gcc_stdout}");
    assert!(gcc_stdout.contains("--sysroot="), "{gcc_stdout}");
    assert!(gcc_stdout.contains("-ffile-prefix-map="), "{gcc_stdout}");
    assert!(gcc_stdout.contains("-fdebug-prefix-map="), "{gcc_stdout}");
    assert!(gcc_stdout.contains(r#""env":{"clear":false,"update":{}}"#));
    assert!(!gcc_stdout.contains(r#""TARGET":"x86_64-linux-gnu""#));
    assert_eq!(gcc_stdout.matches(r#""file":"#).count(), 2);
    let compiler_suffix = "3c76b5039e9994e6c44145f0dbc8867439c77daa26aeae023aa74d4ef7dc0b46";
    let sysroot_suffix = "8392c1d0e3a485930d5132005216c9bf8067848071215c13bbcf6e5ffe0f4d0c";
    assert!(gcc_stdout.contains(compiler_suffix), "{gcc_stdout}");
    assert!(gcc_stdout.contains(sysroot_suffix), "{gcc_stdout}");
    assert!(!cache.exists());

    let repeated = invoke(
        &cache,
        "gcc.telora",
        &["-c", "main.c"],
        Some("x86_64-linux-gnu"),
    );
    assert!(repeated.status.success());
    assert_eq!(repeated.stdout, gcc_stdout.as_bytes());

    let relocated_cache = directory.join("relocated-cache");
    let relocated = invoke(
        &relocated_cache,
        "gcc.telora",
        &["-c", "main.c"],
        Some("x86_64-linux-gnu"),
    );
    assert!(relocated.status.success());
    let relocated_stdout = String::from_utf8(relocated.stdout).unwrap();
    assert!(relocated_stdout.contains(compiler_suffix));
    assert!(relocated_stdout.contains(sysroot_suffix));
    assert!(relocated_stdout.contains(relocated_cache.to_str().unwrap()));
    assert!(!relocated_cache.exists());

    let gxx = invoke(
        &cache,
        "g++.telora",
        &["main.cc"],
        Some("aarch64-linux-gnu"),
    );
    assert!(
        gxx.status.success(),
        "{}",
        String::from_utf8_lossy(&gxx.stderr)
    );
    let gxx_stdout = String::from_utf8(gxx.stdout).unwrap();
    assert!(gxx_stdout.contains("/bin/g++"), "{gxx_stdout}");
    assert_eq!(gxx_stdout.matches(r#""Unpack""#).count(), 2);

    let ar = invoke(&cache, "ar.telora", &["rcs", "lib.a"], None);
    assert!(
        ar.status.success(),
        "{}",
        String::from_utf8_lossy(&ar.stderr)
    );
    let ar_stdout = String::from_utf8(ar.stdout).unwrap();
    assert!(ar_stdout.contains("/bin/ar"), "{ar_stdout}");
    assert_eq!(ar_stdout.matches(r#""Unpack""#).count(), 1);
    assert!(ar_stdout.contains(compiler_suffix));
    assert!(!ar_stdout.contains("--sysroot="));

    let missing = invoke(&cache, "gcc.telora", &[], None);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("TARGET is required"));
    let conflict = invoke(
        &cache,
        "gcc.telora",
        &["--sysroot=/other"],
        Some("x86_64-linux-gnu"),
    );
    assert!(!conflict.status.success());
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("conflicting argument: --sysroot"));
    assert!(!cache.exists());

    let malformed_root = directory.join("malformed");
    fs::create_dir_all(malformed_root.join("app/bin-src")).unwrap();
    fs::create_dir_all(malformed_root.join("gcc-toolchain-define/src")).unwrap();
    fs::create_dir_all(malformed_root.join("gcc-wrapper/src")).unwrap();
    fs::create_dir_all(directory.join("domain-method/src")).unwrap();
    fs::create_dir_all(directory.join("toolchain-method/src")).unwrap();
    fs::write(
        malformed_root.join("app/bin-src/gcc.telora"),
        include_str!("../../../examples/gcc-wrapper/app/bin-src/gcc.telora"),
    )
    .unwrap();
    fs::write(
        malformed_root.join("gcc-wrapper/src/toolchain.telora"),
        include_str!("../../../examples/gcc-wrapper/gcc-wrapper/src/toolchain.telora"),
    )
    .unwrap();
    fs::write(
        directory.join("domain-method/src/method.telora"),
        include_str!("../../../examples/domain-method/src/method.telora"),
    )
    .unwrap();
    fs::write(
        directory.join("toolchain-method/src/toolchain.telora"),
        include_str!("../../../examples/toolchain-method/src/toolchain.telora"),
    )
    .unwrap();
    fs::write(
        malformed_root.join("gcc-toolchain-define/src/source.json"),
        r#"{"compilers":[],"sysroots":{}}"#,
    )
    .unwrap();
    let malformed = {
        let mut command = telora();
        command
            .env("TELORA_CACHE_HOME", &cache)
            .env("TARGET", "x86_64-linux-gnu")
            .args(["exec", "--dry-run"])
            .arg(malformed_root.join("app/bin-src/gcc.telora"))
            .output()
            .unwrap()
    };
    assert!(!malformed.status.success());
    let malformed_stderr = String::from_utf8_lossy(&malformed.stderr);
    assert!(
        malformed_stderr.contains("toolchain.telora"),
        "{malformed_stderr}"
    );
    assert!(
        malformed_stderr.contains("source.json"),
        "{malformed_stderr}"
    );
    assert!(malformed_stderr.contains("compilers"), "{malformed_stderr}");
    fs::remove_dir_all(directory).unwrap();
}

#[cfg(unix)]
#[test]
fn exec_dry_run_rejects_invalid_cli_entry_and_result() {
    let directory = fixture_dir();
    let assert_contract_error = |output: &std::process::Output, source_name: &str| {
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("module export at"), "{stderr}");
        assert!(stderr.contains(source_name), "{stderr}");
        assert!(stderr.contains("is not assignable to Fn("), "{stderr}");
        assert!(stderr.contains("entry/exec.telora"), "{stderr}");
    };
    let value = directory.join("value.telora");
    fs::write(&value, "export let exec = 42;").unwrap();

    let rejected = telora()
        .args(["exec", value.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("currently requires --dry-run"));

    let short = telora()
        .args(["exec", "-n", value.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!short.status.success());

    let not_function = telora()
        .args(["exec", "--dry-run", value.to_str().unwrap()])
        .output()
        .unwrap();
    assert_contract_error(&not_function, "value.telora");

    let not_dict = directory.join("not-dict.telora");
    fs::write(&not_dict, "export def exec = fn(settings, request) { 42 };").unwrap();
    let not_dict = telora()
        .args(["exec", "--dry-run", not_dict.to_str().unwrap()])
        .output()
        .unwrap();
    assert_contract_error(&not_dict, "not-dict.telora");

    let malformed = directory.join("malformed.telora");
    fs::write(
        &malformed,
        "export def exec = fn(settings, request) { {install: ['Copy({})], cwd: 'None, bin: \"x\", args: [], env: {clear: 'False, update: {}}} };",
    )
    .unwrap();
    let malformed = telora()
        .args(["exec", "--dry-run", malformed.to_str().unwrap()])
        .output()
        .unwrap();
    assert_contract_error(&malformed, "malformed.telora");

    let missing_file = directory.join("missing-file.telora");
    fs::write(
        &missing_file,
        "export def exec = fn(settings, request) { {install: ['Unpack({dest: \"/dest\", digest: 'None, src: \"https://example.invalid/a.tar\", strip: 1, ty: 'Tar})], cwd: 'None, bin: \"x\", args: [], env: {clear: 'False, update: {}}} };",
    )
    .unwrap();
    let missing_file = telora()
        .args(["exec", "--dry-run", missing_file.to_str().unwrap()])
        .output()
        .unwrap();
    assert_contract_error(&missing_file, "missing-file.telora");
    assert!(String::from_utf8_lossy(&missing_file.stderr).contains("file"));

    let bad_env = directory.join("bad-env.telora");
    fs::write(
        &bad_env,
        "export def exec = fn(settings, request) { {install: [], cwd: 'None, bin: \"x\", args: [], env: {clear: 'False, update: {BAD: 1}}} };",
    )
    .unwrap();
    let bad_env = telora()
        .args(["exec", "--dry-run", bad_env.to_str().unwrap()])
        .output()
        .unwrap();
    assert_contract_error(&bad_env, "bad-env.telora");

    let bad_cwd = directory.join("bad-cwd.telora");
    fs::write(
        &bad_cwd,
        "export def exec = fn(settings, request) { {install: [], cwd: 'Some(1), bin: \"x\", args: [], env: {clear: 'False, update: {}}} };",
    )
    .unwrap();
    let bad_cwd = telora()
        .args(["exec", "--dry-run", bad_cwd.to_str().unwrap()])
        .output()
        .unwrap();
    assert_contract_error(&bad_cwd, "bad-cwd.telora");

    let structural = directory.join("structural.telora");
    fs::write(
        &structural,
        r#"import "std/rt-types/exec.telora" { ExecSettings, ExecRequest, ExecEnv };
type MyExecFn = Fn(ExecSettings, ExecRequest) -> ExecEnv;
export def exec: MyExecFn = fn(settings, request) {
    {install: [], cwd: 'None, bin: "true", args: request.args, env: {clear: 'False, update: {}}}
};"#,
    )
    .unwrap();
    let structural = telora()
        .args(["exec", "--dry-run", structural.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        structural.status.success(),
        "{}",
        String::from_utf8_lossy(&structural.stderr)
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn build_dry_run_validates_and_prints_text_artifacts_without_writing() {
    let directory = fixture_dir();
    let main = directory.join("build.telora");
    fs::write(
        &main,
        r####"import "std/build" as build_types;
import "std/string" as strings;
type OutputPlan = build_types.OutputPlan;
export def build: Fn() -> OutputPlan = fn() {
    {
        files: [
            'TextFile({
                path: "generated/app.conf",
                content: strings.trim_margin(r"|server {
                    |    listen 8080;
                    |}", "|") |> strings.ensure_trailing_newline,
            }),
            'TextFile({path: "generated/name.txt", content: `Telora\n`}),
        ],
    }
};"####,
    )
    .unwrap();

    let output = telora()
        .args(["build", "--dry-run", main.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.matches("TextFile").count(), 2, "{stdout}");
    assert!(
        stdout.contains(r#""path":"generated/app.conf""#),
        "{stdout}"
    );
    assert!(
        stdout.contains(r#"server {\n    listen 8080;\n}\n"#),
        "{stdout}"
    );
    assert!(!directory.join("generated").exists());

    let repeated = telora()
        .args(["build", "--dry-run", main.to_str().unwrap()])
        .output()
        .unwrap();
    assert_eq!(repeated.stdout, stdout.as_bytes());

    for (name, source, expected) in [
        (
            "escape.telora",
            "export def build = fn() { {files: ['TextFile({path: \"../outside\", content: \"x\"})]} };",
            "normalized relative path",
        ),
        (
            "duplicate.telora",
            "export def build = fn() { {files: ['TextFile({path: \"a\", content: \"x\"}), 'TextFile({path: \"a\", content: \"y\"})]} };",
            "duplicate path",
        ),
    ] {
        let path = directory.join(name);
        fs::write(&path, source).unwrap();
        let output = telora()
            .args(["build", "--dry-run", path.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn show_observes_deterministic_workspace_and_position_queries() {
    let directory = fixture_dir();
    let main = directory.join("main.telora");
    fs::write(&main, "let answer = 42;\nexport { answer as output };").unwrap();

    let first = telora()
        .args(["show", main.to_str().unwrap()])
        .output()
        .unwrap();
    let second = telora()
        .args(["show", main.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert_eq!(first.stdout, second.stdout);
    let report = String::from_utf8_lossy(&first.stdout);
    assert!(report.contains("definitions:"));
    assert!(report.contains("Let answer"));
    assert!(report.contains("references:"));
    assert!(report.contains("expressions:"));
    assert!(report.contains("answer"));
    assert!(report.contains(" = Int"));

    let at = telora()
        .args([
            "show",
            main.to_str().unwrap(),
            "at",
            main.to_str().unwrap(),
            "2",
            "10",
        ])
        .output()
        .unwrap();
    assert!(
        at.status.success(),
        "{}",
        String::from_utf8_lossy(&at.stderr)
    );
    let output = String::from_utf8_lossy(&at.stdout);
    assert!(output.contains("reference:"), "{output}");
    assert!(output.contains("expression:"), "{output}");
    assert!(output.contains("type:"), "{output}");
    assert!(output.contains(" = Int"), "{output}");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn show_reports_inferred_local_type_schemes() {
    let directory = fixture_dir();
    let main = directory.join("main.telora");
    fs::write(
        &main,
        "let identity = fn(value) { value };\nlet result = identity(1); export { result as output };",
    )
    .unwrap();

    let show = telora()
        .args(["show", main.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        show.status.success(),
        "{}",
        String::from_utf8_lossy(&show.stderr)
    );
    let output = String::from_utf8_lossy(&show.stdout);
    assert!(
        output.contains("Let identity") && output.contains("for(A) Fn(A) -> A"),
        "{output}"
    );
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn types_projects_recursive_types_from_the_workspace_snapshot() {
    let directory = fixture_dir();
    let main = directory.join("main.telora");
    fs::write(
        &main,
        "@struct type Node = {children: Array(Node)}; export { Node };",
    )
    .unwrap();

    let types = telora()
        .args(["types", main.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        types.status.success(),
        "{}",
        String::from_utf8_lossy(&types.stderr)
    );
    let output = String::from_utf8_lossy(&types.stdout);
    assert_eq!(output.matches("type Node =").count(), 1, "{output}");
    assert!(!output.contains("let Node:"), "{output}");
    assert!(output.contains("children: Array<"), "{output}");
    assert!(output.contains("Node"), "{output}");
    assert!(output.contains("result:"), "{output}");
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn show_recovers_semantics_from_damaged_source_while_check_remains_strict() {
    let directory = fixture_dir();
    let main = directory.join("main.telora");
    fs::write(
        &main,
        "let before = 1; let broken = ; let after = missing; after",
    )
    .unwrap();

    let show = telora()
        .args(["show", main.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        show.status.success(),
        "{}",
        String::from_utf8_lossy(&show.stderr)
    );
    let output = String::from_utf8_lossy(&show.stdout);
    assert!(output.contains("diagnostics:"), "{output}");
    assert!(output.contains("before"), "{output}");
    assert!(output.contains("after"), "{output}");
    assert!(output.contains("Unknown(UnresolvedName)"), "{output}");

    let check = telora()
        .args(["check", main.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!check.status.success());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn show_continues_independent_type_metadata_after_tool_failure() {
    let directory = fixture_dir();
    let main = directory.join("partial.telora");
    fs::write(
        &main,
        "type A = broken(Int);\
         type B = String;\
         type C = Array(B);\
         type D = Array(A);\
         0",
    )
    .unwrap();

    let show = telora()
        .args(["show", main.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        show.status.success(),
        "{}",
        String::from_utf8_lossy(&show.stderr)
    );
    let output = String::from_utf8_lossy(&show.stdout);
    assert!(
        output.contains("A") && output.contains("Incomputable"),
        "{output}"
    );
    assert!(
        output.contains("B") && output.contains(" = String"),
        "{output}"
    );
    assert!(
        output.contains("C") && output.contains(" = Array<String>"),
        "{output}"
    );
    assert!(
        output.contains("D") && output.contains("BlockedBy"),
        "{output}"
    );

    let check = telora()
        .args(["check", main.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!check.status.success());
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn show_recovers_types_and_causes_across_failed_modules() {
    let directory = fixture_dir();
    let model = directory.join("model.telora");
    let main = directory.join("main.telora");
    fs::write(
        &model,
        "type Broken = missing(Int); type Good = String; export { Good };",
    )
    .unwrap();
    fs::write(
        &main,
        "import \"./model.telora\" as model;\
         type Local = String;\
         type Uses = model.Good;\
         type Down = Array(Uses);\
         export { Local as output };",
    )
    .unwrap();

    let show = telora()
        .args(["show", main.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        show.status.success(),
        "{}",
        String::from_utf8_lossy(&show.stderr)
    );
    let output = String::from_utf8_lossy(&show.stdout);
    assert!(output.contains("Partial"), "{output}");
    assert!(output.contains("model.telora"), "{output}");
    assert!(
        output.contains("Local") && output.contains(" = String"),
        "{output}"
    );
    assert!(
        output.contains("Uses") && output.contains("BlockedBy"),
        "{output}"
    );
    assert!(
        output.contains("Good") && output.contains(" = String"),
        "{output}"
    );

    let check = telora()
        .args(["check", main.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!check.status.success());
    fs::remove_dir_all(directory).unwrap();
}
