# RFC 0215: B2B analytics DSL migration

- Status: Implemented
- Depends on: RFC 0213, RFC 0214

## Summary

Migrate the richer B2B reporting compiler to `analytics.compile_with`. Preserve
its measure alignment, filters, external restrictions, staged SQL lowering,
render validation, stable execution-plan wire shape, and independent recovery
diagnostics.

## Shared sequence

The B2B `compile` function no longer owns:

- the two capability-compilation calls and their result records;
- measure/dimension completeness checks;
- aggregation of measure, dimension, and filter relationship targets;
- relationship classification and dimension-oriented path diagnostics; or
- the ordering of those proofs relative to candidate-plan publication.

The shared compiler deliberately evaluates the model builder before its final
publication gate. This preserves best-effort diagnostics from independent B2B
stages such as restriction and render checking even when a dimension failed.
The resulting candidate is still discarded unless shared evidence is valid.

## Enterprise knowledge retained

B2B continues to own:

- all semantic enums, SQL AST payloads, physical relation facts, and formulas;
- `selected_measures` and explicit grain alignment in
  `combine_query_plans`;
- filter lowering and restriction policy;
- semantic, relational, SQL, and render lowering; and
- final `ExecutionPlan` construction.

The reusable staged functions remain exported as examples of enterprise
lowering. They are not counted as shared ontology infrastructure.

## Adapter audit

The B2B call supplies the same eight mechanical capability/node selectors as
B2C. It additionally maps model-owned filter requirements to extra nodes. The
relation catalogs, endpoint projections, measure-combination closure, and
final builder carry actual enterprise facts or policy.

The long positional call is usable and fully typed, but not ideal. Telora lacks
a way to name a model's associated Entity, Measure, Dimension, Capability, and
Plan families or to constrain generated records structurally. A future model
descriptor could reduce repetition only after those mechanisms are justified;
using `Any`, Dyn, or String ids now would be false reuse.

## Implementation result

Valid single- and multi-measure plans remain byte-for-byte equivalent at the
rendered SQL and wire-value level. The invalid recovery fixture still reports
all independent capability, compatibility, fan-out, ordering, and render
errors expected by the existing tests. No B2B concept was added to the shared
compiler.
