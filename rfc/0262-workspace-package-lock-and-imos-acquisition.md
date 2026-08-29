# RFC 0262: Workspace Package Lock and IMOS Acquisition

- Status: Implemented
- Tracking: #144
- Depends on: RFC 0261
- Supersedes: RFC 0164

## Summary

Telora separates crate requirements, workspace source selection, exact binary
locks, package materialization, and module resolution.

Three JSON documents replace `telora-deps.json`:

- `telora-crate.json` gives one crate its canonical name, authoritative module
  catalog, and direct crate dependency names;
- `telora-config.json` establishes the workspace root, selects the one source
  for every crate name, and can apply development-only path overrides;
- `telora-lock.json` records the workspace-wide exact package graph and the
  complete package closure of each locked binary.

The first version supports workspace-contained source paths and immutable
remote tarball URLs. Telora asks IMOS to download, unpack, install, reuse and
garbage-collect remote packages. Package preparation completes before the
existing module resolver discovers a module graph.

## Workspace layout

```text
workspace/
├── telora-config.json
├── telora-lock.json
├── .telora/
│   └── crates-refs/
│       └── <imos-plan-files>
├── app/
│   ├── telora-crate.json
│   └── src/
└── local-model/
    ├── telora-crate.json
    └── src/
```

Commands that operate in crate mode search upward for the nearest
`telora-config.json`. Its directory is the workspace root. A nested config
starts a different workspace; discovery never combines two configs.

`telora-lock.json` and `.telora/` belong to that root. The lock is portable,
deterministic input to a build and is committed. `.telora/` contains local
materialization state and is ignored by source control.

Standalone source and artifact execution remains independent of workspace
discovery.

## Crate manifest

Every workspace and downloaded crate has one `telora-crate.json` at its root:

```json
{
  "name": "app",
  "modules": [
    "@src/domain/user",
    "@src/model",
    "@src/schema.json"
  ],
  "dependencies": ["query", "local-model"]
}
```

`name` follows the canonical crate-name grammar.

`modules` is the authoritative set of modules published by the crate. It is a
JSON array of unique crate-local selectors; authored order has no semantic
meaning. The accepted forms are:

```text
@src/module/path
@src/data.json
@src/data.yaml
@src/data.yml
@src/data.toml
```

Telora source selectors omit `.telora`; static-data selectors retain their
recognized format suffix. `@src` entries follow the ordinary nested module
path rules.

The module catalog maps deterministically to ordinary files under `src/`.
Every listed file must exist and match its declared format. A source or
static-data file absent from `modules` does not participate in resolution,
workspace recovery or packaging. Directory enumeration never adds an implicit
library module. Duplicate selectors, selectors that normalize to the same
canonical module identity, and files that escape the crate root are errors.

`src/bin/*`, `src/entry/*` and `tests/*` are outside the module catalog
contract. Binary, Entry and test targets continue to use their dedicated
Host-selected flat-directory rules; they are neither listed in `modules` nor
diagnosed as undeclared files. Entry modules remain unavailable to ordinary
import resolution.

`telora check` additionally scans the configured crate directories for valid
module files. A file that exists but is absent from `modules` produces a
warning naming the file and the selector that would declare it. The warning
does not add the file to the catalog, change the checked graph, or make the
module importable. Other commands continue to consume only the declared
catalog.

`dependencies` is a set encoded as a JSON array of unique crate names;
authored order has no semantic meaning. It contains no URL, path, version,
digest, feature selection or override. A dependency means that the workspace
must supply exactly one crate with that name, analogous to a workspace-owned
dependency declaration.

The manifest describes a crate rather than a binary. Every binary under that
crate starts with the same declared dependency capability set; its locked
closure can later exclude crates that are not reachable from the selected
module graph.

## Workspace config

A workspace config has this first-version shape:

```json
{
  "version": 1,
  "members": ["app", "local-model"],
  "sources": {
    "query": {
      "tarball": "https://packages.example/query/2026-08-29.tar.gz"
    }
  },
  "overrides": {
    "query": {
      "path": "query-dev"
    }
  }
}
```

All member and override paths are relative to the workspace root. After
canonicalization they must remain within the workspace, name a directory, and
contain `telora-crate.json`. The manifest name must match the name being
selected. Member manifests establish their names, so duplicate member names
are rejected.

Every `sources` key maps one external crate name to one immutable tarball URL.
The first version accepts `http://` and `https://`; the path must identify a
gzip-compressed tar archive. A URL is an exact package locator, not a version
range. Telora performs no registry lookup, semantic-version comparison or
source fallback.

