# RFC 0271: Struct Field Projection

- Status: Accepted
- Tracking: [#158](https://github.com/hh9527/telora/issues/158)
- Baseline: `8f4c085`
- Scope: Explicit field selection and renaming for nominal construction and update.

## Motivation

Strict struct updates require every patch field to belong to the target. A
projection explicitly selects useful fields from a larger source, catches
misspellings, and can rename fields without evaluating the source repeatedly.

## Syntax

```telora
let selected: Foo = source.{x, y as Y};
let updated = base <~ source.{x, y as Y};
```

Projection is a dot postfix expression with ordinary postfix precedence.
Entries are identifiers with optional `as` destination identifiers. Trailing
commas and empty projections are allowed. Source fields may be selected more
than once under distinct destination names. Destination names must be unique.

## Static Semantics

The source must be a nominal struct with a statically known field set, including
instantiated generic structs. Each source name must exist. Dict, Dyn, enums and
unconstrained type parameters are not field-shape evidence.

In ordinary expression position, the expected type must identify a nominal
struct. The projected destination set must exactly match its fields, and all
values must be assignable to their destination types. Field values retain their
existing nominal identities; projection does not recursively reinterpret them.
Expected types may come from annotations, function arguments or return contracts.
An uncontextualized projection is rejected; shapes do not select nominal types.
This proposal does not add delayed projection-specific type obligations.

As the immediate right operand of `<~`, projection provides update fields rather
than constructing an independent nominal value. Its destinations must be a
subset of the base fields with compatible values. Result identity comes from
the base. Every step of an update chain remains independently checked.

Empty projection constructs a nominal empty struct in an exact construction
context, or supplies an empty update. Standalone inferred anonymous projection
values and projection spreads inside update literals are outside this scope.

## Evaluation and Provenance

Evaluate the receiver exactly once before reading fields. Copy the selected
field Vals without changing their provenance or nested identities. Construct a
fresh container at the projection expression's location. Ordinary construction
attaches the target's runtime nominal identity; an update projection is an
internal field container consumed by the update instruction. The outer updated
container uses the update expression's location and base identity.

Normal failure propagation and allocation accounting apply. There is no
mutation, implicit field dropping, or deep merge.

## Implementation

Add a projection AST node and syntax suffix. Update receiver traversal, name
resolution and semantic indexing. Strict inference distinguishes nominal
construction from the direct update operand; preliminary evidence must not
invent a target identity. Lower receiver evaluation once, field reads and
MakeDict using existing instructions, then use existing nominal ownership
attachment for ordinary construction.

## Acceptance

Use Telora tests for selection, renaming, empty projection, generic source and
target, argument/return contexts, update chains and nominal equality. Diagnostic
tests cover missing context, missing source fields, duplicate destinations,
invalid sources, missing/extra target fields and nested nominal mismatch. Query
and diagnostic-provenance fixtures verify published identity and copied field
origins. Verify debug builds and workspace tests; do not build release binaries.
