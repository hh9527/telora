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
bin/telora run crates/ontology-edsl/probe.telora
  -> {complete: 'Some([11, 21]), results: ['Some(11), 'Some(21)], values: [11, 21]}
bin/telora check crates/ontology-edsl/analytics_probe.telora -> ok (6 dependencies)
bin/telora run crates/ontology-edsl/analytics_probe.telora
  -> 'Some({dimensions: 1, relationships: 1, total: 101})
```

The analytics probe calls the shared `compile_analytics` entry with concrete closed measure,
dimension, node, edge, evidence, combined-measure, requirement, and plan types. Its successful
`Some(Plan)` output observes the valid atomic-publication path.

## Remaining risks

- Paths longer than eight edges can be misclassified.
- The selected safe-edge set is pruned to edges on some safe route toward a required target, but it
  can include multiple alternative routes rather than a minimal tree.
- When measure combination fails there is no combined base node, so relationship and final-builder
  diagnostics cannot run.
- Partial successful dimensions participate in relationship diagnostics by design; completeness
  still blocks publication.
- Measure/extra required nodes without authored requirement records lack equally precise subjects.

No `Any`, `Dyn`, String ids, physical query concepts, fixed plan shape, or Host ABI are present.
