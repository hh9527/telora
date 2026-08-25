# RFC 0261: Canonical crate module space

## Status

Implemented.

## Objective

Module paths use one deterministic crate-owned namespace. File suffixes describe
formats, directory roles describe special identities, and a leading underscore
describes private visibility. Native declaration authority belongs to the
`std` crate rather than to a filename suffix.

## Source layout

```text
telora-deps.json
src/model.telora
src/domain/user.telora
src/_internal.telora
src/bin/main.telora
src/entry/serve.telora
tests/query.telora
```

`src/bin`, `src/entry`, and `tests` contain files only. Nested directories in
these three roots are rejected. Their files do not participate in ordinary
module discovery or import resolution.

A manifest-backed crate declares a stable canonical name:

```json
{"name":"my-crate","dependencies":{}}
```

Directory names never define crate identity. Each dependency-map key declares
the dependency's canonical crate name and follows the same name grammar as the
root manifest.

Resolution queries ordered vendors. The built-in vendor comes first and
currently publishes `std/*`; the configured manifest or standalone-option
vendor follows it. Vendor selection happens at crate granularity: once the
built-in vendor supplies `std`, every `std/*` selector is resolved only within
that crate. A configured crate with the same name is shadowed as a whole rather
than rejected while parsing configuration or allowed to supplement `std`.
The complete crate list is fixed before graph discovery. Registration is
first-win and immutable: the current crate precedes configured dependencies,
and a later dependency declaration cannot redirect an existing crate name.

Standalone mode performs a bootstrap parse of the root's top-level `crate.*`
options before constructing its resolver. Its canonical owner defaults to
`standalone`; `option "crate.name" "my-crate"` can provide an explicit owner,
and `crate.dependency` completes the graph inputs. Module format is always
determined by the explicit recognized file suffix. Formal module identity and
evaluation begin only after this option model is valid.

## Source selectors

Before resolution, a Telora import can use:

```text
@src/module/path
crate-name/module/path
./module/path
../module/path
@bin/name
```

`@bin/name` is accepted only when the importer is the selected Entry. Entry and
test roots are selected by the Host, not imported as ordinary modules.

Telora imports omit `.telora`; writing that suffix is an error. Resolution adds
`.telora` only for physical lookup. Static data imports retain one exact format
suffix: `.json`, `.yaml`, `.yml`, or `.toml`.

Except for one recognized format suffix, a module filename cannot contain `.`.

## Canonical identities

Resolved identities are:

```text
crate-name/module/path
crate-name/bin/name
crate-name/entry/name
crate-name/tests/name
```

The last three identities exist only when the Host selects the corresponding
special module for the current graph. Unselected special files do not appear in
the module catalog.

The canonical identity, not the physical path or source spelling, determines
ModuleId, TypeConstructorId, FuncId, diagnostics, interfaces, and caches.

## Visibility and authority

A source or data module whose filename stem starts with `_` is private. It can
be resolved only by modules with the same canonical crate owner or by the
selected Entry. A leading underscore on a directory has no recursive visibility
meaning in this version.

The selected Entry can resolve every module present in its graph. Its privilege
does not propagate to ordinary modules that it imports. A selected binary Main
can be resolved only by the selected Entry.

Only modules owned by the built-in `std` crate may declare native functions.
There is no `.native.telora` or `.priv.telora` syntax. `core/*` is merged into
`std/*`; public built-ins keep concise extensionless canonical names. Internal
Entry runtime facilities live under a private `std/_...` module.

## Migration

- Rename `.priv.telora` files to `_name.telora`.
- Rename `.native.telora` files to ordinary `.telora` files.
- Move `.entry.telora` files under `src/entry` or the built-in `std/entry` root.
- Rename `core/prelude` to `std/prelude`.
- Remove `.telora` from all Telora import selectors and CLI module selectors.
- Add stable `name` fields to manifests and fixtures.
- Reject old spellings instead of retaining aliases.

## Acceptance

- Resolver tests cover public, private, Main, Entry, test, data, relative, and
  cross-crate resolution.
- Native declarations succeed only for `std`-owned built-ins.
- Catalog output contains canonical names and excludes unselected special roots.
- CLI, query, LSP, diagnostics, and TypeId construction observe the same names.
- `std/_rt.with_diagnostics` powers request-local diagnostics in the standard
  serve Entry without a second permission path outside the resolver.
