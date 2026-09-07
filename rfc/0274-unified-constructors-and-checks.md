# RFC 0274: Unified Constructors and Construction Checks

- Status: Draft; stage-one representation implementation is in progress.
- Tracking: [#161](https://github.com/hh9527/telora/issues/161)
- Branch: `feat/0161-unified-constructors`
- Baseline: `83bb8a6`
- Related: RFC 0236, RFC 0237, RFC 0239, RFC 0270, RFC 0273.
- Delivery: staged commits and pushes, with progress on #161. Completion does
  not authorize merging into main; integration is a separate decision.
- Implementation: newtype declaration metadata, canonical identity, positional
  `.0` access, payload codecs, schema and Dyn tuple observation are implemented
  on the branch. Runtime callable constructors, generic application and
  first-class use and declaration-resolved newtype patterns are implemented.
  Tool-stage callable construction, named enum constructors and checks remain pending.
- Validation: debug build and workspace tests pass for the constructor pattern
  batch, including 296 language fixture groups. New behavior is tested in
  `.telora`; no release binary was built.

## Objective

Make newtype and enum payload constructors declaration-provided functions.
Resolve constructor identity through names, and infer generic arguments through
ordinary contextual inference. Subsequently enforce declaration-bound checks
at every construction boundary.

Implement in this order:

1. Newtype declarations, construction, first-class constructors and patterns.
2. Named enum constructors, member exports and removal of quoted tag syntax.
3. `Unchecked(T)` and `@check(func)`.

Each stage needs its own acceptance gate. Open questions in stage three do not
prevent work on stage one. This draft records proposed contracts, not existing
language behavior; unresolved decisions below must be settled before dependent
implementation is accepted.

## Existing Implementation

`syntax/telora/grammar.llw` accepts only named fields after `struct` and quoted
variant names after `enum`. `parser/patterns.rs::declared_type_initializer`
lowers declarations to private model operations over member dictionaries.
`parser/bindings.rs` records Struct/Enum declaration kinds separately from
ordinary type expressions.

`types/dependency.rs` reserves nominal declaration identities before evaluating
metadata. Types are tool-stage values; generic type families are callable and
produce `TypeOf` evidence. A newtype constructor cannot simply replace the
existing binding without breaking type application and metadata evaluation.

`TypeDescriptor::Declared` carries a declaration identity and a representation
body. The newtype path must retain identity through generic substitution,
publication, Dyn witnesses and runtime value construction. It must not lower
to an alias of its payload or masquerade as a named-field struct.

The grammar currently supports named export lists, not enum member exports.
The standard prelude is also supplemented by core prelude machinery; updating
only `modules/std/prelude.telora` will not establish all constructor bindings.

Runtime codec handling has separate Struct and Enum kinds and declared-value
wrapping. Newtype support needs an explicit metadata/codec contract, not only
a parser and type-checker change.

## Stage One: Newtypes

```telora
type UserId = struct(Int);
type Box(T) = struct(T);

let id = UserId(1);
let make: Fn(Int) -> UserId = UserId;
let boxed: Box(Int) = Box(1);
```

A newtype is a single-element named tuple, with exactly one payload type and
a distinct nominal identity. `value.0` reads its payload and preserves that
payload's type and source location. Other indices are rejected statically.
General multi-element named tuples are outside this implementation scope.
Wrapping and unwrapping are explicit. Neither the payload nor another newtype
with an identical payload is implicitly assignable to it. A tuple payload can
express multiple components without introducing positional multi-field structs.

Proposed pattern syntax is `UserId(value)`. This is an irrefutable pattern for
a known UserId. Ordinary functions returning UserId are not pattern
constructors. Pattern resolution retains the declaring identity independently
of the first-class function representation.

### Type and Value Names

A declaration supplies both type identity and constructor identity. Type
contracts resolve `Box(Int)` as a type application; value expressions resolve
`Box(1)` as construction. Constructor generic specialization uses existing
`@[Ty]` syntax.

Explicit Type expectations select the type facet, including reflection
arguments and functions whose result contract is Type. An unconstrained bare
newtype declaration name in a value binding denotes its constructor. HIR and
module interfaces retain declaration identity; arbitrary Type-valued bindings
and ordinary functions returning Type do not acquire a constructor facet.
Synthesized module exports preserve declarations and their generic contracts.
Runtime selection does not inspect argument values or constructor spelling.
Tool-stage construction still needs the same inference evidence propagated to
its expression compiler before this stage can be declared complete.

Imports, exports and aliases must preserve the two facets of the declaration;
they must not manufacture duplicate nominal identities. Duplicate source names
remain errors rather than a way to independently replace one facet.

### Representation and Integration

Preserve the established declared-value identity mechanism and Val provenance.
Payload references retain their source positions; construction records the
new value's construction position. Add explicit newtype metadata so reflection
can distinguish a newtype from its payload and from a named-field struct.

Proposed codec behavior is transparent payload encoding/decoding with nominal
wrapping on successful decode. This needs verification against schema and
decorator contracts before acceptance. Do not automatically inherit payload
traits such as Display or equality solely from representation equivalence.

Named-field projection and struct merge-update remain operations on named
fields; a newtype exposes its positional member `.0`, not a synthetic named
field. Its runtime container holds the original payload Val separately from
the outer declared identity, including when the payload is itself nominal.

### Acceptance Gate

- Distinct newtypes, generic instances, recursive metadata and module aliases
  preserve identity.
- Direct and first-class construction, explicit specialization and contextual
  generic inference agree.
- Patterns unwrap the matching declared type and reject unrelated constructors.
- Type-valued uses remain available alongside value constructors.
- Codec, schema, static reflection and Dyn witnesses describe the same type.
- Negative cases cover implicit wrapping/unwrapping and unresolved generics.

## Stage Two: Enum Constructor Names

```telora
type Event = enum { Progress(Int), Finished };

export Option.*;
export Result.*;

let result = Ok(1);
```

Payload constructors are functions; unit variants are values. A resolved name
determines the enum family. Context supplies generic arguments, including
arguments absent from a variant's payload; unresolved arguments still produce
an error when no permitted evidence determines them.

This replaces RFC 0273's deferred owner selection for authored constructors.
Do not search all enums by tag spelling or let a return annotation select a
different declaration for an already resolved name.

Define member-qualified references and member imports alongside exports.
Proposed qualified form is `Event.Progress`; validate its interaction with
existing module and field selection. Specify whether member wildcard exports
also introduce local bindings, how duplicates are diagnosed, and how import
aliases preserve constructor identity. These are acceptance prerequisites.

Prelude exports supply Option and Result members. Inventory Bool, FoldControl,
decorator-context enums and other builtin families and explicitly define their
member exposure. Bootstrap code must use the same declaration identities as
user modules without relying on the prelude to load itself.

Remove quoted forms in declarations, expressions and patterns together.
Migrate generated AST paths, embedded sources, language fixtures, examples,
guides and editor grammar. Underlying Atom/Tagged VM storage is outside the
surface-syntax removal scope.

Acceptance includes conflicting member names, qualified references, module
cycles, prelude bootstrap, higher-order constructors, unit variants, patterns
and complete-call generic evidence. Ordinary function aliases do not become
pattern constructors merely because their return types are enums.

## Stage Three: Construction Checks

Members remain public. `@check(func)` binds validation to a declaration rather
than relying on callers to invoke a trait method. Support named-field structs,
newtype structs and individual enum variants. Whole-enum checks are deferred.

`Unchecked(T)` is a distinct, builtin type derived from the representation of T.
It removes only the outer check guarantee. A field declared as another checked
type remains a valid value of that field type. Escaping an unchecked value does
not produce a checked T; there is no unchecked implicit conversion to T.

Construction evaluates and type-checks inputs, invokes the bound check, and only
then publishes a value of the target type. Compiler-controlled successful
construction performs the final wrapping.

Settle the following before implementation:

- Whether newtype checks receive the payload directly or Unchecked(T), and the
  exact access/pattern API of the unchecked representation.
- Variant checks receive payload evidence without introducing standalone
  variant types. Unit-variant checking and supported T domains need definition.
- The concrete check error contract; `Option(Error)` is a design placeholder,
  not an established standard-library type.
- Ordinary construction failure diagnostics and conversion of check failures
  into codec DecodeError with the original input Value.
- Check registration and execution phases, recursive types and termination
  behavior when checks construct further values.
- Reflection, generic metadata and codec handling of check functions.

Every creation path must enforce the same check: callable constructors,
contextual struct literals, merge-update and codec decoding. Projection into
a target struct is also construction. Passing, reading or copying an existing
checked value does not repeat its check.

Each merge-update produces a checked result. Therefore a chain checks each
intermediate result; optimization cannot remove observable failures. A single
patch containing several fields validates their combined result once.

Unchecked metadata or Dyn projection must not permit bypassing validation.
Decoding checked types must validate even though members are public.

## Delivery and Verification

Use focused `.telora` positive and negative fixtures for new behavior. Change
Rust tests only where host-level behavior requires them or existing embedded
source must migrate. Keep parser/editor grammar and diagnostics synchronized.

Run debug builds and appropriate tests for each implementation stage; complete
workspace verification before reporting implementation complete. Do not build
a release binary. Documentation outside RFCs describes supported current
behavior positively. Historical RFC amendments belong in their status areas.

Commit and push reviewable batches to the tracking branch and post evidence,
remaining work and decisions to #161 at stage boundaries. Keep the issue open
until implementation and acceptance are complete. Do not merge into main as
part of this authorization.
