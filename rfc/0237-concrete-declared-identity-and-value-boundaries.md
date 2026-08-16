# RFC 0237: Concrete declared identity and value boundaries

- Status: Proposed
- Tracking issue: #85
- Depends on: RFC 0035, RFC 0090, RFC 0157, RFC 0218, RFC 0235, RFC 0236

## Summary

Telora gives every direct, non-parameterized, acyclic `struct` or `enum`
declaration a private declaration identity and carries that identity through
TypeMetadata, ordinary values, dynamic packaging, codecs, module publication,
heap relocation, and the legacy Host value boundary.

```telora
type Left = struct {value: Int};
type Right = struct {value: Int};

let left: Left = {value: 1};
let right: Right = left; # type error
```

The authoritative metadata remains a runtime value. It becomes an immutable
declared TypeMetadata object rather than a forgeable Dict field or a
compiler-only brand. A declared ordinary value is represented uniformly as a
private wrapper containing that metadata witness and its ordinary structural
payload. The wrapper applies equally to Structs, unit Enum variants, and
payload Enum variants.

RFC 0237 is the first go/no-go boundary of RFC 0235. It covers only concrete
acyclic declaration roots. RFC 0238 integrates the same representation with
recursive SCC reservation and sealing. RFC 0239 adds canonical applications of
parameterized declaration families.

## Motivation

RFC 0236 deliberately lowers the new declaration syntax to existing structural
metadata. Consequently these declarations are still interchangeable:

```telora
type UserId = struct {value: Int};
type OrderId = struct {value: Int};
```

Changing only `TypeDescriptor::assignable` would not solve the problem. A
Struct value is currently a Dict, a unit Enum value is an Atom, and a payload
Enum value is Tagged. Their owning declaration is absent after construction.
That identity would be lost through `Any`, `Dyn`, codec decode, WorkWorld
relocation, MainWorld publication, or Host adaptation.

The identity must therefore belong to authoritative runtime TypeMetadata and
must accompany values at every boundary where the precise static witness is
not sufficient.

## Scope

This RFC adds:

1. deterministic private identity for direct concrete declarations;
2. immutable declared Struct and Enum TypeMetadata values;
3. nominal static equality and assignability for completed declarations;
4. uniform runtime ownership wrappers for declared values;
5. expected-literal and expected-variant construction;
6. transparent projection and pattern matching through the wrapper;
7. identity-preserving `Any`, `Dyn`, codec, heap, module, and Host boundaries;
8. non-forgeable TypeDesc observation; and
9. focused cross-boundary and negative tests.

This RFC does not add:

- recursive declared component sealing;
- declared family-application identity;
- parameterized recursive families;
- public constructors or casts for declaration identities;
- shape-based recovery of a lost declaration identity;
- positional Structs or newtypes; or
- removal of the legacy declaration surfaces.

## Declaration identity

The compiler assigns each direct declaration a key conceptually equivalent to:

```text
DeclaredTypeId {
    module: ModuleId,
    local: u32,
}
```

`module` is the already standardized logical module identity, not a physical
path string. `local` is the declaration's deterministic source-order slot among
direct declared initializers in that module revision. The exact Rust layout is
private.

The following are required:

- recompiling the same module revision yields the same key;
- two declaration sites in one module have different keys;
- declarations in different logical modules have different keys;
- aliases, selective imports, qualified imports, and reexports preserve the
  original key; and
- display names and import aliases do not participate in equality.

A source edit creates a new module revision and may reassign local slots. No
identity is promised across distinct source revisions. Independently loaded
copies of the same logical module revision use the same key.

## Authoritative metadata object

A completed concrete declaration is an immutable runtime object conceptually:

```text
DeclaredTypeMetadata {
    id: DeclaredTypeId,
    name: String,
    kind: Struct(fields) | Enum(variants),
    attributes: Attributes,
}
```

This is a TypeMetadata value. It is not an identity token alongside a separate
authoritative Dict. Static descriptors, validation, codecs, schema, Dyn, and
TypeDesc all project from this object.

The object follows the existing `NativeType` precedent: public Telora code can
hold it as TypeMetadata and pass it to ordinary metadata consumers, but cannot
construct it, inspect its private key, mutate its body, or combine its identity
with another body. Legacy Host code may clone the immutable metadata value but
cannot manufacture a new instance with the same key.

Root decorators run on the structural draft before the immutable object is
created. Sealing validates that they preserve the root model kind. Field and
variant decorators continue to run while their draft members are evaluated.

An alias is the same metadata object:

```telora
type User = struct {id: Int};
type Alias = User;
export {User as PublicUser};
```

`User`, `Alias`, and `PublicUser` carry one identity and one authored name.

## Static descriptor

The analysis projection gains a declared node conceptually:

```text
Declared {
    id: DeclaredTypeId,
    name: String,
    body: Struct(fields) | Enum(variants),
}
```

For this RFC the body is acyclic. `Named(String)` remains the temporary
recursive-reference representation owned by RFC 0034/0035 and is not reused as
declared identity.

