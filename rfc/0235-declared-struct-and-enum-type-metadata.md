# RFC 0235: Declared Struct and Enum TypeMetadata

- Status: Proposed umbrella
- Tracking issue: #85
- Depends on: RFC 0027, RFC 0028, RFC 0034, RFC 0035, RFC 0051,
  RFC 0055, RFC 0090, RFC 0218, RFC 0232

## Summary

Telora will make named Struct and Enum declarations explicit while retaining
one foundational model: a type is authoritative runtime TypeMetadata, and a
parameterized type declaration is a TypeMetadata family applied through the
ordinary Function ABI.

The target surface is:

```telora
type User = struct {
    id: Int,
    name: String,
};

type Option(Item) = enum {
    'None,
    'Some(Item),
};

type Box(Item) = struct {
    value: Item,
};
```

`struct` and `enum` are declaration-initializer keywords. They are not ordinary
prelude Functions, cannot be captured or passed as values, and replace the
current `@struct` and `@enum` model-constructor decorators. The surrounding
`type` declaration supplies the authored name and a hidden declaration
identity. The completed binding is still an ordinary TypeMetadata value, and
`Box(Int)` is still ordinary application of a TypeMetadata family.

The phase adds declared nominal identity for Struct and Enum roots. It does not
create a second compiler-only type universe, make TypeMetadata syntax-only, or
reinterpret type application as indexing syntax.

This is an umbrella RFC. Five child RFCs establish the surface, concrete
identity, recursive graphs, parameterized families, and ecosystem migration in
independently testable steps. This RFC owns the shared invariants and stopping
rules; each child RFC owns its executable implementation detail.

## Motivation

The current declaration surface is visually named but semantically structural:

```telora
@struct
type Left = {value: Int};

@struct
type Right = {value: Int};
```

`Left` and `Right` normalize to the same Struct descriptor and are assignable.
Their authored roots can also lose their names while traversing recursive
metadata, family substitution, module publication, or tooling projection.

That behavior is internally consistent with RFC 0027, but it no longer matches
the role Struct and Enum declarations play in public model APIs. Recursive
expression algebras, plans, protocol messages, and domain models need stable
declaration identity without abandoning TypeMetadata interpretation, codecs,
schema generation, or user-space descriptor traversal.

The change must therefore solve two problems together:

1. make the declaration intent explicit in source; and
2. carry that declaration identity through the existing authoritative
   TypeMetadata graph.

Solving only the syntax would preserve the semantic mismatch. Solving identity
only in the static checker would lose it at `Any`, `Dyn`, codec, heap, module,
or Host boundaries. Replacing TypeMetadata with compiler-owned nominal nodes
would discard a more important language invariant than the feature is meant to
improve.

## Foundational invariants

Every child RFC must preserve all of the following.

### TypeMetadata remains authoritative

For a declared type `A`:

```text
A : TypeOf(A)
```

The value bound to `A` is canonical TypeMetadata in the authoritative promoted
graph. Static descriptors, HIR facts, `show`, LSP data, codec plans, and schema
plans are projections or consumers of that same graph. They do not become a
second source of type truth.

### Type families remain metadata Functions

For:

```telora
type Box(Item) = struct {value: Item};
```

`Box` retains the RFC 0218 value surface:

```text
Box : for(Item) Fn(TypeOf(Item)) -> TypeOf(Box(Item))
```

Consequently:

```telora
Box(Int)
Option(Box(String))
```

remain ordinary TypeMetadata Function applications. This phase does not add
`Box[Int]`, `Option[Int]`, a separate generic-type evaluator, or a general kind
system. `@[...]` remains explicit application of a generic value scheme, and
`value[index]` remains value indexing.

### Identity is declaration-owned

An authored name is not a type identity. Conceptually, a concrete declaration
is initialized as:

```text
declare type User = @user

descriptor = {
    name: "User",
    kind: 'Fields(...),
}

seal_type!(@user, descriptor)
```

`@user` denotes a private declaration slot or equivalent stable identity. It is
not a Telora Atom, String, address, heap handle, or constructible descriptor
field. `"User"` is display and provenance information only.

The exact internal intrinsic may be named differently. No `type_from_desc!`,
`seal_type!`, raw identity constructor, or uninitialized reference operation is
added to the surface language.

### Aliases do not mint identity

Only a direct declared Struct or Enum initializer creates identity:

```telora
type User = struct {id: Int};
type Alias = User;
export {User as PublicUser};
```

