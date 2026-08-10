# Stage 2 notes

## Verification

Executed with the staged binary from the run workspace:

```text
bin/telora check crates/ontology-edsl/src/definitions.telora  -> ok (1 dependencies)
bin/telora check crates/ontology-edsl/src/capabilities.telora -> ok (1 dependencies)
bin/telora check crates/ontology-edsl/src/relationships.telora -> ok (1 dependencies)
bin/telora check crates/ontology-edsl/src/analytics.telora -> ok (3 dependencies)
bin/telora check crates/ontology-edsl/src/lib.telora -> ok (5 dependencies)
bin/telora check crates/ontology-edsl/probe.telora -> ok (6 dependencies)
```

`bin/telora run crates/ontology-edsl/probe.telora` was also attempted. This staged runner requires
an explicit `output` export in an `@main` entry context; the tutorial does not document how to mark
that context. The checked probe exports `output`, `evidence`, and `paths`, but no successful runtime
execution is claimed.

## Remaining risks

- No analytics instantiation probe was available; its complete generic signature is type-checked.
- Paths longer than eight edges can be misclassified.
- The selected safe-edge set is pruned to edges on some safe route toward a required target, but it
  can include multiple alternative routes rather than a minimal tree.
- When measure combination fails there is no combined base node, so relationship and final-builder
  diagnostics cannot run.
- Partial successful dimensions participate in relationship diagnostics by design; completeness
  still blocks publication.
- Measure/extra required nodes without authored requirement records lack equally precise subjects.

No `Any`, `Dyn`, String ids, physical query concepts, fixed plan shape, or Host ABI are present.
