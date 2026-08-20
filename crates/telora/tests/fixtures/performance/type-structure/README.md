# Type-structure performance fixtures

These fixtures preserve the reproductions used by issue #83. They are kept out
of the default test suite because the recursive-value regression is currently
slow by design.

Build an optimized CLI before measuring:

```sh
cargo build --release -p telora
```

Run the stable suite from the repository root:

```sh
python3 crates/telora/tests/fixtures/performance/type-structure/measure.py
```

The runner performs one warm-up and three measured runs per case, then emits
one JSON object per line with median wall, user, and system time. It never
includes compilation time. Use `--samples`, `--fixture`, `--binary`, and
`--timeout` to control an explicit run; `--help` lists the accepted fixture
names.

For an exploratory single sample, run:

```sh
/usr/bin/time -f 'elapsed=%e user=%U sys=%S' \
  target/release/telora check @src/nested-functions.telora \
  -C crates/telora/tests/fixtures/performance/type-structure
```

The modules cover distinct costs:

- `flat-functions.telora`: 100 flat `Int` function contracts.
- `recursive-functions.telora`: 100 constructors checked against a recursive
  `Expr` contract.
- `nested-functions.telora`: 100 constructors checked against a deeply nested
  but non-recursive structural contract.
- `recursive-values-shallow.telora`: a recursive type with repeated shallow
  values.
- `recursive-values-growing.telora`: a recursively growing shared value graph.
- `query-builder.telora`: the real-world QueryBuilder module that exposed the
  regression in the ontology experiment.

Use `query` to isolate workspace recovery from output rendering:

```sh
/usr/bin/time -f 'elapsed=%e user=%U sys=%S' \
  target/release/telora query at @src/query-builder.telora \
  -C crates/telora/tests/fixtures/performance/type-structure \
  -p definitely_missing_name
```

Record the compiler profile, hardware, command, and several runs when comparing
results. Compare medians produced on the same host under similar load.
Wall-clock thresholds should only be added after the underlying hot paths are
understood.
