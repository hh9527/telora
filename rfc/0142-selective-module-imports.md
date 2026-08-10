# RFC 0142: Selective module imports

- Status: Implemented
- Depends on: RFC 0141

## Summary

Forma can bind selected module exports directly:

```forma
import "std/array" { map, filter as select };
```

Each item names an authored export and an optional local alias. `item` is
shorthand for `item as item`.

## Projection

A selective binding retains both names in the AST. Module loading resolves its
target once and projects three corresponding artifacts:

- the runtime export value;
- its persistent heap root;
- its exported `TypeScheme` from `ModuleInterface`.

The local binding therefore has the same generic behavior and opaque runtime
identity as qualified field access through the module record. It is not
implemented as an inferred alias or a copied native closure.

## Validation

Missing runtime fields or interface exports are import diagnostics at the
authored item. A value/interface mismatch is a module publication error.
Duplicate local aliases and conflicts with other explicit top-level bindings
are rejected under the existing single-assignment rules.

Static data modules currently publish no named `ModuleInterface` exports and
therefore cannot be selectively imported. They remain available through a
module binding.

## Scope

This RFC implements the selector without a simultaneous module binding. The
combined `as module, {...}` form follows in RFC 0143 after selectors and module
bindings can coexist in one parsed import edge.

Selective imports do not re-export their members.

## Acceptance criteria

1. `{ item }` binds one export under its own name.
2. `{ item as local }` binds it under `local` only.
3. Generic functions instantiate normally through selective bindings.
4. Function values retain identity with qualified module access.
5. Missing exports and duplicate local names have item-local diagnostics.
6. Resolver caching loads a target only once for multiple selected items.
7. Strict and recoverable analysis publish the selected schemes and semantic
   import targets.

## Implementation result

Selective syntax lowers each authored item to an import binding that retains
its exported and local identifiers. Strict loading projects the legacy value,
persistent root, and one-entry module interface directly from the cached
target. Recoverable loading performs the same value/interface projection and
keeps the target module identity in semantic import records.

Type analysis installs the projected export scheme as the local import scheme,
preserving generic instantiation. Tests cover aliases, generic validation,
model construction, identity equality with qualified access, lossless CST,
missing exports, duplicate local aliases, and shared target caching.

A later corrective audit found that the legacy shallow definition precheck also
installed the raw imported scheme body as a monomorphic descriptor. Because
that pass does not instantiate schemes, unrelated imported and enclosing
`Bound` identities could collide by their private numeric IDs. Namespace
imports happened to avoid the false rejection through their erased module
record shape. The correction keeps the exact scheme only in the authoritative
scheme table and installs an erased descriptor in shallow binding facts.
Namespace, selective, aliased selective, and open imports now pass the same
higher-order generic wrapper regression and publish the same exact scheme.
