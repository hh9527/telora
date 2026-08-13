# RFC 0226: Exporting imported local bindings

- Status: Implemented
- Depends on: RFC 0142, RFC 0143, RFC 0144, RFC 0146, RFC 0192, RFC 0218
- Tracking issue: #44

## Summary

Telora defines `import` and `export` as two independent module operations:

```telora
import "@src/model.telora" {Expr, Model as DomainModel};

export {Expr, DomainModel as Model};
```

`import` introduces a local binding into the current module scope. It never
publishes that binding and never changes the current module interface.
`export` is the only operation that publishes an already-visible local binding
in the current module interface. The local binding may have been declared in
the module or introduced by an import.

Exporting an imported local binding forwards its underlying artifacts. It does
not create a new value, type identity, type family, generic scheme, recursive
metadata graph, or provenance root. This RFC adds no `export import`,
`export ... from`, or wildcard export syntax.

There is no distinct re-export binding state or language operation.
Semantically there is one local import followed by one ordinary explicit
export.

## Motivation

The explicit module model already separates lexical binding from publication:

```telora
let implementation = fn(value) { value };
export {implementation as run};
```

RFC 0144 intended the same export-list operation to work for imported
bindings, but the implemented interface construction still assumes that an
exported name has a locally inferred binding descriptor. That assumption is
observable for imported concrete types and parameterized TypeMetadata
families: a facade module can consume them but cannot reliably publish them to
its consumers.

The missing operation is analogous to `pub use`, but Telora does not need a
combined surface form. Its existing operations compose directly:

```text
import = establish a local scoped binding only
export = publish a local scoped binding
```

A facade should therefore be able to define a stable public module boundary
without duplicating type declarations, wrapping generic functions, rebuilding
families, or asking every downstream module to depend on its implementation
modules directly.

## Module Semantics

A source module has a semantic `ModuleInterface` and an initialized persistent
module root. The synthesized runtime record described by RFC 0146 is an
implementation representation of that module root; it is not the definition
of module semantics.

Each public interface entry identifies:

- a public name and export location;
- the scoped binding selected by the export;
- its exact `TypeScheme`;
- its persistent runtime or TypeMetadata root;
- recursive concrete-type descriptor closure needed by that scheme;
- value and rule provenance; and
- the module dependency that owns the original binding.

For a locally declared binding, these artifacts are produced by the current
module. For an imported local binding, they are projected from its resolved
import and included in the current module interface only when an explicit
export marker names that local binding.

## Surface Rules

The grammar is unchanged:

```text
export_items := "{" export_item ("," export_item)* [","] "}"
export_item  := Identifier ["as" Identifier]
```

Export-prefixed binding declarations are syntax sugar for an ordinary local
binding followed by an export marker:

```telora
export def compile = fn(input) { input };

# Core semantic shape:
def compile = fn(input) { input };
export {compile};
```

The same equivalence applies to `export let` and `export type`, including
decorated type declarations. The binding is created, inferred, and evaluated
exactly as a private local binding would be. The export marker creates no
lexical binding and performs no user-code evaluation; it only selects a
visible local binding for the module interface.

An export marker has no local resolution effect. In:

```telora
let a = 1;
export {a as b};
```

`a` remains the only local binding established by these statements. `b` is a
public `ModuleInterface` name available to downstream import and module-member
resolution; it is not resolvable by subsequent expressions in the exporting
module. If that module can resolve a local `b`, it must come from an independent
local binding, and the public alias does not refer to or modify it.

Likewise, the local `compile` in `export def compile = ...` exists because the
desugared `def` establishes it. The desugared export marker contributes only
the public interface entry.

The source identifier must resolve to a local binding at the export statement's
source position. Forward exports remain invalid. `as` changes only the public
interface name:

```telora
import "@src/internal.telora" {compile as implementation};
export {implementation as compile};
```

The local binding remains `implementation`; consumers see `compile`. The
exporting module does not gain a local `compile` binding from this statement.

The following imported bindings may be named by an export list:

- a selectively imported value, concrete type, type family, or generic
  definition;
- the local alias of a selective import;
- a name introduced unambiguously by an open import; and
- a namespace module binding.

Exporting a namespace module binding preserves its nested module interface.
Consumers of the facade must observe the same member schemes and identities as
consumers of the original namespace:

```telora
# facade.telora
import "@src/model.telora" as model;
export {model};

# consumer.telora
import "@src/facade.telora" {model};
type Item = model.Item;
```

This is a module-interface forwarding operation, not publication of an
untyped Dict. Exporting a local namespace binding must preserve the nested
interface and member schemes; silently degrading it to an ordinary record or
rejecting it as an untyped value would contradict the existing semantic Module
category.

## Identity And Evaluation

Exporting an imported binding performs no user-code evaluation. Module
initialization has already resolved and initialized the import dependency
before the facade is published.

For every exported imported binding:

- runtime functions, opaque values, and ordinary immutable values retain their
  existing identity;
- concrete types retain their original structural or nominal identity as
  applicable;
- recursive TypeMetadata retains the original finite graph and back-edges;
- parameterized type families retain the original quantified constructor
  scheme and template closure;
- generic definitions retain the original quantified scheme and instantiate
  independently at each downstream use;
- public aliases do not rewrite internal type names or descriptor identities;
  and
- provenance continues to point to the originating definition and values,
  while the facade export location remains available as boundary context.

A chain of explicit imports and exports forwards the same binding artifacts
through each module. It does not accumulate wrappers or repeatedly copy
recursive graphs.

## Import Forms

Selective and aliased selective imports already project one runtime root and
one exact interface entry into a local binding. A later export marker publishes
that projection.

