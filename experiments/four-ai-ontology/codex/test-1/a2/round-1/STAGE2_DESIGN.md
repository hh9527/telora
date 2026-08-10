# Stage 2 design

## Layout and surface

- `src/definitions.telora`: metadata families for capability, relationship, compilation evidence,
  measure, and dimension roles.
- `src/capabilities.telora`: generic independent compilation and complete-evidence construction.
- `src/relationships.telora`: bounded graph classification and provenance-aware reporting.
- `src/analytics.telora`: the single shared analytics orchestration entry point.
- `src/lib.telora`: public re-export surface.

Metadata families return `Type`, while rules quantify the actual enterprise record types separately.
Continuation builders construct evidence and classification records that generic Telora signatures
cannot name precisely. Typed accessors recover their fields in the analytics entry point.

## Capability strategy

`compile_capabilities` maps every request without early failure. It returns through a caller builder
with aligned `results`, flattened successful `values`, and an `Option(Array(Output))` complete
witness constructed by a fold. The shared layer owns missing-capability diagnostics.

## Relationship strategy

Forward expansion runs separately over safe edges and over the union of safe plus fan-out edges.
Reverse safe expansion from required targets prunes selected safe edges that are merely reachable
side branches. Fan-out-only and missing targets are computed from the two forward reachability sets.
All expansions are explicitly unrolled to depth eight.

## Analytics ordering and atomicity

The entry point compiles both catalogs, combines successful measure values, classifies nodes derived
from the combination and successful dimensions, reports represented authored requirements, and
evaluates the final builder. It checks complete evidence only at the publication gate. Thus partial
values may support diagnostics but cannot become an observable successful plan.

If combination itself returns `None`, relationship and final-builder phases cannot be meaningfully
typed because there is no base node or combined value; processing returns `None` at that boundary.

## Costs and boundaries

The long signature is a direct cost of preserving closed types without traits, associated types, or
type erasure. Evidence/path builders and accessors are mechanical boilerplate. Depth eight is the
main semantic limit. Measure and extra-node diagnostic provenance requires callers to represent
those authored sources in the common requirement array; dimension requirements are the primary
profile supported in this version.
