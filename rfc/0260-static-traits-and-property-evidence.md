# RFC 0260: Static Traits and Property Evidence

- Status: Implemented
- Tracking: #129
- Depends on: RFC 0258, RFC 0259

## Summary

Telora adds nominal static traits, coherent implementations, generic trait
bounds, explicit trait-member calls, and a built-in `Property(P)` constraint.
Trait calls use dictionary elaboration and execute ordinary Telora functions.
Typed properties remain independent metadata values and can provide the data
used by a trait implementation.

The first standard capability is `fmt.Display`. A successfully published
`fmt.DisplayBy` property supplies `Display` evidence through one standard
blanket implementation. Interpolating a value requires primitive display or a
statically resolved `Display` implementation.

## Surface syntax

### Trait declaration

```telora
trait Display {
    display: Fn(Self) -> String,
};
```

A trait declaration introduces one nominal `TraitId`. `Self` is bound only in
the trait member contracts. The first version accepts function members only;
member names are unique and ordered canonically by UTF-8 bytes.

A trait is capability metadata, not an inhabitable value type. Tooling may
inspect its identity and member contracts. Runtime program code does not
construct trait metadata or a trait object.

### Explicit implementation

```telora
impl Display for Endpoint {
    display: fn(self) { fmt.display(Endpoint, self) },
};
```

An implementation is a top-level static declaration. Replacing `Self` with the
target type must make every supplied member match its trait contract exactly.
Every required member appears once and an implementation cannot add members.

### Generic bounds

```telora
def render: for(T: Display) Fn(T) -> String = fn(value) {
    Display.display(value)
};
```

Multiple bounds use `+`:

```telora
for(T: Display + Debug, U: Hash) Fn(T, U) -> String
```

`+` in a type-parameter bound is conjunction. It does not construct a Union.
A published `TypeScheme` retains every bound by canonical `TraitId`, so an
imported generic contract has the same obligations as its provider.

### Explicit member calls

```telora
Display.display(value)
value |> Display.display
```

Trait qualification is required. The first version has no receiver-method
lookup and does not add `value.display()`.

The selected implementation is determined statically from the operand type and
the lexical evidence environment. A missing or ambiguous implementation is a
type diagnostic at the trait-member expression.

### Property constraint

`Property(P)` is a built-in parameterized constraint:

```telora
for(T: Property(fmt.DisplayBy)) Fn(TypeOf(T), T) -> String
```

For a successfully sealed module, `T: Property(P)` proves that
`get_type_prop(T, P)` is `Some(P)`. The public reflection function keeps its
general `Option(P)` result outside such an evidence scope.

Property evidence depends on the effective published head for the exact
`Ty(T, P)` key. Field and Variant properties do not satisfy `Property(P)` for
their owner type.

### Property-backed blanket implementation

```telora
impl for(T: Property(DisplayBy)) Display for T {
    display: fn(value) { fmt.display(T, value) },
};
```

This declaration supplies one implementation candidate for every concrete `T`
with the required typed property. Its body is type-checked once with rigid `T`
and receives property evidence through the same static evidence mechanism.

For example:

```telora
@fmt.display_by("{host}:{port}")
type Endpoint = struct { host: String, port: Int };

def endpoint: Endpoint = { host: "localhost", port: 8080 };
def text = `endpoint = \{endpoint}`;
```

`Endpoint` publishes `DisplayBy`, the standard blanket implementation supplies
`Endpoint: Display`, and interpolation lowers to `Display.display(endpoint)`.

## Identity and module visibility

`TraitId` consists of the provider `ModuleId` and deterministic local slot.
Module skeleton construction allocates trait slots in canonical source binding
order from `FIRST_DYNAMIC_MODULE_LOCAL`. Imports, aliases and reexports retain
the provider identity.

Trait declarations and implementations participate in the declarative module
skeleton. Their order does not create execution semantics. An implementation
is visible wherever its trait and target are visible and the implementation's
provider module is in the selected module graph.

