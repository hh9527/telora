# RFC 0228: Stable crate module roots

- Status: Proposed
- Depends on: RFC 0059, RFC 0142
- Tracking issue: #54

## Summary

Telora crates use this layout:

```text
telora-deps.json
src/
  lib.telora
  model.telora
  bin/
    main.telora
tests/
  codec.telora
```

Every file has one stable logical identity independent of whether a Host
selects it as an analysis root:

```text
@src/lib.telora       -> <crate>/src/lib.telora
@bin/main.telora      -> <crate>/src/bin/main.telora
@test/codec.telora    -> <crate>/tests/codec.telora
ontology/lib.telora   -> <dependency ontology>/src/lib.telora
```

`@src` is the importable working-crate source namespace. `@bin` and `@test`
are Host-selectable root namespaces and cannot appear in Telora import
requests. Dependency imports expose only the dependency's reusable `src/`
modules and exclude its `src/bin/` subtree and `tests/` tree.

Crate-oriented CLI commands discover context from the current working
directory by walking upward to the nearest `telora-deps.json`. They accept a
logical root module ID rather than a physical root filename. Selecting
`@src/lib.telora`, `@bin/main.telora`, or `@test/codec.telora` analyzes that
exact stable module as the graph root; no `@main` alias is created.

The previous `bin-src/` layout and `@main` identity are removed without
compatibility behavior.

## Motivation

RFC 0059 gave a Host-selected entry the synthetic identity `@main`. That kept
physical paths out of module identity, but made identity depend on invocation:
the selected file was `@main` while reusable files were named from their crate.
It also required the project-specific sibling directory `bin-src/` and did not
provide a first-class test-entry namespace.

Stable crate-relative root IDs remove both sources of indirection:

- the CLI names the module it wants to analyze;
- the resolver maps that logical ID through one CWD-selected crate context;
- diagnostics, semantic snapshots, caches, and queries retain the same ID;
- `src/bin/` and `tests/` communicate familiar project roles; and
- no second `--bin <physical-file>` parameter is needed to establish a world.

The separation between importability and Host selection remains important.
Application and test entries are graph roots, not reusable library modules.
Giving them stable Host names does not authorize source imports of those
namespaces.

## Crate discovery

Crate-oriented commands start at the process current working directory and
walk its ancestors. The first regular file named `telora-deps.json` selects
the working crate root:

```text
find_crate(CWD) = nearest ancestor containing telora-deps.json
```

Discovery does not inspect the requested module's physical path, search sibling
directories, infer a crate from `src/`, or continue beyond the nearest
manifest. This makes nested crates deterministic: changing CWD explicitly
changes the selected crate.

Failure to find a manifest is an error for commands whose root is a logical
module ID. The diagnostic includes the starting CWD and the required filename.
There is no implicit fallback to an arbitrary ancestor `src/` directory.

The manifest determines dependency aliases, exact path dependency roots, and
format overrides. The working source root is `<crate>/src`; binary roots are
under `<crate>/src/bin`; test roots are under `<crate>/tests`.

Low-level embedding APIs may continue to construct an explicit resolver from
a physical document, but public crate-oriented CLI semantics do not use that
path to reinterpret logical IDs.

## Stable root identities

`ModuleId` gains explicit working-crate root categories:

```text
Source(path)   displays @src/<path>
Binary(path)   displays @bin/<path>
Test(path)     displays @test/<path>
Dependency { name, path } displays <name>/<path>
Builtin(name) displays the canonical built-in name
```

There is no `Main` variant. Selecting a root returns the same structured ID
that the module keeps in the semantic graph.

Paths are normalized relative POSIX-style module paths. Empty paths, absolute
paths, `.`/`..` components, non-UTF-8 components, reserved private roots, and
physical symlink escapes are rejected. Standard exact extension and format
rules remain unchanged.

Mappings are:

