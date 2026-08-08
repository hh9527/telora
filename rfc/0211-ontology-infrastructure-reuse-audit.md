# RFC 0211: Ontology-infrastructure reuse audit

- Status: Implemented
- Depends on: RFC 0208, RFC 0209, RFC 0210

## Summary

Audit what the B2B and B2C models actually share after the higher-order rule
phase. Correct earlier overclaims and distinguish shared infrastructure from
model-owned semantics.

This RFC measures use, not availability. An exported helper does not count as
enterprise reuse unless both models reach it through their executable paths.

## Current size context

At this revision:

```text
ontology-method/types.telora       93 lines
ontology-method/ontology.telora   251 lines

B2B model + relation model        765 lines
B2C model                         252 lines
```

These numbers describe maintenance surface, not a reuse percentage. B2B also
contains SQL AST integration, filters, restrictions, rendering, and a stable
wire protocol that B2C deliberately does not duplicate.

## Constructed by both

| TypeMetadata family | B2B instances | B2C instances |
|---|---|---|
| `MeasureDefinition` | MeasureCapability | MeasureCapability |
| `DimensionDefinition` | DimensionCapability | DimensionCapability |
| `RelationDefinition` | Relation with six-field join mapping | Relation with `{table, on}` mapping |
| `Compilation` | Query and GroupRequirement compilation | Measure and DimensionPlan compilation |

All generic parameters remain closed concrete model types. Physical relation
mapping differs structurally and is preserved as the Mapping parameter.

## Executed and tested by both

| Higher-order rule | Maintained invariant |
|---|---|
| `compile_requested` | requested id -> capability -> independent result -> completed values -> publication evidence |
| `compilation_complete` | successful publication only from complete evidence |
| `collect_required_nodes` | lowering outputs determine relation targets through typed projections |
| `classify_paths` | safe joins, fan-out-only targets, and unreachable targets are derived together |
| `verify_path_requirements` | fan-out/missing classifications reject the concrete authored Dimension subject |

The following lower-level functions are also executed by both through those
rules: `find_capability`, `lower_requested`, `completed`, `contains`,
`expand_once`, `close_six`, and `select_connecting_edges`. They are
implementation components, not counted again as separate high-level ontology
protocols.

## Available only or fixture only

| API | Evidence class |
|---|---|
| `verify_allowed` | available only; B2B retains richer restriction messages, B2C has no restriction input |
| `all_complete` | available only after Compilation replaced direct use |
| `Maybe`, `Many`, `Requirement`, older `Capability` | construction fixture only |
| `analytics.missing_names` | B2B only, result/render concern |

These APIs may remain useful, but they do not support a claim that two
enterprise ontology models share them.

## Model-owned in both

No concrete domain fact is shared:

- Entity, Measure, Dimension, Filter, semantic value, aggregation, and
  alignment variants;
- metric formulas and natural-grain policy;
- dimension compatibility and unavailable-capability choices;
- relation catalogs and cardinality declarations;
- physical tables, columns, aliases, predicates, and expression payloads;
- restriction data and authorization vocabulary;
- concrete intermediate plan structures; and
- final plan assembly and Host wire shape.

Both models still contain compiler code, but it now mainly sequences their
domain-specific stages around shared compilation and path rules. B2B's richer
pipeline remains substantially larger because it lowers through semantic,
relational, SQL, render, and encoded execution plans.

## Abstraction result by layer

| Layer | Result |
|---|---|
| Closed data and metadata expression | strong existing Telora capability |
| Definition-role types | shared by both after RFC 0208 |
| Capability compilation rule | shared by both after RFC 0210 |
| Relation classification and failure rule | shared by both after RFC 0209 |
| Grain/aggregation policy | represented by shared fields, semantics remain model-owned |
| Physical lowering | model-owned by design |
| Full ontology compiler pipeline | not shared as one abstraction |

The result is therefore stronger than “five generic helpers” but narrower than
a complete ontology compiler framework. Telora successfully abstracts several
higher-order rules while allowing their inputs and outputs to remain concrete.

## Language gaps exposed

1. User-generated metadata families cannot be named precisely as
   `TypeOf(F(A))` in another generic scheme. Continuation-style construction is
   safe but verbose.
2. Generic function bodies cannot always name surrounding scheme parameters in
   local annotations; result-context inference currently carries the relation.
3. TypeMetadata fixed-point dependency discovery missed Bool in the generated
   Compilation constructor. `Option(Array(Output))` provided a stronger
   publication protocol, but the discovery behavior remains an ergonomics gap.
4. Workspace module-result display widens exported generic schemes to Any even
   when strict checking and definition slots retain precision.
5. The relation algorithm remains explicitly bounded to six expansion rounds.

None required `Any`, Dyn, a compiler special case, or a Host-side parallel
checker in the model APIs.

## Honest conclusion

Telora can abstract from concrete rules to higher-order rules in this domain:
definition roles, capability compilation, path classification, and path
failure policy are now maintained once and exercised by two distinct models.

It has not shown that arbitrary ontology compiler pipelines collapse into one
framework. Business semantics and physical lowering remain intentionally
concrete, and the lack of a statically nameable user type family makes some
interfaces more continuation-heavy than their conceptual form.

The next evidence should come from Code Agent use or a third independently
designed analytics domain, not another model tailored solely to increase the
shared API count.