The first version applies an orphan boundary: an implementation module must own
the trait or the outer nominal target constructor. Built-in primitive
implementations belong to the standard trait provider. This keeps the complete
module graph coherent without global registration order.

## Coherence

The implementation key is `(TraitId, concrete TypeId)`. Exactly one candidate
must apply.

Blanket candidates use a constructor-level pattern and static bounds. The first
version rejects overlapping blanket implementations at module sealing, even
when their current concrete matches happen to be disjoint. An explicit impl
that overlaps a blanket impl is also rejected; there is no specialization.

Repeated imports and reexports retain one implementation identity and do not
create duplicate candidates.

## Dictionary elaboration

Each trait defines a canonical dictionary shape whose fields are the trait
member functions after substituting a target for `Self`. An explicit impl
constructs one immutable dictionary value. A blanket impl constructs evidence
using its rigid type parameters and bound dictionaries.

A surface scheme:

```telora
for(T: Display) Fn(T) -> String
```

has the internal callable ABI:

```text
for(T) Fn(DisplayEvidence(T), T) -> String
```

The hidden evidence argument is inserted at bounded generic call sites and is
not part of source arity, CLI signatures, or diagnostic rendering. Inside the
function, `Display.display(value)` lowers to projecting `display` from that
evidence dictionary and calling the resulting ordinary closure.

For a concrete target, the compiler links the canonical implementation
dictionary directly. VM bytecode, heap publication, copying, equality, fuel and
failure propagation continue to use existing ordinary value and function
semantics. There is no trait-object heap kind and no runtime implementation
search.

## Property publication boundary

The module skeleton records planned type-property evidence from a decorator's
statically determined carrier type. The evidence becomes usable outside the
provider only after the declaration's property batch publishes successfully.
If provider evaluation or atomic publication fails, the enclosing module is not
available and no trait evidence escapes.

Trait membership never depends on inspecting arbitrary property payload data.
The payload is implementation data consumed after static evidence selection.

## Interpolation

Primitive interpolation remains defined directly for String, Int, Float and
Atom runtime categories. For every other statically known type `T`, interpolation
requires `T: fmt.Display` and lowers to the trait member call. Unknown, Any and
Dyn values require an explicit projection or explicit formatting operation.

Failure to resolve `Display` is a type diagnostic at the interpolated
expression. A selected implementation may still produce an ordinary sourced
runtime failure while executing its display logic.

## Diagnostics

Required diagnostics include:

- duplicate trait member;
- invalid use of `Self`;
- missing, extra or duplicate impl member;
- impl member contract mismatch;
- orphan impl;
- overlapping or duplicate impl;
- unknown trait in a bound or impl;
- missing or ambiguous evidence at a generic call or trait-member call;
- property-backed evidence whose property carrier capability is invalid; and
- unsupported interpolation type with the missing `Display` obligation.

Diagnostics point to the authored trait, bound, impl or call location. Runtime
failures from a selected member retain the member implementation as rule source
and the original operand as data source.

## First-version boundaries

The following are outside this RFC:

- trait objects and dynamic dispatch;
- `narrow` and adapter-bearing values;
- associated types or constants;
- default members;
- specialization, negative impls and trait inheritance;
- receiver-method syntax and implicit trait imports;
- local or runtime implementation declarations;
- quote, generated code and decorator codegen; and
- implementation selection based on runtime property payload content.

## Acceptance

- local and cross-module explicit implementations dispatch correctly;
- bounded generic functions receive and forward hidden evidence;
- schemes and reexports preserve canonical trait bounds;
- orphan, completeness, contract and coherence failures are sourced;
- `Property(P)` is proved only by a successfully published type property;
- `DisplayBy` supplies `Display` through the standard blanket implementation;
- nested Display values work through `fmt.display`;
- interpolation of a Display value succeeds without changing primitive
  interpolation behavior; and
- strict and best-effort checking agree on success or failure.
