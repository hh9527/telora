# RFC 0268: Remove Untagged Unions

- Status: Accepted
- Partial supersession by [RFC 0269](0269-remove-any.md): The provision retaining explicit Any is superseded by complete Any removal.
- Tracking: [#155](https://github.com/hh9527/telora/issues/155)
- Baseline: `f6164be`
- Scope: Removal of public untagged Union and explicit common-type contexts.
- Partial supersession: RFC 0052's public Union assignment rules and RFC 0075's
  arbitrary Union fallback for branch results. Historical RFC bodies remain unchanged.
- Implementation: Public constructor, metadata and runtime Union paths are removed.
  PendingAlternatives is a temporary inference representation; module publication,
  runtime type interning and metadata materialization reject unresolved candidates.

## Summary

Remove arbitrary untagged Union from the public type system. Remove the `union`
type constructor, stop inferring final Union types from incompatible collection
elements or expression branches, and remove Union from runtime type metadata,
reflection and codec contracts.

Use explicit enums for alternative domain values, Tuple for positional
heterogeneity, and an existing common contract where one is available. Preserve
Bool, Option, Result, user enums, explicit Any and Dyn, and `std/value.Value`.
Do not replace failed joins with implicit Any.

Inference may temporarily retain alternative constraints while solving a common
type. Those constraints are not a value type and must not escape into module
interfaces, canonical type identity, reflection, or runtime schema metadata.

## Motivation and Current Evidence

Union is not merely unused dynamic-language compatibility. Current code uses it
in several active paths:

- `types/relations.rs`: `join_types` falls back to `canonical_union`; collections
  and branches use this join.
- `types/inference-expression/core.rs`: Array, Dict, if and match result inference.
- `types/expression.rs`: a separate descriptor inference path, including boolean
  expressions and branch joins. Updating only GenericInference is insufficient.
- `types/graph.rs`, `types/descriptor.rs`, and the type store: analysis nodes,
  assignability and canonical type representation.
- `core.rs`, `types/prelude.rs`, `vm/model-type.rs`: constructor and metadata
  admission, including metadata that can be authored without calling `union`.
- `vm/codec-type.rs`, `vm/codec-transform.rs`, `vm/codec-schema.rs`: decoding,
  validation and schema generation for alternatives.
- `modules/std/type-desc.telora`, `vm/type-desc.rs`, `vm/dyn.rs`: reflection.

An independently reproduced defect at this baseline is:

```telora
type Item = struct {value: Int};
type Mixed = union('None, [Item, Int]);
def item: Item = {value: 42};
def mixed: Mixed = item;
```

The checker rejects the assignment with
`cannot unify {value: Int} with Int | Item`. It exposes the declared body before
matching the expected Union. This is a fixable implementation defect, not proof
that unions are inherently unsound. The reason to remove Union is a deliberate
choice for explicit alternatives and a smaller combination surface.

The current public specification also explicitly accepts `[1, "one"]` as
`Array(Int | String)`. Removing that behavior is a breaking language change.

## Proposed Common-Type Rules

The following are draft recommendations, not a description of current behavior.

| Construct | Proposed rule |
| --- | --- |
| Explicit expected type | Check every reachable contributor against that contract; propagate literal construction context recursively. |
| Identical types | Keep the same type, including canonical nominal identity. |
| Never | A non-returning contributor does not constrain the common type. |
| Empty collection | Use contextual element type or evidence from other compatible contributors; report unresolved types where no existing empty-collection rule settles them. |
| Array and Dict | Infer one element/value contract; reject incompatible contributors without a common type. |
| Tuple | Preserve distinct positional types; `(1, "one")` remains valid. |
| if, if let, match, return | Require a common result contract across reachable results, including explicit returns and the tail expression. |
| Nested structures | Apply the same rules at corresponding elements, fields and payload positions. No new implicit field dropping is introduced. |
| Different nominal identities | Do not merge by equal payload shape. Require an explicit alternative or already-supported abstraction boundary. |
| Existing explicit Any | Preserve the existing erased contract; never invent Any solely to make incompatible contributors pass. |
| Dyn | Keep the existing explicit packing/evidence requirements. |
| Generic arguments | Solve constraints on the same instantiated type variable; incompatible constraints are errors, not a reason to infer Union. |

An existing explicit common contract can accept differently shaped values if
the current assignability rules allow it. This proposal does not redefine every
subtyping or structural compatibility rule. It removes the arbitrary Union
fallback and must not add implicit numeric conversion or nominal rebranding.

Generic inference must remain independent of argument ordering. Literal
construction may need to wait for evidence from other arguments, return
constraints or callbacks. Previously constructed anonymous values are not
retroactively branded. Unsolved constraints and incompatible constraints must
produce different diagnostics.

## Tagged Alternatives

Bool, Option and Result are retained. Expressions such as the following remain
valid under their expected contracts:

```telora
def maybe: Fn(Bool) -> Option(Int) = fn(found) {
    if found { 'Some(42) } else { 'None }
};
type Scalar = enum {'Integer(Int), 'Text(String)};
def values: Array(Scalar) = ['Integer(1), 'Text("one")];
```

Without a common type supplied by context or existing type evidence, different
tagged alternatives are an error. Do not infer a new structural Enum from
`'None` and `'Some(42)`. Authors can supply context through `value.ty!(Ty)`,
explicit generic application `func@[Ty](...)`, or an existing type annotation.
The context must reach the constituent expressions before their join is finalized.

For example, `(if found { 'Some(42) } else { 'None }).ty!(Option(Int))`
supplies the required common contract. A generic call whose shared result type
would otherwise remain ambiguous can supply that contract with `@[Option(Int)]`.
This requirement does not make all inference illegal: identical types, existing
nominal evidence, and built-in operations with a known Bool result remain valid.

Different payload types for the same tag require a common payload contract.
`'Some(1)` and `'Some("one")` must not silently introduce an untagged payload
Union. Use distinct tags, an explicit nested enum, or an explicit dynamic
contract when that is the author's intent.

Contextual checking must preserve Option/Result assignment and match
exhaustiveness. Previously inferred helpers that combined different variants
without a common contract must add explicit context. Queries and module
interfaces must never publish a synthesized enum as a fallback for this error.

## Migration Examples

| Current code or behavior | Proposed replacement or result |
| --- | --- |
| `union('None, [Int, String])` | An explicit enum such as Scalar above. |
| `[1, "one"]` | `(1, "one")` for a positional pair, or an Array of explicitly tagged values. |
| `if condition { 1 } else { "one" }` | Error; use a common contract or explicit enum constructors. |
| Array of different named structs | An enum with separate payload variants; equal fields do not establish common identity. |
| `'Some(value)` / `'None` branches | Require a common enum context, such as `.ty!(Option(T))`, a result annotation, or explicit generic application. |
| A function with incompatible explicit returns | Error naming the conflicting returns and expected contract. |
| Comparisons between Union variants | Compare values of a declared common enum, or intentionally use an existing explicit dynamic boundary. |
| Reflection of kind `'Union` | No accepted final type of that kind; migrate consumers to their enum or explicit dynamic model. |
| A Union codec schema accepting multiple raw shapes | Use an explicit enum; existing `@codec.untagged` may preserve the wire shape where applicable. |

Enum wrapping changes value construction and can change serialization. It is
not a transparent rewrite of an existing Union schema. Preserve the existing
untagged enum decoder, but introduce no compatibility decoder for removed Union
metadata. Ambiguous enum payloads still follow the existing codec policy.

## Host Data and Runtime Boundaries

`std/value.Value` already represents data through tagged variants, including
recursive Array(Value) and Dict(Value). JSON parsing exposes this model; removing
Union does not require removing heterogeneous external data or inventing a new
universal value representation. Source admission, provenance and source budgets
are unchanged by the proposal.

There are two distinct codec layers. `json.parse` returns `Result(Value, ...)`,
and `codec.decode` accepts Value as input. However, target-type decoding and
schema generation still accept `CodecKind::Union`; `vm/codec-transform.rs`
currently tries each Union variant. Remove these target-type branches directly.
They are not needed to represent the generic Value input.

Keep `JsonUntagged` / `@codec.untagged`: they control enum wire representation,
not language-level Union. `std/value.ScalarValue` already uses an untagged enum.
The enum's language values retain their tags and type contract even when those
tags are omitted on the wire. No redesign of Value or new data carrier is required.

Runtime and tool-stage metadata decoders must reject a manually authored
`{kind: 'Union, ...}` type description as well as the removed constructor. Merely
removing the prelude binding would leave an inconsistent alternate entry point.

Union currently contributes to type storage and codec schemas. Audit every
persistent or Host-visible format that can contain those shapes. Reject removed
Union metadata at these boundaries; provide no deprecation window or compatibility
mode. Do not assume there is a persistent format merely
because an in-memory type store exists.

Removing Union-specific `anyOf` schema generation does not imply banning all
`anyOf`/`oneOf` output: tagged enums may legitimately need alternatives too.
Preserve their existing serialization contracts and test the resulting schemas.

## Implementation Plan

1. Inventory positive and negative tests under the explicit-common-type rule,
   and all public Union entry points. Separate semantic tests from
   tests that happen to depend on an internal representation.
2. Introduce shared fallible common-type solving with contextual enum handling.
   Migrate both inference paths, returns, collections, generic constraints,
   projection and diagnostics. Keep any temporary candidate representation
   private to inference.
3. Migrate source and standard-library tests. Remove the public constructor,
   metadata kind, module-interface representation and runtime admission paths in
   a coordinated breaking change, without a deprecation release or compatibility window.
4. Remove dead Union machinery from canonical types, reflection and codecs;
   retain and verify enum behavior. Update LANGUAGE and relevant guides after
   acceptance and implementation, not while this RFC remains a draft.

Intermediate commits may retain internal Union code during the transition, but
the completed implementation must have no accepted public untagged Union type.
Use an explicit invariant at interface publication to catch candidate leakage.

## Validation and Diagnostics

Prefer pure Telora tests for user-visible behavior:

- Positive tests for Bool, Option/Result, nested enums, explicit return paths,
  empty collections with evidence, Never, generics, and imported nominal types.
- Negative tests for heterogeneous arrays/Dicts, incompatible branches and
  returns, conflicting generic constraints, same-tag incompatible payloads,
  distinct nominal identities, and every removed public constructor route.
- Argument, branch and element permutation tests proving order independence.
- Tests preserving explicit Any, Dyn and heterogeneous `Value` inputs, without
  allowing inference to invent Any as a fallback.
- Reflection, codec and schema tests for tagged enums and rejected Union
  metadata; Rust tests for interface/type-store invariants and Host boundaries.

Diagnostics should identify both conflicting origins and suggest an enum or
Tuple when appropriate. Internal candidate sets must not be printed as if they
were valid public Union types. Existing macro-generated nodes can share source
locations, so constraints must retain expression-specific type evidence rather
than treating location alone as semantic identity.

## Accepted Decisions and Coverage

The design direction is settled: require explicit context instead of synthesizing
an enum, remove Union directly, and preserve the existing enum Value codec model
and untagged enum representation. Implementation coverage includes:

- Positive and negative examples showing that `.ty!(Ty)` and `@[Ty]`
  reach nested branches, collections and callback returns before finalization.
- Removal of codec Union-specific paths and verification that existing
  enum/Value and `JsonUntagged` behavior remains intact.
- Status metadata identifying the superseded historical RFC provisions.

The language change is intentionally breaking. Built-in `should_ok!` and
`try_unwrap!` expansions retain their specified Option result contract. Compiler
generated match expressions receive that context rather than synthesizing a
public enum from unconstrained user branches.

## Alternatives

Keeping Union and fixing nominal assignment is technically feasible and has
lower immediate compatibility cost. It retains the ongoing interaction surface
across inference, assignment, generic evidence, reflection and codecs.

Removing only the constructor preserves inferred public Union and does not
achieve this proposal's semantic simplification. Replacing Union with Any loses
static information and is explicitly rejected. Synthesizing a structural enum
from unrelated variants was considered and rejected in favor of explicit common
type context; the resulting helper-function migration is an accepted tradeoff.

## Non-Goals

This proposal does not remove Any/Dyn, alter the module catalog, change test
execution, fix trait import aliases, or independently settle the callback
literal-context defect. Those issues should not be hidden inside Union removal.
