# RFC 0266: Test Command and Test Module Catalog

- Status: Proposed

## Summary

Add `telora test NAME` to evaluate one test module with the existing best-effort
checking semantics. Before discovering its dependency graph, the Host builds a
command-local catalog of all supported modules under the current crate's
`tests/` directory. Test modules may import other test modules, including files
at the top level. Catalog membership does not select a module for execution.

```text
declared source catalog + command-local test catalog
  -> select tests/NAME.telora
  -> discover imports, exports and static slots of reachable modules
  -> assign static identities
  -> analyze and evaluate the selected dependency graph
  -> diagnostics and one test summary
```

This proposal does not add cyclic module initialization, automatic test-function
discovery, a test wrapper type, or an application effect loop.

## Motivation and Current Baseline

The current CLI has no `test` command. Users select `tests/name.telora` through
`telora check @test/name`. The resolver permits only a single-level test root,
rejects imports using `@test/`, and rejects relative imports from test modules.
Reusable test code consequently cannot form an ordinary module hierarchy under
`tests/`.

The relevant current design is defined by:

- [Language design](../docs/design/LANGUAGE.md), module and evaluation semantics;
- [Implementation architecture](../docs/design/IMPLEMENTATION.md), module graph,
  recovery and publication;
- [Workspace guide](../guide/WORKSPACE.md), source catalogs and test roots;
- [CLI guide](../guide/TELORA-CLI.md), best-effort checking and JSONL output.

The source catalog and module graph serve different purposes. Package
preparation validates declared source paths and formats. It does not parse the
contents of every declared module. `ModuleGraph::discover` subsequently parses
the selected root and its reachable imports, collecting imports, exports and
static slots before module initialization. Unreachable catalog entries are not
parsed or evaluated.

The removal of special `src/bin/` and `src/entry/` identities made those files
ordinary declared source modules. It did not enable cyclic initialization.
Strict loading still rejects import cycles; recovery retains useful facts and
reports cycles without evaluating their cyclic members.

Tests should likewise separate module availability from root selection. A
directory convention for organizing helpers need not prohibit importing another
test module.

## Command and Root Selection

```sh
telora test t1
telora -C examples/app test parser/expressions
```

`NAME` is a normalized path relative to the current crate's `tests/` directory,
without the `.telora` suffix. It selects exactly one Telora root:

```text
t1                  -> tests/t1.telora
parser/expressions  -> tests/parser/expressions.telora
```

The command uses the same workspace discovery, current-crate selection, package
preparation and lock validation as `check`. It does not modify the manifest or
lock. Absolute paths, parent traversal, file suffixes and `MODULE:EXPORT`
selectors are rejected. A missing root is an error. Existing private-module root
restrictions remain in effect.

The first version requires one name. No-argument suite discovery, multiple root
selection, glob filters, parallel test execution and watch mode are deferred.

## Test Catalog and Import Rules

For an explicitly selected test root, the Host recursively enumerates the
current crate's `tests/` directory before graph discovery. The catalog includes
`.telora`, `.json`, `.yaml`, `.yml` and `.toml` modules at every depth. Other file
types are ignored. Static data modules use their existing `data: Value` export
and provenance semantics; they cannot be selected as `test` command roots.

The catalog is an additional resolver input for this command, not an extension
to the published crate manifest. Existing source and dependency catalogs remain
authoritative: this command does not discover undeclared `src/` modules.

Enumeration uses deterministic logical-path ordering and existing filename
validation. The catalog is fixed for the command's lifetime. Files added later
are not admitted; missing or changed reachable modules must follow existing
loader consistency checks rather than silently changing graph membership.
This is not a new atomic filesystem-content snapshot guarantee.

To keep the initial enumeration policy explicit, symlink entries under `tests/`
and a symlinked `tests/` root are rejected. Enumeration must not follow directory
links into cycles, other crates, or the source tree. Supporting aliases through
symlinks can be considered separately with an explicit identity policy.

Test identities use the existing canonical namespace:

```text
tests/t1.telora             -> my-crate/tests/t1
tests/helpers/common.telora -> my-crate/tests/helpers/common
tests/fixtures/input.json   -> my-crate/tests/fixtures/input.json
```

Within the current crate's test modules:

- `@test/path` resolves from the test root directory;
- `./path` and `../path` resolve from the importer's logical directory, remaining
  within the test catalog;
- `my-crate/tests/path` resolves to the same identity as `@test/path`;
- `@src/path` resolves through the owner's declared source catalog;
- standard-library and declared dependency imports keep their existing rules.

Telora selectors omit `.telora`; static data selectors retain their suffixes.
All accepted spellings of a module share one identity and initialization.

```telora
# tests/t1.telora
import "@src/model" as model;
import "./helpers/common" as common;
import "@test/t2" as t2;
import "./fixtures/input.json" { data as input };

export def checks = do {
    let actual = common.normalize(model.decode(input));
    if actual == t2.expected { 'True }
    else { fail!("unexpected normalized value", actual) }
};
```

This example illustrates module composition; no names such as `checks` or
`expected` receive special test semantics.

Every test module, including a top-level file, is importable by another test
module. Source modules cannot import test modules, even when reached from a test
root. Dependencies cannot access the current crate's tests, and the command
does not expose dependency crates' test directories. Access is checked using
the importer identity, not merely the presence of a test catalog.

A self-import or a dependency back to the selected root must resolve to the
same catalog identity and reach ordinary cycle handling. Root selection must
not create a second identity or an exception allowing duplicate initialization.