Open imports may provide an exported local name only when ordinary open-import
resolution selects exactly one provider. Existing ambiguity and local-shadow
rules apply before export publication. Export does not introduce a second
provider-selection algorithm.

Namespace imports retain the target module interface alongside the namespace
root. Exporting that local namespace binding forwards both. Downstream
namespace access must therefore preserve generic instantiation, recursive
concrete-type closure, and semantic navigation for nested members.

Every import form is private to the importing module. No import implicitly
publishes or exports any name. Publication always requires an explicit export
item in the current module.

## Conflicts And Cycles

Existing public-name uniqueness rules are unchanged. Exporting the same local
binding twice under the same public name is a duplicate export. Publishing it
under distinct public aliases is valid.

An export adds no module dependency beyond the import that established its
local binding. The static
module import graph remains the authority for initialization ordering and
cycle rejection. Import/export chains cannot be used to create or legalize a
module initialization cycle.

An unavailable, failed, ambiguous, or unknown imported binding cannot produce
a partial public entry. Strict loading fails at the export reference. Recovery
marks that export unavailable while retaining independent exports and the
original causal chain.

## Static And Tooling Semantics

The authoritative scheme when exporting an imported local is the imported
scheme. The analyzer must not synthesize a monomorphic fallback from an erased
shallow descriptor when an exact imported scheme exists.

The module interface must carry all concrete descriptor closure required to
interpret `Named` references in exported imported schemes, including recursive
struct/enum graphs used directly or through a type-family result.

Definition, references, hover, completion, and workspace queries should
distinguish:

- the facade's public export occurrence;
- the local imported binding occurrence; and
- the originating public definition.

Navigation may pass through the facade, but type and completion results must
be identical to importing the origin directly.

## Non-goals

This RFC does not add:

- `export import`, `export ... from`, or wildcard export syntax;
- implicit export of imported names;
- mutable or live bindings;
- dynamic module paths or runtime-selected exports;
- a new type-only namespace;
- renaming inside recursive TypeMetadata graphs; or
- permission to import an otherwise inaccessible private module.

Visibility is not widened at resolution time. A facade may only export an
imported binding that it could legally establish as a local under the existing
crate and private-module rules.

## Implementation Plan

1. Represent each resolved local import binding with reusable publication
   projection containing its exact scheme, persistent root, concrete-type
   closure, provenance, and optional nested module interface.
2. Resolve export markers to either local publication data or that imported
   projection. Do not route imported schemes through local monomorphic
   fallback inference.
3. Synthesize the facade module root by referencing the imported persistent
   root once, preserving runtime identity and recursive heap links.
4. Forward concrete-type closure and nested module interfaces through strict
   loading, recoverable workspace loading, selective imports, open imports,
   and explicit import/export chains.
5. Extend semantic indexing so facade exports retain both their boundary
   occurrence and origin target.
6. Update `docs/design/LANGUAGE.md` after implementation so it remains the
   language SSOT.

## Acceptance Criteria

1. A selectively imported ordinary value can be exported and imported again
   without reevaluation or identity change.
2. Selective aliases and public aliases compose without changing the local or
   originating binding identity.
3. An exported imported generic definition retains its exact quantified scheme and
   instantiates independently in a downstream module.
4. An exported imported concrete recursive struct/enum type retains exact contracts,
   metadata back-edges, codec behavior, and no `Any` fallback.
5. An exported imported parameterized type family remains applicable downstream,
   including to recursive concrete arguments and through a top-level alias.
6. An explicitly selected open-import binding can be exported; ambiguous
   providers produce the existing deterministic ambiguity diagnostic.
7. An exported local namespace binding preserves its nested interface and member schemes; it
   never degrades to an untyped record.
8. Multi-hop facade chains retain value identity, schemes, concrete-type
   closure, provenance, and dependency identity.
9. Duplicate, unknown, forward, failed, and inaccessible exports have
   source-located diagnostics and publish no partial entry.
10. Strict loading, recovery, `run`, `types`, `show`, completion, hover,
    definition, and references agree on the resulting public surface.
11. The full workspace test suite, formatting checks, and warning-denied
    Clippy pass.

## Rejected Alternatives

### Add `export import`

A combined syntax duplicates operations that already compose and introduces a
second import grammar inside export. It can be reconsidered only if repeated
two-line facades become a demonstrated usability problem.

### Rebuild a local alias

Wrapping functions, reconstructing TypeMetadata, or inferring an ordinary
local `let` loses identity or exact generic and recursive information.
Exporting an imported local is interface forwarding, not source-level
adaptation.

### Treat the module root Dict as the interface

The root representation does not contain sufficient static information for
generic families, recursive named descriptors, nested module members, or
semantic navigation. Module semantics remain explicit even when the runtime
stores public roots in a canonical Dict-shaped object.

## Implementation Result

The existing explicit-export pipeline already resolves export markers against
all visible local bindings, including bindings established by selective,
aliased selective, open, and namespace imports. Recent recursive metadata and
type-family boundary work also preserves the exact imported scheme, persistent
root, concrete descriptor closure, and nested namespace interface through
facade modules. No new runtime or resolver path was required.

This RFC makes that behavior authoritative rather than incidental. End-to-end
regressions now cover imported values, public and local aliases, generic
definitions, parameterized families, recursive concrete types, native opaque
types, open imports, namespace Modules, multi-hop facades, runtime identity,
and the absence of local resolution effects for public aliases.

`docs/design/LANGUAGE.md` now records the core model: import is always local;
only export forms the public Module interface; export markers create no local
bindings; and export-prefixed binding declarations are syntax sugar for an
ordinary local binding followed by an export marker.