Assignability rules are:

1. two declared descriptors are mutually assignable exactly when their IDs are
   equal;
2. a declared descriptor is not assignable to or from an anonymous structural
   descriptor merely because the body matches;
3. `Never` remains assignable to every expected type;
4. explicit `Any` retains its existing dynamic checking behavior but does not
   authorize shape-based identity recovery; and
5. aliases and imported projections compare by the preserved ID.

Field projection, Enum exhaustiveness, display, and structural tooling may
inspect the body without weakening root assignability.

## Declared ordinary values

All values owned by a declaration use one private wrapper:

```text
DeclaredValue {
    ty: DeclaredTypeMetadata,
    payload: ordinary value,
}
```

The payload is:

- the ordinary Dict for a Struct;
- the ordinary Atom for a unit Enum variant; or
- the ordinary Tagged value for a payload Enum variant.

The wrapper is semantically transparent to operations authorized by the
declared body's kind:

- Struct field projection reads the wrapped Dict;
- Enum matching reads the wrapped Atom or Tagged value;
- equality first requires equal declared identity and then compares payloads;
- debug/display renders the ordinary source-level value; and
- provenance stays on the authored payload and wrapper boundary.

Generic Dict, Atom, or Tagged operations do not silently discard the wrapper.
An operation that is only defined for an anonymous structural category must
either explicitly preserve the witness or reject a declared value.

## Construction

There is no general declared cast. Construction is expected-type directed.

```telora
type User = struct {id: Int};
let user: User = {id: 1};

type State = enum {'Idle, 'Ready(Int)};
let idle: State = 'Idle;
let ready: State = 'Ready(1);
```

The checker validates the anonymous literal against the declared body and the
compiler emits one ownership operation after the payload is complete. The
operation accepts the exact declared metadata witness and refuses a payload
that does not satisfy its body.

A value already carrying another declared identity is never rebranded by an
expected annotation, function argument, return annotation, `validate`, or
codec API.

Unannotated literals remain anonymous. Consequently this is invalid even when
the fields match:

```telora
let raw = {id: 1};
let user: User = raw; # no shape-based identity manufacture
```

This restriction can be revisited only through an explicit, witness-directed
constructor API with the same validation semantics. RFC 0237 does not add one.

## `Any` and validation

Widening a declared value to `Any` retains the runtime wrapper. A later dynamic
boundary can verify that the wrapper identity equals the expected declaration.
An anonymous value carried as `Any` cannot acquire declared identity by passing
a structural check.

`validate(DeclaredType, value)` therefore has two cases:

- an already declared value with the same identity validates and is returned
  without rewrapping; and
- every anonymous or differently declared value fails with an identity
  mismatch, regardless of structural equality.

Codec decode is different because the exact declared witness authorizes fresh
construction from external data. It validates the decoded structural payload
and wraps it exactly once.

## `Dyn`

`std/dyn.pack` stores both its existing precise descriptor and the declared
runtime wrapper. `std/dyn.desc` returns the declared metadata object. Structural
Dyn observers may inspect the declared body but cannot return an unwrapped
payload that would subsequently be mistaken for an anonymous value.

Projection operations return ordinary child values according to field or
variant payload metadata. They do not propagate the root declaration wrapper
to children unless a child is itself declared.

## Codec and schema

Codec planning unwraps the declared metadata body while retaining the owning
witness in the plan:

```text
DeclaredCodecPlan {
    owner: DeclaredTypeMetadata,
    structural: StructPlan | EnumPlan,
}
```

Decode wraps a successfully decoded root. Encode requires a value with the
same owner identity before reading its payload. Equal-shaped values from
another declaration and anonymous values are rejected.

Schema generation remains structural and emits the authored declaration name
where the target format supports definitions. The private identity never
appears in JSON schema, JSON data, text output, or diagnostics.

## Heap and world boundaries

The runtime heap gains immutable declared metadata and declared value object
kinds, or an equivalent representation with the same edges:

```text
DeclaredTypeObject -> body metadata graph
DeclaredValueObject -> declared type object + payload graph
```

Both participate in the existing forwarding plan:

- WorkWorld-to-WorkWorld relocation copies Work-owned objects once and retains
  existing MainWorld metadata edges;
- WorkWorld-to-MainWorld publication copies the metadata object once, then
  rewrites every declared value wrapper to that canonical published object;
- repeated wrappers preserve one shared owner edge;
- no heap address is used as semantic identity; and
- a foreign, pending, or malformed metadata edge aborts the complete batch.

RFC 0237 covers only acyclic declared metadata, but declared payload values may
still contain ordinary supported sharing. RFC 0238 adds recursive metadata
edges without changing the value wrapper.

## Legacy Host boundary

The legacy `Value` representation gains immutable declared metadata and
declared value variants. Export preserves both identity and payload. Import
reconstructs the same private runtime objects and validates that the metadata
body agrees with its ID-owned object.