## Graph Discovery and Evaluation

After catalog construction, the Host selects one root and uses the existing
graph-discovery pipeline. It scans the entire reachable dependency graph before
initialization, including reachable test, declared source, dependency and
built-in modules. It does not parse every test catalog entry merely because the
file was enumerated.

Graph discovery establishes imports, exports and static slots; it does not
provide completed type interfaces or values for cyclic modules. Import cycles
remain errors. Recovery may retain independent facts but must not report the
test as successful when a reachable cycle exists.

Test evaluation reuses `check` semantics. Ordinary module initialization runs
the authored checks; there is no automatic invocation of exported functions,
Boolean assertion convention, or requirement to export `Value`. Existing
explicit module-export requirements remain applicable. A returned `'False`
alone does not fail a test; code must use the existing failure/diagnostic APIs.

Independent computations continue under best-effort evaluation. Any error in
the reachable graph makes the test fail, including an error in an imported test
module. An unreferenced test with a syntax error or failing initializer does not
fail the selected test. Catalog enumeration and path-validation failures remain
preparation errors regardless of reachability.

Diamond imports reuse the same module result within this invocation. Separate
test invocations have separate execution lifetimes. Existing quotas, source
attribution, failure propagation and publication restrictions apply. The command
does not start an application RunHost, reducer or application EES service;
normal package acquisition remains a Host preparation operation.

## Output and Existing Tools

`test` emits JSONL using `telora.test/v1`. Reuse the diagnostic and summary
record fields of `telora.check/v1`, including canonical `module`, diagnostic
locations, summary `status` (`ok` or `error`) and `dependencies`. Emit diagnostics
followed by exactly one summary after evaluation completes. Success exits with
0; a test with errors exits with 1. Warnings alone do not fail a test.

CLI argument errors retain the existing clap behavior. Preparation errors use
the existing stderr `telora.error/v1` protocol and exit nonzero; they do not
fabricate an evaluation summary. Debug observation keeps its existing stderr
protocol. Test result values are not serialized to stdout.

`check @test/name` remains supported and uses the same test catalog and
evaluation behavior, while retaining `telora.check/v1`. Explicit test selection
through `query at`, `query exports`, or a workspace rooted at a test module uses
the same resolver rules, including nested test roots. Ordinary `query modules`
continues to expose the production catalog; this RFC does not add global test
discovery to that command or to LSP startup.

## Alternatives

- Admit only the selected top-level test and subdirectory helpers: rejected.
  This adds a directory-depth restriction unrelated to graph initialization and
  prevents deliberate composition of test modules.
- Require tests in the published manifest: deferred. A command-local catalog
  supports local test code without changing package publication or lock data.
- Resolve arbitrary test paths on demand: rejected for this proposal. A fixed
  catalog makes the permitted module set explicit before graph discovery.
- Parse or execute all catalog entries: rejected. Availability does not imply
  participation in the selected test.
- Add cyclic module initialization or a test wrapper protocol: deferred. Both
  change evaluation contracts beyond the module reuse and CLI scope here.

## Implementation Plan

1. Add Host preparation of a deterministic, command-local test catalog and make
   it available to explicit test-root resolver construction. Keep package and
   source catalogs unchanged.
2. Extend test-root and import resolution with nested paths, importer-scoped
   visibility and canonical identities. Audit selected-root back edges so cycle
   detection sees one module identity.
3. Reuse this resolver in graph discovery, strict loading and recovery. Preserve
   graph/skeleton consistency checks and existing cycle rejection.
4. Add the `test NAME` CLI command, sharing evaluation and diagnostic generation
   with `check` while selecting the test output schema.
5. Add focused resolver, graph/recovery and CLI acceptance coverage. Update the
   language and implementation design documents, workspace/CLI guides and
   relevant examples when implementation lands, not while this RFC is Proposed.

## Executable Acceptance Criteria

The implementation must demonstrate:

1. `test t1` and `test nested/t1` select exactly the requested root in the current
   crate, including a member whose directory differs from the workspace root.
2. Top-level tests import one another; helpers import other helpers and
   top-level tests. Relative, `@test/` and canonical spellings share identities.
3. Unreachable validly named modules with syntax errors or failing initializers
   do not affect the selected test; importing them makes the test fail.
4. Tests can import declared source and dependency modules; undeclared source
   modules, source-to-test imports and cross-crate test imports are rejected.
5. Nested static data fixtures retain canonical source names and field-level
   diagnostic provenance.
6. Self-imports, cycles through the selected root and cycles entirely among
   helpers terminate with diagnostics and fail. They do not hang, duplicate
   module identities or become successful through recovery.
7. Diamond dependencies initialize once per invocation. Permuting catalog
   enumeration order preserves graph identities and observable results.
8. Traversal outside the test catalog, symlink entries, missing roots and invalid
   root selectors are rejected according to the preparation rules.
9. Independent failing exports produce best-effort diagnostics. Any error causes
   exit 1 and an error summary; a clean test exits 0. A plain `'False` export does
   not become an implicit assertion failure.
10. `check @test/name` retains its output schema; explicit test queries resolve
    the same graph. Ordinary source catalogs, lock contents and production
    module-query results do not gain test entries.
11. No application effects run, no new modules are admitted during evaluation,
    and existing quota and publication failures still prevent success.

These are implementation acceptance requirements, not claims of completed
validation for this proposal.