Member names and source names are disjoint. An override key must name an
existing external source and replaces only its effective development root.
The override manifest must have the same crate name and direct dependency-name
set as the locked external package. An override does not rewrite the remote
source or package graph in `telora-lock.json`.

The config must provide a member or source for every direct and transitive
dependency name reached by a selected binary. An absent name is an error with
the dependency path that required it.

## Workspace-wide uniqueness

Within one workspace, a crate name selects one source. Different versions or
different source pointers under the same name are unsupported.

A diamond is valid only when both paths converge on the same selected crate:

```text
app -> a -> common
    -> b -> common
```

Package preparation rejects conflicts before module discovery. Resolver
first-win behavior must not hide a package conflict. Diagnostics report the
crate name and the dependency paths that produced incompatible selections.

This rule preserves canonical module identities of the form
`crate-name/module/path`; package versions and physical store paths do not
enter `ModuleId`, source provenance, type identity or diagnostics.

## Lock document

The lock stores one global package table and binary roots into that table:

```json
{
  "version": 1,
  "packages": {
    "app": {
      "source": { "workspace": "app" },
      "modules": ["@src/domain/user", "@src/model", "@src/schema.json"],
      "dependencies": ["local-model", "query"]
    },
    "local-model": {
      "source": { "workspace": "local-model" },
      "modules": ["@src/model"],
      "dependencies": []
    },
    "query": {
      "source": {
        "tarball": "https://packages.example/query/2026-08-29.tar.gz"
      },
      "modules": ["@src/query"],
      "dependencies": []
    }
  },
  "binaries": {
    "app/t1": {
      "root": "app",
      "packages": ["app", "local-model", "query"]
    }
  }
}
```

Package keys and object keys are emitted in UTF-8 byte order. Dependency and
package arrays are duplicate-free and sorted by canonical crate name; module
arrays are sorted by canonical module identity. The writer emits one canonical
JSON representation and updates the lock atomically. It emits no timestamps,
physical cache roots, temporary paths or Host-specific identifiers.

The lock's remote URL is the immutable package identity in this version. A
server that changes bytes at an existing package URL violates the source
contract. Content digests and sizes may be added as observations without
changing source identity, but this RFC does not require a caller to author
them and defines no update-by-version operation.

Each binary record contains its complete exact crate closure. All binary
records refer to the same global package table, so two binaries cannot lock
the same crate name to different sources. Changing a package source refreshes
every affected binary record in one lock transaction.

## Lock lifecycle

`telora lock` is the only first-version command that creates or rewrites
`telora-lock.json`. It resolves the baseline member and remote sources without
substituting development overrides, materializes remote manifests as needed,
validates their module catalogs and the complete package graph, computes binary
closures, and atomically publishes the new lock.

`run`, `check`, `query` and LSP startup do not rewrite the lock. They require a
present lock consistent with config source selection and crate dependency
names. They automatically ask IMOS to materialize missing packages described
by that lock. A stale or absent lock produces a diagnostic directing the user
to `telora lock`; it does not silently change committed dependency state.

After the baseline graph is validated, development overrides replace effective
physical roots for ordinary workspace commands. Their required name and
direct-dependency equality ensures that the locked package graph remains
valid. An override may have a different module catalog because it is explicit
local development input; module discovery uses that effective catalog without
rewriting the baseline lock.

## Tarball contract

A remote tarball contains exactly one crate root. The archive may place that
root directly at archive root or beneath one common top-level directory. The
crate root contains:

```text
telora-crate.json
src/
```

The first version materializes source crates. A later artifact RFC can permit
`lib.t` in place of `src/` without changing the package graph or locator
model.

Installation rejects absolute paths, parent traversal, escaping links,
duplicate output paths and entries outside the selected crate root. Expanded
byte count, entry count, path length and nesting depth are bounded before the
installed root becomes visible. The installed manifest name and dependency
set must match the locked package node. Its module catalog must match the lock,
and every catalog entry must resolve to exactly one installed file.

## IMOS boundary

Telora converts every locked remote package into a deterministic IMOS Plan.
The Plan key is derived from a domain-separated encoding of the source URL,
archive kind and installation rules. Telora writes each complete plan under
the workspace request home, invokes the configured `imos create` command, and
consumes its single-line immutable installation root.

The workspace request home is:

```text
<workspace>/.telora/crates-refs/
```

Each live remote package has one deterministic IMOS plan file in this
directory. Its presence expresses that the workspace still needs the
installation. Removing a remote package from the lock removes its stale plan
file; IMOS garbage collection determines when unreferenced store content is
reclaimed.

