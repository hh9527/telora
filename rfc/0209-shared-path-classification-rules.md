# RFC 0209: Shared path-classification rules

- Status: Implemented
- Depends on: RFC 0208

## Summary

Move complete relation-path classification and requirement-oriented
fan-out/missing verification into ontology-method. Previously both models used
the same closure primitives but independently maintained the actual invariant.

## Classification

`classify_paths` accepts safe and fan-out edge catalogs, base and target nodes,
and typed endpoint selectors. It computes:

- safe connecting edges;
- targets reachable only through a fan-out edge; and
- targets unreachable through either catalog.

The two models have different PathPlan records. Instead of erasing the result
or requiring a generic named tuple type, the function accepts a continuation:

```telora
classify_paths(
    safe_edges,
    fanout_edges,
    base,
    targets,
    from,
    to,
    fn(joins, fanout, missing) { model_path_plan(...) },
)
```

This continuation-style constructor preserves each model's exact Result type
under ordinary rank-1 generics.

## Verification

`verify_path_requirements` receives fan-out and missing nodes, model-specific
Requirement values, and two projections:

```text
required_node : Requirement -> Node
subject_of    : Requirement -> Subject
```

The first connects the requirement to path classification. The second retains
the authored model subject used for blame. The shared rule reports grain
expansion or a missing verified path without replacing Dimension with a generic
Node location.

## Acceptance criteria

1. neither B2B nor B2C implements closure/classification locally;
2. neither implements its own fan-out/missing diagnostic loops;
3. distinct PathPlan records remain statically checked;
4. B2B failures retain the original Dimension subject and established message
   categories;
5. B2C reports ProductCategory through the same shared grain rule;
6. valid physical join selection remains unchanged; and
7. no `Any`, Dyn, VM, analyzer, or Host addition is used.

## Implementation result

Both relation planners now call `classify_paths` and construct their local
PathPlan through a typed continuation. Both model verifiers call
`verify_path_requirements` with local required-entity and source projections.

This is the first higher-order rule in the experiment that is **executed by
both**, **tested by both**, and owns a complete domain invariant rather than a
single graph helper. Relation catalogs, cardinality declarations, physical
mappings, and policy subjects remain model-owned.

The attempted direct generic tuple result exposed a surface boundary: a
parameterized `Tuple([Array(Edge), ...])` could not be written directly as this
function's generic result annotation. Continuation-style construction solved
the problem without weakening types and is useful evidence for Telora's
higher-order function route.