Host consumers can compare, clone, log, and pass these values back to Telora.
They cannot construct a declared identity or replace the payload/body through a
public constructor. Existing cycle rejection and DAG-sharing limitations of
legacy projection remain separate concerns; this RFC must not regress them.

## Modules and imports

Module interfaces carry declared descriptors and authoritative metadata values.
Qualification rewrites display paths for diagnostics but never rewrites the
declared ID. Selective imports, namespace imports, aliases, and reexports all
refer to the provider-owned declaration.

Two physical loads resolving to the same canonical `ModuleId` in one Engine
must reuse one declaration identity. Different dependency identities remain
different even if their source text and declaration names are equal.

## Transitional boundary

Until RFC 0240, legacy `@struct` / `@enum` declarations remain structural. Only
the direct RFC 0236 initializer mints declared identity. This prevents an
ordinary decorator call from gaining language-owned authority.

During this branch-only phase:

- direct, non-parameterized, acyclic initializers are nominal under RFC 0237;
- direct recursive initializers retain RFC 0236 behavior until RFC 0238;
- direct parameterized initializers retain RFC 0236 family behavior until RFC
  0239; and
- legacy declarations remain structural until migration.

The umbrella branch is not merged to `main` in this mixed state.

## Diagnostics and observation

Identity mismatch diagnostics show authored and qualified names plus the
relevant declaration and use locations. They do not print private IDs.

`std/type-desc` observes a declared root through stable public facts:

- kind: `Struct` or `Enum`;
- authored name;
- structural children; and
- a boolean declared-root classification if needed.

It cannot extract, serialize, hash, compare, or rebuild the private identity
token. `show`, hover, and debug use the authored name and terminate on the
acyclic body covered here.

## Acceptance criteria

1. two equal-shaped direct concrete Struct declarations are statically
   incompatible;
2. two equal-shaped direct concrete Enum declarations are statically
   incompatible;
3. aliases, imports, and reexports preserve compatibility and authored name;
4. expected Struct literals and Enum variants receive exactly one owner
   wrapper;
5. an anonymous or differently declared value cannot be rebranded by
   annotation or `validate`;
6. field projection, Enum matching, equality, and display preserve ownership;
7. ownership survives `Any`, `Dyn`, callbacks, function arguments/results, and
   captured values;
8. codec decode creates the expected owner, while encode rejects a wrong or
   anonymous owner;
9. schema and TypeDesc inspect the body without exposing a forgeable identity;
10. Work-to-Work relocation, Work-to-Main publication, repeated sessions, and
    module imports preserve one semantic identity;
11. legacy Host export/import round-trips declared metadata and values without
    erasing or manufacturing ownership;
12. invalid and quota-failed construction publishes no partial wrapper;
13. recursive and parameterized direct declarations retain their RFC 0236
    behavior for their later child RFCs; and
14. legacy decorator declarations remain structural until RFC 0240.

## Implementation plan and go/no-go gates

### Gate 1: metadata and static identity

Add the private declaration key, authoritative metadata object, descriptor
projection, module-interface preservation, display, and nominal assignability.
Prove distinct declarations and aliases before changing ordinary values.

If this gate requires public identity tokens or shape-derived identity, stop
the umbrella work.

### Gate 2: owned value construction

Add the uniform runtime wrapper, expected literal/variant ownership, projection,
matching, equality, and strict no-rebranding behavior.

If unit Enum ownership cannot share the same representation and rules as
Struct/payload Enum ownership, stop and revise the representation.

### Gate 3: dynamic and codec boundaries

Audit `Any`, `Dyn`, validation, callbacks, codecs, schema, and TypeDesc. Add a
negative wrong-owner test at every boundary.

### Gate 4: worlds, modules, and Host

Audit heap tracing/copying, Work relocation, Main publication, module imports,
session reuse, and legacy `Value` conversion. Verify shared metadata edges and
atomic failure.

Only after all four gates pass may RFC 0237 be marked Implemented and RFC 0238
begin.

## Rejected alternatives

### Reuse `Named(String)` as identity

`Named` currently represents recursive and imported analysis references and is
renamed during interface qualification. A String is forgeable and conflates
display, recursion, and declaration ownership.

### Put a hidden numeric field in a metadata Dict

Ordinary Dict operations and Host values can clone and recombine fields. A
hidden field would either be observable/forgeable or require pervasive special
cases while still allowing a body to be paired with the wrong key.

### Keep identity only in static descriptors

That loses ownership through `Any`, Dyn, codecs, heap movement, and Host
adaptation, which is the central failure this RFC must prevent.

### Brand only Struct Dicts and payload Tagged values

Unit Enum values are immediate Atoms. Different physical branding schemes
would make Enum identity depend on variant representation and create unequal
dynamic behavior. One wrapper is simpler and uniform.

### Recover identity by validating shape

Two declarations may intentionally have the same shape. Shape validation proves
structural compatibility, not declaration provenance, and cannot decide which
identity to mint.