```text
@src/<path>  -> <crate>/src/<path>
@bin/<path>  -> <crate>/src/bin/<path>
@test/<path> -> <crate>/tests/<path>
```

`@src/bin/...` is always invalid. The exclusion is a path-component rule, so
`@src/binning.telora` and `@src/binary/model.telora` remain valid.

Root selection may use `@src`, `@bin`, `@test`, a declared dependency module,
or a built-in module when the Host operation supports that module format.
Typical executable operations select `@bin` or `@test`; semantic inspection
may select any resolved module category.

## Import rules

Telora source import requests may produce:

- contextual `@src/<path>` in the requester's owner;
- owner-preserving `./` and `../` requests between reusable source modules;
- declared dependency modules such as `ontology/lib.telora`; and
- canonical built-in modules such as `std/array`.

They may never produce `@bin` or `@test`. Requests beginning with `@bin/` or
`@test/` are invalid from every importer, including another binary or test
root. Dependency request paths entering `bin` as their first source component
are invalid. Dependencies expose no spelling for `tests/`.

Binary and test roots import reusable modules through explicit contextual
requests:

```telora
import "@src/model.telora" {Model};
```

They cannot use `./` or `../`. This keeps their physical nesting out of import
semantics and prevents entry-to-entry composition. Moving a root within
`src/bin/` or `tests/` does not rewrite its library imports.

Ordinary reusable source modules retain relative imports. Normalization must
remain within the owner's reusable source namespace and cannot enter the
reserved `bin` subtree.

Contextual `@src` inside a dependency resolves against that dependency's own
`src/`, never against the consuming crate. It applies the same `bin`
exclusion.

## Root evaluation and semantic workspaces

A Host selects one root ID in the CWD-discovered crate and constructs the
reachable module graph from it. The root does not receive a synthetic name.
For example:

```text
telora check @bin/main.telora
telora check @test/codec.telora
telora show @src/model.telora -k type
```

The first graph root remains `@bin/main.telora`, the second remains
`@test/codec.telora`, and the third remains `@src/model.telora`. These IDs are
used consistently in loader caches, cycle diagnostics, semantic module
records, JSONL queries, provenance, and tooling.

One physical file cannot have multiple valid public IDs:

- `src/bin/main.telora` is addressable only as `@bin/main.telora`;
- `tests/codec.telora` is addressable only as `@test/codec.telora`; and
- `src/model.telora` is addressable only as `@src/model.telora` in the working
  crate.

The same dependency source can still have different owner-qualified IDs in
different consuming resolver snapshots, as established by RFC 0059.

## Manifest and entry options

`telora-deps.json` is mandatory for public logical-root CLI commands, so crate
configuration has one discovery mechanism. Embedded `crate.*` options on a
synthetic `@main` root are removed with `@main`.

If publishable embedded manifest metadata remains required by packaging, it
must be handled as packaging data and must not participate in CWD crate
discovery or create an alternate public resolver mode. This RFC removes the
runtime ambiguity in which one entry could use either a filesystem manifest or
embedded crate options.

## CLI surface

Crate-oriented commands accept stable module IDs:

```text
telora run @bin/main.telora [--input <file|->]
telora check @test/codec.telora
telora show @src/model.telora [-p <substring>] [-k <kinds>] [--exports]
telora show @src/model.telora --at <line>[:<column>]
```

`exec` and `build` likewise select their application root by module ID. LSP
workspace initialization uses the client root/CWD to discover the manifest
and uses logical module identities internally.

This RFC does not add automatic binary or test discovery, a default binary,
`telora test`, assertion syntax, or test-to-test imports. The caller always
names one root.

## Migration

There is no compatibility period. Implementation migrates maintained content
atomically:

- `bin-src/*.telora` application/demo entries move to `src/bin/*.telora`;
- focused validation entries move to `tests/*.telora`;
- physical root arguments become logical `@bin`, `@test`, or `@src` IDs;
- `@main`, `bin-src`, embedded-entry resolver alternatives, and old diagnostics
  are removed from code and maintained documentation;
