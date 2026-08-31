# Language acceptance tests

Each case is a `testee.telora` and `check.telora` pair under `src/<mode>/<case>/`.
The mode selects the public Telora command used to observe the testee:

- `eval`: evaluate the `result` export;
- `query`: query the testee's exports;
- `check`: check the testee and collect diagnostics.

Run the suite after building Telora:

```sh
cargo build -p telora
scripts/test-language.sh
```

Set `TELORA_BIN` to exercise another Telora binary. The runner requires `jaq`.

For every testee, the runner records this value:

```json
{
  "exit_code": 0,
  "stdout": [],
  "stderr": []
}
```

`stdout` and `stderr` contain the JSON or JSONL records emitted by Telora. The
runner generates a workspace manifest and one aggregate checker, invokes all
check functions in one `eval-with`, and reports whether every checker returned
`true`.

Generated sources, raw streams, observations, and checker output are kept in
`target/language-tests/` for inspection. Test sources do not participate in the
Rust build, so changing a testee or checker does not recompile Telora.
