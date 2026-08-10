# AI-3 contract

## Enterprise model must provide

- Closed measure, dimension, and node id types.
- Concrete measure/dimension capability, output, combined-measure, edge/mapping, and plan types.
- Requested ids, catalogs, typed id selectors, lower adapters, and lowering inputs.
- Evidence record types, builders, `values` accessors, and `complete` accessors.
- A measure combination policy returning `Option(CombinedMeasure)`.
- Base-node, combined-measure required-node, and dimension-node selectors; extra required nodes.
- Disjoint safe and fan-out edge catalogs plus typed endpoint selectors.
- Authored relationship requirements with node and diagnostic-subject selectors.
- A concrete path evidence type, builder, and three accessors.
- A final builder returning `Option(Plan)`.

Mechanical field-forwarding selectors contain no domain policy. Combination, authorization,
alignment, mapping meaning, and plan construction remain enterprise policy.

## Shared layer guarantees

- Every requested capability is independently searched and lowered.
- Results preserve request order, successful values remain available, and complete evidence cannot
  exist after a missing or failed request.
- Missing capabilities are diagnosed against the authored id.
- Safe reachability, fan-out-only reachability, and missing reachability are distinguished.
- Represented authored requirements receive provenance-bearing relationship diagnostics.
- The final builder can run for independent diagnostics after a combined candidate exists.
- No plan is published unless measure evidence, dimension evidence, combination, relationships,
  and the final candidate all succeed.

## Limits AI-3 must respect

Paths are bounded to eight edges. A required node originating only from the measure or `extra_nodes`
is classified, but it receives no authored diagnostic unless AI-3 also includes a corresponding
value in `requirements`. Relationship mappings are preserved on selected enterprise edges but are
opaque to this library. Do not place a safe edge into the fan-out catalog a second time.
