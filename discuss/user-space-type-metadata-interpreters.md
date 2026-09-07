# User-space TypeMetadata interpreters

- Stage: Discussion
- Scope: typed lifting of user-defined erased interpreters
- First vertical slice: equality
- Related: `type-directed-capability-factories.md`, RFC 0055, RFC 0085

## Objective

Telora should let ordinary Telora code interpret canonical TypeMetadata with the
same public logical reach as a native interpreter. Native code may retain heap,
cache, and execution optimizations, but it should not be the only place where
users can define nested type-directed semantics.

The target is not dynamic code generation:

```text
TypeOf(A) -> generated Telora code specialized for A
```

It is user-space data interpretation behind a typed outer boundary:

```text
TypeOf(A) + erased interpreter + values of A
    -> deterministic interpreted result
```

Equality is the first vertical slice because its result type is fixed, Telora
already has authoritative native behavior for conformance, and it exercises
the logical data shapes without requiring dynamic construction of an `A`.

## Proposed form

The erased interpreter is an ordinary recursive Telora function:

```telora
type InterpreterError = struct {message: String, value: Dyn};

def my_eq_i:
    Fn(Dyn, Dyn) -> Result(Bool, InterpreterError) =
    fn(left, right) {
        # Inspect dyn.desc(left), project both values, then recurse normally.
        my_eq_i(child_left, child_right)
    };
```

A contextual `interpreter` expression lifts it into a statically typed
factory:

```telora
def eq_fn:
    for(A) Fn(TypeOf(A)) ->
        Fn(A, A) -> Result(Bool, InterpreterError) =
    interpreter(my_eq_i);
```

`interpreter` is a keyword, not a Function binding. Its call-like spelling
does not imply runtime lookup or ordinary Function application.

This separated form is preferred to an inline interpreter body. The erased
function can use ordinary Telora recursion, can be tested directly, and does
not need a special recursive capability supplied by the runtime.

## Contextual typing and erasure

`interpreter(expression)` is meaningful only under a complete expected scheme.
For the first version, the accepted target shape is:

```text
for(A) Fn(TypeOf(A)) -> Fn(P0(A), ..., Pn(A)) -> R
```

with these restrictions:

- exactly one quantified type parameter and one `TypeOf(A)` witness;
- the outer Function takes only that witness;
- the result is one ordinary monomorphic Function;
- each value position depending on `A` can be safely packed as `Dyn`;
- `R` does not contain `A`;
- no callback parameter contains `A`; and
- no higher-rank scheme, overload, variadic parameter, subtype, or coercion is
  introduced by the lifting.

The compiler derives the required erased operand type mechanically. For
example:

```text
Expected:
for(A) Fn(TypeOf(A)) -> Fn(A, A) -> Result(Bool, InterpreterError)

Operand requirement:
Fn(Dyn, Dyn) -> Result(Bool, InterpreterError)
```

Conceptually, the compiler lowers it to an ordinary adapter:

```telora
fn(type_witness) {
    fn(left, right) {
        my_eq_i(
            trusted_dyn_pack(type_witness, left),
            trusted_dyn_pack(type_witness, right),
        )
    }
}
```

The generated adapter preserves the relationship between the static `A`, its
runtime witness, and each packed value. There is no runtime `interpreter`
callable, type-specific bytecode generation, or implicit capability search.

Initially, `interpreter` should be accepted only as the initializer of a `def`
with an explicit contract. General contextual occurrences can be considered
after diagnostics and inference are stable.

## Consumers before transformers

The first version supports interpreters whose result does not contain `A`:

```telora
for(A) Fn(TypeOf(A)) -> Fn(A, A) -> Bool
for(A) Fn(TypeOf(A)) -> Fn(A) -> String
for(A) Fn(TypeOf(A)) -> Fn(A) -> Result(Array(Change), InterpreterError)
```

It deliberately defers type-preserving outputs:

```telora
for(A) Fn(TypeOf(A)) -> Fn(A) -> A
for(A) Fn(TypeOf(A)) -> Fn(A) -> Option(A)
for(A) Fn(TypeOf(A)) -> Fn(Value) -> Result(A, codec.DecodeError)
```

Those forms require dynamic construction plus validation against the original
`TypeOf(A)` witness. Packing inputs as `Dyn` is a one-way safe operation;
reconstructing an `A` is a separate trusted boundary.