- examples, test fixtures, scripts, RFC 0227, and current experiment plans are
  updated; and
- historical experiment archives and already implemented RFC documents are
  not rewritten.

The ontology experiment becomes:

```text
ontology/
  telora-deps.json
  src/
    ontology.telora
    bin/main.telora
  tests/ontology.telora

ent-1/
  telora-deps.json
  src/
    logistics.telora
    bin/main.telora
  tests/logistics.telora
```

Validation commands run from the corresponding crate root so CWD selects the
intended manifest.

## Implementation plan

1. Replace `ModuleId::Main` with `Source`, `Binary`, and `Test` working-crate
   identities and update canonical display, equality, semantic projection, and
   cache keys.
2. Add manifest-from-CWD resolver construction and logical root parsing and
   location.
3. Map `@src`, `@bin`, and `@test` to their fixed physical roots with lexical
   and symlink containment checks.
4. Reject `@src/bin`, dependency `bin`, all import requests for `@bin` or
   `@test`, and all relative imports from binary/test roots.
5. Remove `bin-src` detection, `@main`, physical CLI root selection, and the
   embedded-entry resolver branch without aliases.
6. Migrate maintained crate layouts, examples, CLI/core fixtures, language
   documentation, RFC 0227, and active experiment plans.
7. Update `docs/design/LANGUAGE.md` after implementation so it remains the
   module-resolution SSOT.

## Acceptance criteria

1. From any directory below a crate root, the nearest ancestor
   `telora-deps.json` determines the working crate.
2. Logical roots `@src/...`, `@bin/...`, and `@test/...` map exactly to
   `src/...`, `src/bin/...`, and `tests/...` and retain those IDs in semantic
   output.
3. CWD discovery fails deterministically when no manifest exists and never
   guesses a crate from the requested module path.
4. `@src/bin/...`, dependency `bin/...`, `@bin` imports, `@test` imports,
   entry-relative imports, test-to-test imports, and physical/symlink escapes
   are rejected.
5. Working and dependency `@src` requests remain contextual and cannot expose
   a consuming crate to a dependency or vice versa.
6. No `ModuleId::Main`, `@main`, special `bin-src`, or public physical-root CLI
   path remains.
7. Each maintained binary and test root has one stable module identity across
   loading, recovery, diagnostics, semantic queries, and execution.
8. `run`, `check`, `show`, `exec`, `build`, and LSP use the same CWD crate
   discovery and logical root resolver.
9. Maintained examples, fixtures, docs, and ontology experiment plans use the
   new layout and commands; historical archives remain untouched.
10. Resolver, strict loading, recovery, semantic queries, LSP, execution,
    formatting, warning-denied Clippy, and complete workspace tests pass.

## Rejected alternatives

### Keep `--bin <physical-file>`

It requires two coordinates for one query and keeps root establishment outside
the logical module model. Stable root IDs plus CWD crate discovery provide the
same context directly.

### Map every selected root to `@main`

Identity would continue to depend on invocation and semantic records could not
name a binary or test root independently of its selection state.

### Keep `bin-src/`

It preserves implementation history but offers no conventional test-entry
location and keeps application entries detached from the crate source layout.

### Let `@bin` and `@test` be importable

Host roots would become reusable modules, allowing entry-to-entry graphs and
making application/test organization part of ordinary library dependency
semantics.

### Infer crate context from the requested module

That couples identity parsing to physical search, makes nested workspaces
ambiguous, and prevents a command from clearly selecting which dependency map
it intends. CWD plus nearest-manifest lookup is explicit and deterministic.

### Retain standalone physical-path CLI mode

Two public resolver modes would preserve the identity and configuration
ambiguity this RFC removes. Minimal reproductions can create a temporary crate
with a manifest and one `@test` root.