`Alias`, `PublicUser`, and `User` denote the same declaration identity. Alias
bindings and export aliases do not rename, rebrand, clone, or structurally
reconstruct the type. The authored display name remains `User`; tools may add a
qualified access path without changing identity.

### Identity cannot be recovered from shape

Two declarations with equal descriptors remain distinct:

```telora
type Left = struct {value: Int};
type Right = struct {value: Int};
```

Neither ordinary descriptor construction nor validation of an untyped record
against the same field shape may manufacture `Left` or `Right` identity.
Expected-type construction, a declared constructor, or a trusted typed boundary
may create a value of the declared type. Once identity is erased deliberately,
it cannot be recovered merely by comparing structure.

The child RFC responsible for runtime values must define how this invariant
survives `Any`, `Dyn`, validation, codec decode, Work/Main heap movement, module
publication, and Host adaptation. A static-checker-only brand is not an
acceptable implementation.

## Surface model

### Record Struct declarations

The canonical named record form is:

```telora
type Point = struct {
    x: Float,
    y: Float,
};
```

The binding name supplies the authored metadata name. There is no repeated
source string such as `struct "Point"`, and changing the display spelling does
not itself define semantic identity.

Conceptually, this remains continuous with the old decorator call:

```telora
@struct type Point = {x: Float, y: Float};
```

which supplied `{kind: 'Type, name: "Point"}` to the ordinary `struct`
Function. The new keyword makes declaration identity and initialization order
part of the language rather than asking a general Function to infer them from a
decorator context.

### Enum declarations

The canonical named Enum form is:

```telora
type ResultValue = enum {
    'Ok(Int),
    'Err(String),
};
```

Enum declarations describe the same tagged surface used by values and
patterns:

```telora
type OptionValue(Item) = enum {
    'None,
    'Some(Item),
};

let value: OptionValue(Int) = 'Some(1);

match value {
    'None => 0,
    'Some(item) => item,
}
```

In a declaration, a bare tag is a unit variant and a tag with parentheses has
one payload TypeMetadata expression. The first phase does not add variadic
variant payloads. Multiple positional values use one explicit Tuple payload;
named values use one declared Struct payload:

```telora
type Event = enum {
    'Stopped,
    'Moved(Tuple([Int, Int])),
    'Created(User),
};
```

The canonical descriptor may continue to store a deterministic map from tag
name to unit marker or payload metadata. That map is an internal normalized
shape, not the declaration surface. Construction, pattern checking,
assignability, `Dyn`, and codec behavior must retain the owning Enum identity.
Equal variant maps from distinct Enum declarations do not become
interchangeable.

### Parameters belong to `type`

Parameters retain the existing family declaration position:

```telora
type Box(Item) = struct {
    value: Item,
};
```

This avoids conflating three separate concepts:

```text
type Box(Item)       TypeMetadata family parameters
struct { ... }       named-field data shape
struct(...)          reserved positional/newtype shape
```

This umbrella RFC reserves the possibility of positional Struct and newtype
initializers but does not define their grammar, value construction, layout, or
ABI. In particular, it does not consume `Name(...)` after a future `struct`
declaration keyword for both family parameters and positional fields.

### Keywords are not metadata Functions

The final surface contains no ordinary prelude bindings named `struct` or
`enum`:

```telora
let constructor = struct;       # invalid
let metadata = struct(ctx, fs); # invalid
```

The keyword forms are accepted only as direct declared type initializers. The
tool-stage VM may still evaluate field metadata, variant metadata, attributes,
and family templates as ordinary values before the declaration root is sealed.

`Array`, `Dict`, `Tuple`, `Option`, `Result`, `Func`, and user-defined families
remain ordinary metadata constructors. Struct and Enum differ because their
declared roots mint module-owned identity and participate in recursive
reserve/seal components.

## Descriptor and identity model

The implementation must represent a declared root independently from its
structural body. One possible analysis projection is:

```text
DeclaredType {
    id: DeclaredTypeId,
    name: String,
    arguments: [TypeId],
    body: Fields(...) | Variants(...),
}
```

This is illustrative, not a requirement to expose a new public Rust or Telora
data type. The authoritative runtime TypeMetadata graph may encode the root
through a hidden link, an opaque declaration brand, or another canonical
private node, provided that:

1. identity survives graph promotion and copying;
2. identity is stable across repeated analysis of the same module revision;
3. identity is independent of heap addresses and hash iteration;
4. identity includes the canonical module and resolved declaration, not merely
   a source spelling or line number;