Higher-order inputs such as `Fn(A) -> Bool` are also deferred because they need
recursive adapter wrapping rather than simple value erasure.

## `TypeOf(A)` and `TypeDesc`

`TypeOf(A)` is the precise static witness used at the trusted boundary.
`TypeDesc` is the erased logical descriptor passed to user code:

```text
TypeOf(A) --trusted one-way projection--> TypeDesc
```

`TypeDesc` should be opaque and unforgeable. User code observes it through
safe functions; it does not receive raw heap handles, VM up-links, or a way to
claim that an arbitrary descriptor is `TypeOf(A)`.

The public descriptor is a finite type graph, not a tree that implicitly
unfolds forever. Recursive edges are explicit `$ref` nodes. A conceptual
descriptor for a recursive list may look like:

```text
$type {
    id: 0,
    body: Enum {
        Nil: None,
        Cons: Record {
            head: $param(0),
            tail: $ref(0),
        },
    },
}
```

The exact data representation is not public ABI, but the logical distinction
is:

- ordinary data nodes such as Struct, Enum, Array, and Dict;
- `$ref`, a reference within the current descriptor graph; and
- `$param`, a generic parameter before or during instantiation.

A reference identifier is graph-local, not a global type identity. The public
API may expose an opaque `TypeRef`, or keep identity hidden and expose only
safe inspection and resolution:

```telora
type_desc.kind: Fn(TypeDesc) -> TypeDescKind;
type_desc.resolve: Fn(Type) -> Result(Type, type_desc.ResolveError);
```

The current hidden VM up-link may implement this behavior, but it must not leak
as the public representation.

## Functions are descriptor leaves

For data interpreters, a Function value is an opaque, indivisible entity. The
first public `TypeDesc` view therefore exposes `Function` as a kind but does
not expose or traverse its parameter and result descriptors.

This means recursive references below Function parameters or results are not
part of the first interpreter ABI. The compiler's internal type representation
remains complete; only the user-facing data interpretation view is restricted.

An equality interpreter can reject a Function, delegate to an authoritative
opaque operation, or use another explicitly provided atomic policy. It cannot
inspect Function contents or raw identity. Signature reflection, if needed,
should be designed separately from this data-interpreter mechanism.

## Value recursion, not type-plan recursion

Telora currently has recursive types and recursive functions, but no cyclic
runtime values. A value of a recursive type is therefore a finite tree. An
interpreter descends through a child value and its corresponding child
descriptor together:

```text
my_eq_i(desc, left, right)
    -> observe child_desc, child_left, child_right
    -> my_eq_i(child_desc, child_left, child_right)
```

The value becomes structurally smaller even when `child_desc` resolves through
`$ref` to an earlier type node. Ordinary Telora recursion is sufficient. The
first version needs no framework-owned open-recursion dispatcher, visited-pair
set, recursive capability plan, or memoized field/variant capability factory.

An interpreter that walks TypeDesc without following a value must handle
`$ref` explicitly and decide its own termination policy.

If cyclic runtime values are introduced later, each operation will need an
explicit cycle contract. Equality, rendering, hashing, and encoding need not
share the same policy, so the first version should not pre-emptively impose one.

## `Dyn`: descriptor and value kept together

The public interpreter ABI uses an opaque `Dyn` package that retains a value
and the canonical witness for its type together:

```text
Dyn = exists A. {
    desc: TypeOf(A),
    value: A,
}
```

This is a semantic model, not a user-constructible Struct. Only trusted
lowering and native observer code can create a `Dyn`; ordinary Telora code
cannot forge the relationship between its descriptor and value. The VM stores the witnessed value internally and observers preserve that
relationship when returning child packages.

Introducing `Dyn` does not introduce a global `Unknown` top type, implicit
subtyping, or arbitrary value coercion. If codec, reflection, and module
boundaries later demonstrate a general need for `Unknown`, that should be a
separate type-system decision.

The minimal read-only API is:

```telora
dyn.desc: Fn(Dyn) -> TypeDesc;
dyn.kind: Fn(Dyn) -> ValueKind;
dyn.field: Fn(Dyn, String) -> Result(Dyn, dyn.AccessError);
dyn.array_items: Fn(Dyn) -> Result(Array(Dyn), dyn.AccessError);
dyn.tuple_items: Fn(Dyn) -> Result(Array(Dyn), dyn.AccessError);
dyn.tag: Fn(Dyn) -> Result(String, dyn.AccessError);
dyn.payload: Fn(Dyn) -> Result(Option(Dyn), dyn.AccessError);
```