IMOS identifies request intent through hard-linked plan inodes. Therefore
`.telora/crates-refs/` and the configured IMOS store must be on the same local
Unix filesystem. Telora checks this before submitting a request and reports an
actionable configuration error instead of copying or relocating request files
implicitly.

IMOS owns network transfer, concurrent same-key reuse, archive installation,
atomic publication and garbage collection. It does not parse Telora manifests,
choose crate sources, construct dependency graphs or assign module identities.

## Preparation and resolution

For a selected binary, crate-mode execution proceeds in this order:

```text
discover telora-config.json
  -> read members, sources and optional overrides
  -> read and validate telora-lock.json
  -> materialize locked remote tarballs through IMOS
  -> validate installed telora-crate.json files
  -> establish one crate-name -> effective-root map
  -> construct ModuleResolver
  -> discover the selected module graph
  -> run, check or query
```

No network operation occurs inside `ModuleResolver`, module loading,
evaluation or the VM. A resolver observes an immutable crate-root map for its
entire lifetime. The built-in vendor remains first and owns `std`; configured
workspace packages cannot replace or supplement that crate.

`run`, `check`, `query` and LSP workspace recovery use the same package
preparation result. Concurrent commands converge through IMOS rather than
creating distinct physical installations.

## Migration

`telora-deps.json` has no mixed-mode compatibility. A migrated repository:

1. creates `telora-config.json` at its workspace root;
2. creates one `telora-crate.json` in every member crate;
3. moves path selection into config members or overrides;
4. moves remote tarball selection into config sources;
5. generates and commits `telora-lock.json`;
6. ignores `.telora/`.

Commands report the old filename and the three replacement responsibilities
when they encounter `telora-deps.json`; they do not silently translate it.

## Deferred work

- semantic versions, registries and version solving;
- multiple versions or multiple sources under one crate name;
- Git and GitHub-specific locators;
- loose HTTP directory publication;
- authentication, mirrors and source fallback;
- direct remote `lib.t` dependencies;
- `.t` library and binary artifact encoding;
- fat and thin standalone binary generation;
- workspace-external path dependencies;
- platform-specific dependency selection.

## Implementation plan

1. Add strict JSON models and diagnostics for config, crate and lock files.
2. Replace manifest discovery with upward workspace-config discovery.
3. Build and validate the workspace-wide name-to-source catalog.
4. Add deterministic lock graph reading, generation and atomic writing.
5. Add an IMOS client boundary and deterministic tarball Plans under
   `.telora/crates-refs/`.
6. Validate materialized crate roots and pass the resulting root map into the
   existing resolver.
7. Migrate CLI, LSP, fixtures, tests and current repository manifests.
8. Refresh LANGUAGE, CONCEPT and IMPLEMENTATION SSOT documents after the
   executable behavior is complete.

## Acceptance criteria

1. The nearest `telora-config.json` deterministically establishes one
   workspace root.
2. Crate manifests contain a canonical name, an authoritative module catalog,
   and dependency names without physical dependency sources.
3. Duplicate member names, missing sources, source/member collisions and
   dependency cycles produce deterministic graph diagnostics.
4. A valid same-source diamond resolves to one physical and logical crate.
5. One crate name cannot select multiple paths, URLs or installed roots.
6. Workspace path dependencies resolve without network access and cannot
   escape the workspace root.
7. A locked HTTPS tarball is installed through IMOS and reused by a second
   process without a second installation.
8. Archive traversal, escaping links, invalid layouts and manifest-name
   mismatches are rejected before resolver construction.
9. Undeclared source files cannot be imported or selected, and missing catalog
   entries are rejected before graph discovery.
10. `telora check` warns for valid module files absent from the authoritative
    catalog without adding them to the checked graph.
11. `.telora/crates-refs/` accurately retains IMOS intent for every live remote
   lock node and drops stale intent after a lock update.
12. The same lock produces the same canonical crate graph independently of
    physical IMOS store paths.
13. Development overrides affect effective roots without rewriting locked
    remote source identities or dependency edges.
14. `run`, `check`, `query` and LSP consume the same package-preparation
    semantics.
15. Module IDs and source provenance contain canonical crate paths and no
    workspace, download or cache paths.
16. Existing built-in vendor ownership and private-module visibility remain
    unchanged.
17. Formatting, warning-denied Clippy and the complete workspace test suite
    pass after implementation.

## Stopping rules

Implementation returns to design discussion if the first vertical slice
requires a package registry, version comparison, multiple same-name sources,
network access during module loading, an IMOS store mutation outside the IMOS
protocol, or physical cache paths in canonical module identity.