5. aliases retain the same identity;
6. separate declarations never collide; and
7. public observers cannot construct or mutate it.

The implementation must not repurpose the existing structural `Named` display
or recursive-reference projection as nominal identity without auditing every
consumer. Recursive references and declared ownership are related but distinct
semantic properties.

## Recursive declarations

Closed concrete recursion is part of this phase, not deferred. A recursive
declaration behaves conceptually as:

```telora
type Node = struct {
    value: Int,
    next: Option(Node),
};
```

```text
declare type Node = @node
evaluate Fields({
    value: Int,
    next: Option(@node),
})
validate the complete component
seal @node
```

Mutually recursive Struct and Enum declarations reserve every declaration in
the strongly connected component before evaluating any body. The component is
validated and sealed atomically. No pending identity, incomplete descriptor,
`Any` approximation, or partially nominal graph may be published.

This protocol extends RFC 0034 and RFC 0035; it does not establish a parallel
recursion mechanism. Existing up-link lifecycle, Work-to-Main promotion,
component failure atomicity, cancellation, quotas, and finite graph traversal
remain authoritative.

## Parameterized declarations

An acyclic parameterized declaration has one declaration head and a
deterministic identity for each application:

```telora
type Box(Item) = struct {value: Item};
```

Conceptually:

```text
Box(Int)    = DeclaredApplication(@Box, [Int], Fields({value: Int}))
Box(String) = DeclaredApplication(@Box, [String], Fields({value: String}))
```

Repeated `Box(Int)` applications are the same declared type. `Box(Int)` and
another declaration `Other(Int)` remain distinct even when their bodies are
equal. Identity and caching must derive from the declaration head plus
canonical TypeMetadata arguments, never from re-executing arbitrary source or
from display strings.

RFC 0218's symbolic-template evaluation remains the basis: a family body is
evaluated once with rigid Bound descriptors, then instantiated through
capture-avoiding substitution. The parameterized child RFC must extend that
model rather than replace it with per-application execution.

Parameterized recursion remains rejected under RFC 0232:

```telora
type List(Item) = struct {
    head: Item,
    tail: Option(List(Item)),
};
```

Supporting this form requires declared application nodes on recursive
back-edges, canonical instantiation graphs, substitution under recursion, and
an ecosystem-wide traversal audit. It may be proposed after this phase, but is
not required to complete it.

## Values and boundaries

Declared TypeMetadata alone is insufficient if ordinary values lose identity
at a dynamic boundary. The phase must define all of these operations:

1. construction of a declared Struct value from a field literal;
2. construction of unit and payload Enum variants;
3. field projection and Enum pattern matching;
4. argument/result checking between distinct declarations;
5. packaging and observation through `Dyn`;
6. explicit erasure to `Any` and the limits of later validation;
7. JSON and text codec decode with a precise declared witness;
8. schema and display interpretation;
9. Work/Main heap copying and Host adaptation; and
10. best-effort failed children without loss or manufacture of identity.

The implementation may erase a brand where a precise witness remains in the
static or packaged boundary, or retain a compact private runtime brand. It may
not accept two declared types by shape after the witness has been lost. The
responsible child RFC must choose one coherent representation and prove it with
cross-boundary tests.

## Copy collection and world movement

Declared identity changes the graph copied between worlds. The current copying
collector preserves ordinary object sharing by reserving a target handle before
recursively copying that object's children. The target protocol makes this
planning boundary explicit and completes it before materialization.

Copy collection proceeds in four phases:

1. **Trace and classify.** Starting from every root, traverse each reachable
   reference once. A reference owned by the source WorkWorld enters the copy
   set. An already-persistent MainWorld reference is validated and retained but
   not traversed as source-owned work. Invalid target-world, foreign-world,
   pending, failed, or unsealed references fail planning.
2. **Build the forwarding plan.** Assign a target handle or stable retained
   edge to every discovered source identity. The complete source-to-target map
   exists before any object payload is copied. Required object, text, shape,
   and quota capacity is known at this boundary.
3. **Materialize and rewrite.** Copy every discovered object shallowly exactly
   once. Rewrite each edge only through the completed forwarding plan or the
   retained MainWorld-edge classification; do not recursively discover new
   objects while materializing.