Each structural observer checks the descriptor and runtime shape together,
selects the corresponding child descriptor and child value, and returns a new
unforgeable `Dyn`. Struct/Record observation is therefore not implemented as
an unchecked Dict lookup that happens to use the same field name.

The public `ValueKind` covers logical values only. It does not expose storage
generations, registers, raw handles, shapes, or metadata up-links.

At a statically known leaf, user code narrows `Dyn` through checked helpers:

```telora
dyn.check_int: Fn(Dyn) -> Option(Int);
dyn.check_string: Fn(Dyn) -> Option(String);
dyn.check_bool: Fn(Dyn) -> Option(Bool);

dyn.expect_int: Fn(Dyn) -> Result(Int, dyn.AccessError);
dyn.expect_string: Fn(Dyn) -> Result(String, dyn.AccessError);
```

`check_*` supports branch selection; `expect_*` preserves diagnostics and
paths. Wrong-kind projection is a value-level failure, never memory unsafety
or an unrecoverable VM type error.

A later generic projection is possible:

```telora
dyn.check: for(A) Fn(TypeOf(A), Dyn) -> Option(A);
dyn.expect: for(A) Fn(TypeOf(A), Dyn) -> Result(A, dyn.AccessError);
```

It is not required by the first equality slice. It needs canonical descriptor
comparison and a trusted recovery of `A`, so concrete primitive projections
should establish the boundary first.

An unchecked `static_cast` is not a normal Telora capability. The compiler may
use an equivalent trusted operation while packing a statically checked `A`,
and native projections may use it only after validating the descriptor.

Runtime-selected structure is not cast wholesale to `typeof(desc)`. That
would attempt to turn runtime metadata back into a static type. Instead, the
interpreter projects child `Dyn` packages and narrows only concrete leaf kinds.

## Invariant and blame

The generated adapter establishes the root invariant:

> Every root `Dyn` packages a value statically accepted as `A` with the
> `TypeOf(A)` witness supplied to the generated adapter.

Observer APIs preserve it by returning a new opaque package for a
corresponding child descriptor and value. A checked leaf projection should
therefore fail only when the interpreter chose an inconsistent branch, a
malformed dynamic value entered through another boundary, or trusted code
violated the observer contract.

Logical interpretation failures are values:

```text
wrong logical kind
failed checked projection
missing field
unknown variant
unsupported opaque node
policy rejected by attributes
```

They return `InterpreterError`. Paths can initially be constructed by the ordinary
Telora interpreter as it recurses; they do not require a hidden dispatcher.

Execution failures remain VM/query failures:

```text
fuel exhausted
allocation quota exhausted
call depth exhausted
cancelled query
stale revision
trusted native bug
```

Ordinary recursive execution already uses shared resource and cancellation
machinery. A cancelled or stale query publishes no partial typed capability.

## Native and Telora parity

Native and Telora implementations need semantic parity, not implementation
parity. User code must be able to observe every public data node and value
shape needed by the operation. Native code may retain:

- raw heap handles and object layout;
- persistent caches and interning;
- bulk heap access and specialized traversal;
- optimized `$ref` resolution; and
- future bytecode or JIT optimization.

For opaque leaves, the boundary must be explicit. A native implementation may
offer a safe atomic operation that user code can call, while raw identity
remains private. That is a defined fallback capability, not silent native-only
structural access.

Automatic fallback chains and nested override dispatch are not required for
the first slice. Users can compose ordinary interpreter functions explicitly.
If repeated operations demonstrate a need for a common fallback protocol, it
can be added as a library abstraction without changing typed lifting.

## Current gap audit

### Already available

- canonical TypeMetadata for primitive, Array, Dict, Tuple, Struct, Enum,
  Union, Function, Atom, Tagged, attributed, and named types;
- `TypeOf(A)` witnesses and explicit type application;
- normalized attribute wrappers and attribute access helpers;
- arbitrary Dict key/value/pair enumeration and Dict construction;
- generic Array traversal combinators;
- ordinary recursive Telora definitions with explicit contracts;
- typed higher-order adapters when the type argument is explicit;
- shared fuel, allocation, call-depth, cancellation, and publication machinery;
- structural native equality as a conformance reference; and
- no cyclic runtime values, which keeps the initial recursion model finite.

### Missing or incomplete

