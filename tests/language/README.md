# Language acceptance tests

For language-level testing practices, see [guide/TESTING.md](../../guide/TESTING.md).
This page describes Telora's own acceptance harness and its command-level fixtures.

Each case has a `testee.telora` under `src/<mode>/<case>/`. Ordinary runtime
semantics use named `std/test.Test` exports under `src/test/`. Their assertions
run inside thunks and fail explicitly; a zero exit code and empty stderr are
sufficient, so they do not need a case-specific checker.
Cases that verify one diagnostic substring add an `expected.txt`. Cases that
need richer diagnostic or output protocol checks add a `check.telora`.
The mode selects the public Telora command used to observe the testee:

- `eval`: evaluate the `result` export;
- `query`: query the testee's exports;
- `query-at`: query the testee's top-level semantic facts, including recovery;
- `check`: check the testee and collect diagnostics;
- `test`: run direct `std/test.Test` exports. The runner copies `src/test/`
  into the generated workspace's `tests/`, preserving fixture and helper paths.
  A checker can inspect intentionally failed cases and the v2 summary.

## Writing runtime tests

Test assets may use LF or CRLF; no forced-LF Git checkout is required.
The harness normalizes line endings in `expected.txt` expectations, while
language and data files reach the compiler unchanged. The language normalizes
physical newlines in string literals to LF; explicit CR escapes remain CR.
The harness does not rewrite literal contents. The Rust CLI runner reads
the shell script at runtime and invokes Bash explicitly (Bash and `jaq` are
required on every platform). Set `TELORA_LANGUAGE_ROOT` to a copied acceptance
directory to verify another checkout's assets without changing tracked files.

Prefer independent `.telora` tests for language behavior, including codecs,
reflection, text operations, and generic functions. Rust tests should focus on
compiler structures, Wasm ABI, Host interactions, GC, and precise diagnostic
origins. When a Rust test needs source fixtures, read them at runtime with
`std::fs::read_to_string`; do not embed test assets with `include_str!` or
`include_bytes!`. Bundling the production standard library is unrelated.

After building the CLI once, source and expectation changes need no Cargo build:

```sh
TELORA_BIN="$PWD/target/release/telora" bash scripts/test-language.sh
```

The `runtime-*` groups keep migrated Wasm/CLI assertions alongside their source
modules. JSON expectations are checked in explicitly; never regenerate them
from the implementation's current output. `runtime-support.all` also checks
the number of boolean results to prevent an empty or shortened result passing.

Group related cases in one testee and give each export a descriptive name:

```telora
import "std/test" as test;
import "@src/test-support" {expect};

export def array_index: test.Test = test.should_ok(fn() {
    let values = [10, 20];
    expect(values[1] == 20)
});
export def array_bounds: test.Test = test.should_fail_with(fn() {
    [10][1]
}, "OutOfRange");
```

`should_ok` accepts any normal return, including `False`. Always use `expect`
for a boolean condition, and put the computation itself inside the thunk.
Module-level types, decorators, and reusable functions can remain outside it.
`should_fail_with` checks recoverable runtime failures; syntax, type, import,
initialization, and terminal quota diagnostics keep their command-level fixtures.

Use `with_fixtures` for parameterized external input (see `src/test/fixtures`).
Tests of static data modules must continue to use `import`: fixture sources do
not exercise module discovery, static loading, or imported nominal identities.
Keep `query`, `query-at`, and output-protocol fixtures on their original commands.
See [MIGRATION.md](MIGRATION.md) for the completed migration and retained scope.

## Running the suite

Run the suite after building Telora:

```sh
cargo build -p telora
scripts/test-language.sh
```

Set `TELORA_BIN` to exercise another Telora binary. The runner requires `jaq`.
Do not run this script concurrently with `cargo test`: the Rust acceptance test
also invokes it, and both use the same generated workspace and output directory.

For every testee, the runner records this value:

```json
{
  "exit_code": 0,
  "stdout": [],
  "stderr": []
}
```

`stdout` and `stderr` contain the JSON or JSONL records emitted by Telora. The
runner generates a workspace manifest, aggregate execution modules, and one
aggregate checker. Ordinary successful `check` cases share one best-effort
process. Simple expected-diagnostic cases share another process and are split
back into per-case observations by their diagnostic source identity. It applies
a generic success check to self-validating testees, invokes explicit check
functions where present, and reports whether every case passed.

Generated sources, raw streams, observations, and checker output are kept in
`target/language-tests/` for inspection. Test sources do not participate in the
Rust build, so changing a testee or checker does not recompile Telora.
