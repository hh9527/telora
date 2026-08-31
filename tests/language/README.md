# Language acceptance tests

Each case has a `testee.telora` under `src/<mode>/<case>/`. Ordinary language
features assert their expectations in the testee with `fail!`; a zero exit code
and empty stderr are sufficient, so they do not need a case-specific checker.
Cases that verify one diagnostic substring add an `expected.txt`. Cases that
need richer diagnostic or output protocol checks add a `check.telora`.
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
runner generates a workspace manifest, aggregate execution modules, and one
aggregate checker. Ordinary successful `check` cases share one best-effort
process. Simple expected-diagnostic cases share another process and are split
back into per-case observations by their diagnostic source identity. It applies
a generic success check to self-validating testees, invokes explicit check
functions where present, and reports whether every case passed.

Generated sources, raw streams, observations, and checker output are kept in
`target/language-tests/` for inspection. Test sources do not participate in the
Rust build, so changing a testee or checker does not recompile Telora.