1. `interpreter(expression)` contextual typing, signature erasure, diagnostics,
   and trusted adapter lowering do not exist.
2. `TypeOf(A)` evidence does not currently infer `A` through the tested
   higher-order `eq_fn(Int)` result; explicit `eq_fn[Int](Int)` works.
3. Recursive metadata up-links have no stable public `$ref`/resolution view.
4. The public data-interpreter view has not yet made Function an explicit leaf.
5. There is no opaque `Dyn` value that binds a descriptor to an erased runtime
   value, nor trusted interpreter lowering that constructs it.
6. Struct/Dict field projection does not yet return a checked child `Dyn`.
7. Array and Tuple have no safe dynamic item observer returning child `Dyn`.
8. Tagged/Enum values have no dynamic tag and payload observer.
9. There is no public logical runtime `ValueKind`.
10. There are no checked primitive `Dyn` projections with value-level blame.
11. Union selection needs logical kind and validation helpers.
12. Recursive-reference and malformed-descriptor behavior is not yet a stable
    public interpreter ABI.
13. User-defined transformers need construction and validation back into `A`;
    consumers deliberately avoid that larger problem.

Visited sets, plan memoization, dispatcher-owned paths, and automatic fallback
chains are no longer initial gaps. They are deferred features whose need must
be demonstrated independently.

## Equality validation slice

The first experiment should compare a Telora reference interpreter with native
structural equality for the supported public data domain:

| Case | Native | Telora |
| --- | --- | --- |
| scalar | same result | same result |
| Array/Dict | same result | same result |
| Tuple/Struct | same result | same result |
| Atom/Tagged/Enum | same result | same result |
| attributes | same declared policy | same declared policy |
| recursive type, finite value | terminates | terminates |
| opaque Function | same explicit atomic policy or rejection | same |
| malformed observation | same blame category | same blame category |
| cancellation | atomic | atomic |
| quotas | bounded | bounded |

Cyclic values and Function signature traversal are outside this table because
they are outside the initial language/runtime domain and public descriptor
view, respectively.

## Staged validation plan

Before a numbered RFC series:

1. specify contextual `interpreter` typing and erased-signature derivation;
2. fix or explicitly isolate the `TypeOf(A)` higher-order inference gap;
3. define opaque `TypeDesc`, explicit `$ref`, and safe resolution;
4. define Function as a leaf in the public descriptor view;
5. implement opaque `Dyn`, concrete checked projections, and minimal read-only
   structural observers;
6. lower one annotated `interpreter(my_eq_i)` to an ordinary adapter;
7. implement finite recursive equality in ordinary Telora code;
8. compare it with native equality on the supported data domain; and
9. only then extract an umbrella RFC and independently executable child RFCs.

The first RFC series, if justified, should remain consumer-oriented and
Eq-first. A second operation such as rendering or stable hashing should test
whether the lifting and observer boundaries are genuinely reusable.

## Success boundary

The experiment succeeds when ordinary Telora code can:

- receive an opaque, recursively observable `TypeDesc`;
- observe matching finite runtime values through safe `Dyn` APIs;
- recurse as an ordinary Telora function;
- use checked leaf projection;
- define equality for every supported public data shape; and
- expose it as `Fn(A, A) -> Result(Bool, InterpreterError)` through one trusted,
  mechanically checked `interpreter` lift.

It fails if the lift relies on unchecked user assertions, if meaningful public
data cases remain visible only through VM internals, or if `$ref` cannot be
given a stable logical contract. Such a failure should narrow the claim rather
than expose raw heap state or introduce a trait system.

## Open questions

1. Should `interpreter` initially accept only a named Function binding, or any
   expression matching the derived erased signature?
2. Which concrete `dyn.check_*` and `dyn.expect_*` projections are required by
   the first equality interpreter?
3. Should `$ref` identity be observable as an opaque `TypeRef`, or should the
   first API expose only `resolve`?
4. How are generic `$param` nodes represented after full instantiation?
5. Which Union-selection helper is sufficient without turning runtime metadata
   into a static type?
6. What atomic policy, if any, should native code expose for opaque Functions?
7. Must the initial `eq_fn(Int)` call infer `A`, or may the first prototype
   require `eq_fn[Int](Int)`?
8. Should the lifted equality return `Bool` or `Result(Bool, InterpreterError)` after
   construction and observer contracts are proven?
9. Should a general `Unknown` ever subsume `Dyn`, or is the existential package
   intentionally limited to reflection and interpretation?
