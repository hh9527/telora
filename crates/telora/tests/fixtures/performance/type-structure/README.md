# Type-structure performance fixtures

These fixtures measure frontend type processing, module initialization, and
runtime construction. They are opt-in benchmarks outside the default test suite.
The measured programs use the current workspace manifest and enum syntax.

Build the debug CLI and prepare the fixture workspace:

```sh
cargo build -p telora
target/debug/telora -C crates/telora/tests/fixtures/performance/type-structure lock
```

Run the stable suite from the repository root:

```sh
python3 crates/telora/tests/fixtures/performance/type-structure/measure.py \
  --binary target/debug/telora
```

The runner performs one warm-up and three measured runs per case, then emits
one JSON object per line with median wall, user, and system time. Runtime cases
also verify the computed result. It never includes compilation time. Use
`--samples`, `--fixture`, `--binary`, and
`--timeout` to control an explicit run; `--help` lists the accepted fixture
names.

For an exploratory single sample, run:

```sh
target/debug/telora -C crates/telora/tests/fixtures/performance/type-structure \
  check @src/nested-functions
```

The modules cover distinct costs:

- `startup.telora`: frontend work for a module exporting one integer.
- `flat-functions.telora`: 100 flat `Int` function contracts.
- `recursive-functions.telora`: 100 constructors checked against a recursive
  `Expr` contract.
- `nested-functions.telora`: 100 constructors checked against a deeply nested
  but non-recursive structural contract.
- `recursive-values-shallow.telora`: a recursive type with repeated shallow
  values.
- `recursive-values-growing.telora`: a recursively growing shared value graph.
- `query-builder.telora`: the real-world QueryBuilder module that exposed the
  regression in an eDSL experiment.
- `runtime-integer.telora`: 2,000 integer iterations through `eval`.
- `runtime-plain.telora` and `runtime-generic.telora`: 2,000 nominal constructions;
  the `-long` variants run 20,000 iterations to separate fixed and per-iteration costs.
- `runtime-codec.telora`: 2,000 JSON decode operations.
- `runtime-checked.telora`: 2,000 constructions with a successful `@check`.

Use `query` to isolate workspace recovery from output rendering:

```sh
target/debug/telora -C crates/telora/tests/fixtures/performance/type-structure \
  query at @src/query-builder \
  -p definitely_missing_name
```

Record the compiler profile, hardware, command, and several runs when comparing
results. Compare medians produced on the same host under similar load.
Wall-clock thresholds should only be added after the underlying hot paths are
understood.

The dated RFC 0275 measurements and profiling conclusions are in
[PERFORMANCE-2026-09-08.md](PERFORMANCE-2026-09-08.md).
