# RFC 0210: Shared capability-compilation protocol

- Status: Implemented
- Depends on: RFC 0209

## Summary

Extract the repeated capability compilation skeleton into one typed protocol:

```text
requested identities
    -> independent capability results
    -> completed values
    -> completeness/publication evidence
```

Both B2B and B2C use it for measures and dimensions.

## Generated result type

`types.Compilation(Output)` generates:

```telora
{
    results: Array(Option(Output)),
    values: Array(Output),
    complete: Option(Array(Output)),
}
```

`complete` is not a detached Bool. `Some(values)` is evidence that every
requested capability produced a value; `None` prevents publication while raw
results and completed values remain available for best-effort downstream
diagnostics.

This shape keeps observation, continued analysis, and successful publication
in one typed value without introducing an accumulation effect.

## Higher-order compilation

`compile_requested` accepts requested identities, concrete generated
capabilities, typed id/lower projections, model input, and a continuation that
constructs the model's instantiated Compilation result.

Continuation-style construction is again required because the shared function
cannot name a user-generated family result in its own generic annotation. It
preserves the complete model type without `Any`.

`compilation_complete` reads the publication evidence, and
`collect_required_nodes` centralizes typed extraction of relation requirements
from completed outputs.

## Acceptance criteria

1. B2B and B2C both instantiate `Compilation` for measure and dimension output;
2. both use `compile_requested` for both capability families;
3. independent raw results remain available after incomplete lowering;
4. publication requires `complete: Some(values)`;
5. both use `collect_required_nodes` when assembling relation targets;
6. existing diagnostics remain independent and source-linked;
7. successful B2B SQL/wire and B2C plans remain unchanged; and
8. no `Any`, effect, VM, analyzer, or Host addition is introduced.

## Implementation result

B2B now has generated QueryCompilation and RequirementCompilation types. B2C
has generated MeasureCompilation and DimensionCompilation types. Their compile
functions no longer separately call `lower_requested`, `completed`, and compare
array lengths; one shared rule maintains that correspondence.

Both models use `collect_required_nodes` with model-owned projections. B2B
collects QueryPlan, GroupRequirement, and FilterRequirement entities; B2C
collects MeasurePlan and DimensionPlan entities.

This protocol is **constructed by both**, **executed by both**, and **tested by
both**. Final semantic/relational/SQL or execution-plan assembly remains local
because those concrete stages differ materially.

## Language observations

Two surface constraints shaped the implementation:

- a local annotation inside a generic function could not name the surrounding
  scheme's `Output` parameter, so inference and the result continuation carry
  that relation; and
- placing Bool directly in the generated metadata function exposed a
  fixed-point dependency limitation. Encoding completeness as
  `Option(Array(Output))` avoided the dependency and produced a stronger
  protocol rather than a workaround with Int or Any.

These should inform future type-constructor ergonomics, but neither blocks the
current strongly typed abstraction.