4. **Validate and commit.** Verify that the planned target graph is
   self-contained, all brands resolve to canonical identities, and no pending
   entry remains. Commit the complete batch atomically; a failure publishes
   none of it.

The trace uses an explicit work queue and identity set rather than the native
call stack. Cycles terminate when their source identity is already present in
the plan, and repeated edges preserve sharing by resolving to the same target
entry.

A declared value brand must be represented as either a compact stable identity
or an edge to a canonical persistent TypeMetadata witness. It must not embed or
clone the complete Struct/Enum descriptor in every value. The chosen
representation must define:

1. how a WorkWorld value refers to a declaration or family-application
   identity;
2. how WorkWorld-to-WorkWorld relocation copies the Work-owned payload while
   preserving already-persistent MainWorld identity edges;
3. how WorkWorld-to-MainWorld publication relocates each Work-owned object once
   and reconnects its brand to the canonical published identity;
4. how a brand that is temporarily Work-owned becomes publishable without
   creating a second semantic type identity;
5. how pending, failed, or unsealed identities are rejected before either
   boundary;
6. how sharing and cycles in the value payload remain governed by the complete
   forwarding plan rather than by type identity; and
7. how legacy Host `Value` projection preserves declared ownership and shared
   subgraphs, or rejects a projection that cannot represent them.

This is a semantic requirement, not only a collector optimization. A unit Enum
is currently representable as an immediate Atom, while a payload Enum and a
Struct are Tagged and Dict objects. Nominal ownership cannot be silently
discarded merely because one representation has no object header. The concrete
identity/value child RFC must choose a uniform branded wrapper, branded object
kinds, or an equivalent witness-preserving representation before nominal Enum
checking becomes authoritative.

Copy-collector planning and legacy `Value` projection are distinct. The former
has a complete handle map before materialization. A legacy tree projection that
tracks only the active recursion stack can still expand a shared DAG repeatedly;
declared identity does not by itself repair that path. A projection capable of
preserving DAG sharing should use the same scan/map/materialize discipline; a
projection whose Host representation cannot express a discovered cycle must
reject it after planning rather than recurse until failure. Performance
acceptance must measure both operations independently.

## Attributes and decorators

Field, variant, and root attributes remain ordinary metadata. A declaration
decorator may transform the draft descriptor before sealing, but it cannot:

- replace the declaration identity;
- change a Struct root into Enum or another metadata kind;
- publish the draft before component sealing; or
- run once per concrete family application.

The migration child RFC must define the exact decorator context for keyword
initializers and update codec-format, schema, text, and other standard
decorators. Removing `@struct` and `@enum` does not remove user-defined metadata
decoration.

## Phase sequence

This phase is planned as five child RFCs.

### RFC 0236: Struct and Enum declaration initializer syntax

Add direct `type Name = struct {...}` and tagged
`type Name = enum {'Unit, 'Payload(T)}` initializer grammar, inferred authored
names, recovered CST/HIR forms, formatting, semantic facts, and focused
migration diagnostics. Initially lower to the existing normalized metadata
shapes so the syntax can be validated independently from nominal assignability.

### RFC 0237: Concrete declared identity and value boundaries

Add non-recursive declaration identity end to end: authoritative TypeMetadata,
static assignability, expected literal construction, runtime values, `Any`,
`Dyn`, codec, Host, heap, module, and import boundaries. Distinct equal-shaped
declarations become observably non-assignable only when this child RFC is
complete.

### RFC 0238: Recursive declared component sealing

Integrate declared identity with the existing concrete recursive TypeMetadata
SCC protocol, graph promotion, recovery, `show`, LSP, descriptor observation,
codec/schema traversal, diagnostics, and performance safeguards. Cover direct
and mutual Struct/Enum recursion without adding parameterized recursion.

### RFC 0239: Acyclic parameterized declared families

Extend RFC 0218 family templates with declared head identity and canonical
argument applications. Preserve `Family(A)` as ordinary TypeMetadata Function
application, module-exported schemes, alias behavior, and the RFC 0232 cycle
rejection.

### RFC 0240: Declared-model migration and legacy removal

Migrate the standard library, core modules, executable fixtures, experiments,
SSOT, tutorials, codecs, schema providers, and user-space interpreters. Remove
the ordinary `struct` and `enum` prelude Functions and the `@struct`/`@enum`
declaration decorators without compatibility aliases. Audit all metadata kinds,
observers, serializers, debug/show renderers, equality/assignability paths, and
resource accounting before declaring the phase complete.

