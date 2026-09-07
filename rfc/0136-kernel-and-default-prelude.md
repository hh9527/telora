# RFC 0136: Kernel and default prelude

- Status: Implemented
- Partial supersession by [RFC 0269](0269-remove-any.md): Any and validate are removed from bootstrap and prelude surfaces.
- Depends on: RFC 0028, RFC 0048, RFC 0114, RFC 0134
- Child RFCs: RFC 0137 through RFC 0139

## Summary

Forma separates the irreducible type kernel from ordinary built-in
capabilities. The kernel is sufficient to parse, type-check, and execute a
system module. The default prelude is then compiled from a real privileged
Forma module and injected lexically into every later module.

The bootstrap order is:

```text
kernel
  -> core/prelude.forma-sys
  -> frozen PreludeArtifact
  -> std and Host native modules
  -> user modules
```

This replaces the current arrangement in which runtime values, static types,
and generic schemes for every prelude name are independently assembled in
Rust.

## Kernel boundary

The kernel contains only values required to express Forma types and the
signature of `core/prelude`:

```text
Type Dyn Any Never
Int Float String Bytes Bool BlameError

Atom Array Dict TypeOf Tagged Tuple Fn
Option Result FoldControl
```

These are language-level Type metadata values and constructors. Several have
precise witness-polymorphic signatures whose declarations would recursively
depend on the constructors being declared. They therefore remain an explicit,
centralized Rust bootstrap boundary.

The following callable capabilities are not kernel operations:

```text
struct enum union validate
```

They are declared by `core/prelude.forma-sys`, backed by registered native
implementations, and exported like members of any other built-in module.

## Default prelude module

`core/prelude` is a resolver-registered RuntimeSystem module with a stable
reserved native module ID. Its source lives at:

```text
crates/forma-core/modules/core/prelude.forma-sys
```

It may be imported explicitly:

```forma
import prelude from "core/prelude";
prelude.struct('None, fields)
```

The runtime also injects the same exported bindings into every non-bootstrap
module's lexical environment. Explicit and implicit access must identify the
same persistent function values and publish the same type schemes.

No module name is privileged by spelling. `core/prelude` is privileged because
the runtime registers it, following RFC 0134.

## Prelude artifact

Successful bootstrap freezes one immutable artifact:

```rust
struct PreludeArtifact {
    values: BTreeMap<String, PersistentValue>,
    interface: ModuleInterface,
}
```

The concrete representation may additionally retain the module root needed by
the layered heap, but it must have one semantic source: the executed module and
its analyzed interface. Runtime evaluation, static checking, semantic queries,
workspace recovery, and explicit imports all project from this artifact.

The artifact is constructed once per Engine configuration. It is not rebuilt
per module or per query.

## Bootstrap isolation

`core/prelude` is analyzed with the kernel only. It cannot see its own eventual
implicit exports, standard modules, Host modules, project dependencies, or an
input value. This makes the dependency graph acyclic and keeps Host extension
from changing the language bootstrap.

All later built-in modules are analyzed with kernel plus the completed default
prelude. User modules observe the same environment.

Failure to build the default prelude is an Engine construction failure, not a
recoverable user-module diagnostic.

## Compatibility

Existing Forma source continues to use unqualified `struct`, `enum`, `union`,
and `validate`. Their implementations and type behavior do not change. This
RFC changes ownership and bootstrap mechanics, not the language surface.

Kernel names remain implicit. This RFC does not create a public module that
allows replacing or shadowing the language type kernel.

## Child RFC sequence

RFC 0137 introduces the explicit kernel/bootstrap inputs and frozen
`PreludeArtifact` without moving public names.

RFC 0138 adds `core/prelude.forma-sys`, registers its native implementations,
and migrates `struct`, `enum`, `union`, and `validate` from direct Rust prelude
injection.

RFC 0139 makes runtime, static, semantic, and explicit-import views project
from the same artifact, adds identity and interface consistency tests, and
removes superseded prelude assembly paths.

## Shared acceptance criteria

1. The kernel boundary is enumerated in one implementation location.
2. `core/prelude.forma-sys` is compiled and executed once per Engine.
3. `struct`, `enum`, `union`, and `validate` are absent from direct kernel
   injection.
4. Existing unqualified uses retain their behavior and inferred types.
5. Explicit `core/prelude` imports expose the same functions and schemes as
   implicit bindings.
6. Standard, Host, user, strict, and recoverable analysis use one completed
   prelude artifact.
7. The prelude cannot depend on itself or on later module layers.
8. Full workspace tests and static checks pass after each child RFC.

## Implementation result

RFCs 0137 through 0139 establish the two-level boundary. Analysis owns a
single bootstrap artifact; `core/prelude.forma-sys` declares the four ordinary
capabilities in a resolver-registered module; and each `MainWorld` installs
that module first and projects its exact exported closures into later
bytecode.

The implementation keeps heap ownership local to `MainWorld` rather than
moving it into `Engine`. Consequently, a prelude is executed once per module
world, not globally once per `Engine`; strict loading and each recovery build
already create distinct worlds. This is the correct lifetime for
`PersistentValue` in the current layered-heap architecture.

Runtime Dict exports and the analyzed module interface must have the same
field set. The precise generic `validate` scheme is audited against the
bootstrap projection. Explicit and implicit access share opaque closure
identity for all four capabilities. Direct parser/type-analysis entry points
without a module world retain the deterministic kernel bootstrap fallback.