The tracking issue may split a child RFC when implementation evidence reveals
a smaller executable boundary. Combining children is acceptable only when the
result still permits an end-to-end acceptance test and does not hide an
intermediate loss of identity.

## Shared acceptance criteria

The phase is complete only when:

1. `type A = struct {...}` and `type E = enum {...}` are the canonical named
   model declarations;
2. `@struct`, `@enum`, and the callable prelude `struct`/`enum` APIs are absent;
3. every completed declaration is authoritative runtime TypeMetadata;
4. equal-shaped declarations are not assignable across distinct identities;
5. aliases and reexports preserve identity and authored provenance;
6. expected literals, constructors, projection, and matching preserve declared
   ownership;
7. identity survives or is deliberately constrained at every dynamic, codec,
   heap, module, and Host boundary;
8. closed direct and mutual recursive Struct/Enum declarations seal and publish
   as finite canonical graphs;
9. acyclic parameterized declarations retain ordinary family application and
   stable application identity;
10. parameterized recursive declarations remain deterministically rejected;
11. TypeDesc observers expose enough declared/name/reference information to
    interpret the graph without exposing a forgeable identity token;
12. `show`, LSP, diagnostics, codec, schema, and debug output terminate and
    preserve authored names on recursive and parameterized graphs;
13. no failure path publishes a pending identity, incomplete graph, `Any`
    approximation, or structurally recovered nominal value;
14. metadata construction remains deterministic, quota-accounted, cancellable,
    and independent of source order within one dependency graph; and
15. the complete repository and language documentation use the new surface.

## Stopping rules

Implementation stops and returns to design discussion if a child RFC requires:

1. a second authoritative type representation outside runtime TypeMetadata;
2. deriving declaration identity from display names, source line numbers, heap
   addresses, or structural hashes;
3. re-running arbitrary family code for each concrete type application;
4. recovering declared identity from an equal structural value after erasure;
5. exposing pending recursive links or identity constructors to ordinary
   Telora code;
6. approximating an unresolved recursive edge or family application with
   `Any`, `Dyn`, or depth-bounded unfolding;
7. changing `Family(A)` into non-Function application syntax;
8. adding a general kind system, higher-kinded parameters, traits, or nominal
   method dispatch merely to complete declared Struct/Enum identity; or
9. making parameterized recursion a prerequisite for non-parameterized
   recursive declarations.

These conditions indicate that the proposal has crossed the bounded phase, not
that an implementation shortcut is required.

## Deferred work

- positional Struct declarations and their value-constructor syntax;
- dedicated newtype ergonomics and representation transparency;
- unit Struct declarations;
- parameterized recursive Struct and Enum families;
- higher-kinded parameters or partial TypeMetadata family application;
- visibility of individual fields or constructors;
- nominal Union or arbitrary nominal aliases such as `UserId = String`;
- singleton nominal Atom types;
- user-visible comparison, hashing, serialization, or construction of declared
  identity tokens; and
- ABI/layout optimization based on declared representation.

## Rejected umbrella alternatives

### Replace TypeMetadata with compiler-owned nominal types

This would make declaration identity straightforward but break the shared VM,
user-space interpreter, codec, schema, `Dyn`, and tooling model. Declared
identity is additional authoritative metadata, not a replacement type system.

### Keep `@struct` and make the decorator nominal

An ordinary decorator Function has neither a stable declaration slot nor
authority over recursive SCC sealing. Making one callable mint identity would
also permit accidental fresh types from ordinary calls. The keyword boundary
states the authority explicitly.

### Derive identity from `{name, fields}`

Names can be aliased, qualified, reexported, and edited. Structural fields may
be equal intentionally. Neither is a safe nominal identity source, and both are
forgeable ordinary metadata.

### Use `struct Name[T]` and `Name[T]`

This would turn existing TypeMetadata family application into dedicated generic
syntax and collide conceptually with value indexing and `@[...]` scheme
application. Family parameters remain on `type Name(T)` and applications remain
ordinary calls.

### Use `for(T) struct Name`

`for(T)` denotes a universally quantified TypeScheme. A parameterized metadata
declaration defines a TypeMetadata Function/template relation instead. Reusing
the same syntax would hide that distinction and complicate future higher-order
type reasoning.

### Implement parameterized recursion immediately

RFC 0232 records why recursive family application needs a distinct graph and
instantiation model. Concrete recursive declared identity addresses the current
pressure without silently reopening that larger decision.
