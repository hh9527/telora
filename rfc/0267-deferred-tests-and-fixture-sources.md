# RFC 0267: Deferred Tests and Host-Prepared Fixture Sources

- Status: Accepted
- Partial supersession by [RFC 0269](0269-remove-any.md): Discovery boundary fixtures use typed containers or explicit Dyn instead of Any erasure.
- Tracking: [#151](https://github.com/hh9527/telora/issues/151)
- Builds on: RFC 0266
- Partial supersession: RFC 0266's module-evaluation-only `test` verdict and
  shared check/test output contract. Its test catalog and import rules remain.
- Implemented limits: 10,000 expanded Test/failed-fixture nodes, 64 nested
  groups, and a 256 MiB cumulative fixture admission budget. The budget counts
  source bytes, 64 bytes per logical data node, and decoded payload bytes;
  fixture materialization also charges the shared session allocation account.

## Summary

Introduce `std/test.Test`, a standard-library-owned opaque nominal value that
describes deferred tests. `should_ok` and `should_fail` capture thunks;
`with_fixtures` captures source declarations and a factory returning child Tests.
Constructing a Test never invokes its thunk or reads fixture files.

`telora test NAME` first initializes the selected module graph, then executes
the Test values directly exported by the selected root. It reports individual
cases and a summary. `telora check` continues to check and evaluate modules
without executing exported Tests.

Fixtures are Host-provided data sources, analogous to `--source`, rather than
module imports. The Host reads and admits their data before invoking the
corresponding factory. Telora code receives an immutable, sourced `Value`.

## Motivation and Design Baseline

RFC 0266 established a command-local catalog of `tests/**`, one selected root,
and ordinary imports among tests. Its execution contract treats a clean module
evaluation as a successful test. This leaves two gaps:

- a module cannot treat an expected `fail!` as a successful assertion;
- repeated calculations do not have independent case identities or results.

The current private `std/_rt.with_diagnostics` boundary can capture recoverable
failures and consume scoped diagnostics. It is not a public test API, and
ordinary tests cannot import it. The proposed runner can reuse the underlying
continuation machinery without exposing `BlameError` or internal Fail values.

Current design references:

- [Language design](../docs/design/LANGUAGE.md): module initialization, failure
  classes, diagnostic scopes and Host boundaries;
- [Implementation architecture](../docs/design/IMPLEMENTATION.md): Main/Work
  worlds, diagnostic continuations, quotas and source admission;
- [Workspace guide](../guide/WORKSPACE.md): test catalogs and root selection;
- [Execution modes](../guide/EXEC-MODE.md): explicit Host-prepared data sources;
- [RFC 0266](0266-test-command-and-module-catalog.md): the preceding test command.

RFC 0266 remains a historical record. When this proposal is accepted, only its
status metadata should identify the superseded execution and output portions.
Current design documents and guides must describe the implemented contract;
historical RFC bodies must not be rewritten to match it.

## Public Test API

The conceptual public signatures are:

```text
should_ok:
  for(A) Fn(Fn() -> A) -> Test

should_fail:
  for(A) Fn(Fn() -> A) -> Test

should_fail_with:
  for(A) Fn(Fn() -> A, String) -> Test

with_fixtures:
  Fn(Array(String), Fn(Value) -> Test) -> Test
```

`Test` has stable standard-library nominal identity and a hidden representation.
A user record with similar fields is not a Test. The representation must retain
and trace Telora closures through ordinary World copying; it must not retain
untracked handles into a discarded WorkWorld. No new syntax, user-defined macro
mechanism or general-purpose exception API is introduced.

```telora
import "std/test" as test;

def require_positive: Fn(Int) -> Int = fn(value) {
    if value > 0 { value }
    else { fail!("expected positive value", value) }
};

export def accepts_positive = test.should_ok(fn() {
    require_positive(1)
});

export def rejects_negative = test.should_fail_with(fn() {
    require_positive(-1)
}, "expected positive value");
```

These are ordinary qualified functions, distinct from the existing contextual
`should_ok!` intrinsic. They store their arguments and construction location;
they do not invoke the thunk. Successful thunk results are discarded by the
runner and need not be serializable, comparable, or of a uniform static type.

### Success and Expected Failure

`should_ok` passes when its thunk returns normally without error diagnostics.
Returning `'False` or `Result.Err` is still a normal return. Users express
assertions through existing failure APIs; returning a Boolean is not an
implicit assertion.

`should_fail` passes only when its thunk produces a recoverable execution
failure. A normal return fails the case with an expectation diagnostic at the
Test construction location.

`should_fail_with(thunk, expected)` additionally requires the primary failure
message to contain the nonempty literal substring `expected`. This is
case-sensitive substring matching, not a regex or a match against formatted
stacks, warnings, or physical paths. An empty expectation is rejected when the
Test is constructed. A mismatched failure fails the case and reports both the
expectation and the actual failure, preserving its rule and data origins.

An expected failure is consumed within that case's diagnostic scope and must
not remain an unhandled root error in the outer evaluation account. A passing
expected-failure case does not emit its caught error as a command-level error.
Warnings are reported with the case identity, do not satisfy a failure
expectation, and do not themselves fail a case. Host debug observation retains
its existing independent stderr behavior.

The runner executes thunks with strict failure behavior inside the diagnostic
scope. It continues with other independent cases after a recoverable case
failure; it does not expose best-effort Fail children as successful return
values or make broad failure counts the matching criterion.

Fuel, stack, allocation or call-depth exhaustion, cancellation, invalid
bytecode and other terminal failures cannot satisfy `should_fail`. They abort
the invocation. Syntax/type errors and import or module-initialization errors
occur before a runnable Test exists and cannot be caught by it.

## Fixture Groups

```telora
import "std/test" as test;

export def fixture_shape = test.with_fixtures(
    ["fixtures/a.json", "fixtures/b.json"],
    fn(value) {
        test.should_ok(fn() {
            match value {
                'Object(_) => 'True,
                _ => fail!("expected object fixture", value),
            }
        })
    },
);
```

The group stores the ordered source list and factory. Each source supplies one
`Value`; the deferred factory returns a child Test. That child may be either an
expectation or another fixture group. This permits each fixture to choose its
own success/failure expectation through ordinary Telora computation.

The factory is not itself the assertion thunk. If it fails while constructing a
child, that fixture case fails in the `factory` phase. A child `should_fail`
cannot consume a failure that occurred before the child existed.

An empty source list creates a group that fails during discovery with an
explicit "no fixtures" diagnostic. It is not silently treated as a passing
test. Duplicate source declarations are allowed: each array position denotes a
distinct case, even if the locator text is identical.

### Source Resolution and Authority

The first version accepts local file paths with `.json`, `.yaml`, `.yml` or
`.toml` format inference and explicit `file+json://`, `file+yaml://` and
`file+toml://` locators. It reuses the source-format parser and data admission
mechanisms of `--source`; it does not imply support for every `--source`
transport. Stdin, network locators, glob expansion, environment interpolation
and application EES are outside this proposal.

Relative paths resolve against the module containing the actual
`with_fixtures` constructor call. CWD, the selected root module and the thunk's
eventual call site do not rebase them. The constructor records its own authored
source location, not an inherited outer blame boundary. Consequently a helper
which calls `with_fixtures` owns the path base; callers can instead pass a Test
constructed in their own module when they need caller-relative fixtures.

The Test carries a source identity, not a physical base path. The Host maps
that identity to the declaration module's private locator and owning crate.
The initial CLI policy admits only files contained in that declaring crate's
root after normalization and canonicalization. Parent components may reach
sibling directories within the crate; absolute paths and escapes are rejected.
An imported/reexported Test retains its original declaring module and crate.
If the Host has no file base for that source, preparation fails explicitly.

This policy also applies to explicit `file+FORMAT://` locators. Existing test
catalog symlink restrictions remain independently applicable to files under
`tests/`; fixture loading does not bypass those restrictions.

Reading happens only when a selected Test group is executed. Importing a module
that constructs Tests, running `check`, or inspecting exports must not acquire
fixture data. Constructing a Test does not grant its closure a filesystem API.

### Preparation, Admission and Provenance

Before invoking the first factory of a group, the Host prepares all of that
group's immediate fixture sources in declaration order. It reads each distinct
resolved file once for that group and retains the admitted data for its uses.
Duplicate entries still have separate case identities and sourced Values.
An implementation may materialize each Value only when its case starts.

Each source passes existing `DataLimits` before Value materialization. The
runner also enforces a finite aggregate retained-fixture budget, so preparing
many individually valid files cannot create an unbounded Host buffer. Exceeding
the aggregate budget is terminal. Normal file, decoding, syntax or per-source
admission failures produce a failed fixture case; other independently prepared
fixtures still run. No factory is invoked for a failed source.

A nested group prepares its own sources when reached. The promise is that one
group's inputs are fixed before its factories run, not an atomic snapshot of
every file that dynamically produced descendants might later reference.

Fixture values use canonical names of the form:

```text
@test-ctx/<encoded-root-module>/<encoded-export>/<fixture-index-path>
```

Root module and export are separate percent-encoded UTF-8 components; array
indices are zero-based decimal components. For example, the first fixture of
`fixture_shape` has index path `0`, and its second nested fixture has `0/1`.
Non-module fixture sources never receive a `ModuleId`, import edges, exports,
or module-query entries. Their public identity remains distinct from that of
the same file imported as a static data module.

Field-level locations survive data parsing, Value construction, factory calls
and thunk execution. Failure reports can therefore identify both an authored
rule and the fixture subject. Public case output uses declared source labels
and canonical origins; physical Host locators remain private.

Because RFC 0266 enumerates supported files under `tests/`, a fixture file may
incidentally also have a test-catalog entry. Enumeration does not parse its
contents. Using it as a fixture creates no module dependency; only an explicit
import would use the module path. Existing catalog preparation errors are not
converted into expected test failures.

## Root Selection and Execution Lifecycle

The command spelling and test-module catalog remain those of RFC 0266:

```sh
telora test t1
telora -C app test parser/expressions
```

The lifecycle is:

1. Prepare packages, source/test catalogs and the selected root's reachable
   module graph, including its static imports, exports and slots.
2. Check and initialize the graph using existing diagnostic recovery behavior.
   Any initialization error prevents all deferred test execution. Independent
   initialization diagnostics may still be reported.
3. Select direct public exports whose concrete static type is exactly
   `std/test.Test`, verify their runtime nominal witness, and retain their
   closure graphs. Sort these exports by UTF-8 name bytes.
4. Execute each selected Test, expanding fixture groups in array order with
   depth-first execution of child Tests. Capture recoverable failures per case.
5. Emit case results and a final summary; return success only if there is at
   least one executed passing case, no failed cases and no command errors.

Selection does not inspect arbitrary Arrays, Dicts, namespace exports, `Dyn`,
`Any`, or function bodies to find hidden Tests. A similarly shaped record is
not selected. Non-Test exports are allowed as helpers but are not executed by
the runner. A root with no direct Test exports fails with a clear discovery
diagnostic rather than reporting success.

An explicitly reexported Test is selected under its root export name. Two
export names referencing the same Test produce two named executions. Importing
another test module alone does not select its exports. This rule separates
ordinary module initialization from test execution without restricting test
module imports.

The runner observes and drives the Test description using a private standard
adapter or equivalent controlled native boundary. Ordinary callers cannot run
the stored thunk by inspecting the opaque value. A Test's private representation
is not a public ABI, a generic Host plan format, or a new language-level value
outcome category.

### Worlds and Budgets

Module preparation uses its existing account. The entire deferred execution
uses one Host-controlled session quota account, including factory calls,
thunks and ordinary native operations. Starting a new root, fixture or nested
group must not reset fuel, allocation, cancellation or other terminal limits.

The runner creates and discards case-local Work storage with the immutable
initialized module world as background. Dynamically created child Tests and
their captured fixture Values must remain in a live owning World or be copied
through the existing root-driven collector before storage is released. The
runner must not serialize closures or reconstruct their graph through an owned
Host value model. Captured expected failures cannot leak into published Test
descriptions, sibling cases or the final failure verdict.

Expansion of nested groups and emission of case records are bounded Host work,
including finite case-count and nesting limits. Group cycles cannot bypass
these limits. Exhausting these limits aborts the invocation rather than being
caught as an expected program failure. No application reducer, RunHost or EES
service is created; ordinary package acquisition remains a separate Host phase.

## Case Identity and Output

This changes the meaning of `test` substantially enough to introduce
`telora.test/v2`. `check` retains `telora.check/v1` and its existing semantics.

A case is identified structurally by the selected root module, root export
name and an Array of fixture indices. A direct expectation uses an empty index
path. Locator text is a label, not the unique identifier. Group nodes do not
emit successful case records in addition to their children.

```json
{"schema":"telora.test/v2","record":"case","module":"app/tests/t1","test":"fixture_shape","fixtures":[0],"sources":["fixtures/a.json"],"status":"passed"}
{"schema":"telora.test/v2","record":"summary","module":"app/tests/t1","status":"ok","total":1,"passed":1,"failed":0,"aborted":false}
```

Diagnostics retain the current severity, message, labels and notes fields.
Case-associated diagnostics additionally carry `test`, `fixtures`, `sources`
and `phase` (`discovery`, `fixture`, `factory` or `execution`). Module
initialization diagnostics have no case identity. Diagnostics precede the
corresponding `case` record. Case status is `passed` or `failed`.

A fixture preparation or factory failure emits one failed case at that fixture
index; an empty group emits one failed case at the group's index path. Their
counts are included in `total` and `failed`. `should_fail` only interprets a
leaf thunk's execution outcome, never another phase's failure.

Recoverable case failures allow subsequent cases to run. Terminal failures set
`aborted: true`, fail the active case if one exists, and stop expansion; unstarted
cases do not receive fabricated results. Summary counts describe emitted case
records, so `total == passed + failed`. Initialization or no-Test discovery
errors may yield an error summary with zero cases and `aborted: true`.

When evaluation has begun and stdout remains writable, emit exactly one final
summary. CLI argument errors retain clap behavior; package/catalog/root
preparation errors retain stderr `telora.error/v1` without an evaluation
summary. A transport failure need not fabricate a summary it cannot deliver.

Exit 0 requires a successful summary with at least one case. Otherwise exit
nonzero; ordinary test failures use 1. Warnings do not change a passed case or
successful summary. Expected failures consumed by passing cases must not
increment the failure count or appear as unhandled errors.

## Compatibility and Migration

RFC 0266's catalogs, import permissions, canonical module identities, nested
root selection and cycle rejection remain. Its module-only test verdict and
`telora.test/v1` output are superseded when this RFC lands. The historical body
of RFC 0266 stays intact.

Existing initialization checks remain valid with `telora check @test/name`.
To run them as deferred cases, wrap the computation itself:

```telora
# Before: checked during module initialization.
export def check = validate(input);

# After: executed as a named case by telora test.
export def check = test.should_ok(fn() { validate(input) });
```

Wrapping an already evaluated binding does not defer its earlier failures.
Legacy assertions that remain at module scope still participate in module
initialization; an initialization error prevents deferred cases from starting.
They are not silently counted as Tests.

`check` and `query` may inspect Test types and definitions but never invoke
their thunks or acquire fixture sources. The first implementation does not add
suite auto-discovery, parallel scheduling, snapshots, skip/xfail markers,
compiler-error expectations, arbitrary source compilation or test-time IO APIs.

## Alternatives

- Execute `should_fail` immediately during initialization: rejected. It keeps
  test execution coupled to imports and supplies no uniform case lifecycle.
- Treat fixture paths as imports or special compiler syntax: rejected. Host
  source preparation provides data without extending the module graph.
- Export the private diagnostic API as a general catch facility: deferred.
  This proposal needs scoped test outcomes, not a new application recovery API.
- Execute every Test found recursively in arbitrary values: rejected. Direct
  root exports plus explicit fixture composition make selection predictable.
- Infer assertions from returned Bool or Result: rejected. Expectations are
  explicit and ordinary value semantics remain unchanged.

## Implementation Plan

1. Define `std/test`, stable Test nominal identity and constructor bindings.
   Preserve constructor origins and trace captured closures through World copy.
2. Add an internal runner interface that can classify Test nodes and call
   factories/thunks without exposing private diagnostic carriers to user code.
3. Adapt strict diagnostic continuations for expected-failure execution,
   warning forwarding and literal message matching. Audit scope cleanup on
   success, mismatch, recoverable failure and terminal failure.
4. Add fixture source preparation reusing structured locator parsing, sourced
   data admission and materialization. Define explicit aggregate, case-count
   and group-depth limits in runner configuration before implementation review.
5. Separate deferred `test` execution from `check`, implement direct export
   selection and `telora.test/v2`, and preserve all RFC 0266 resolver boundaries.
6. Add execution, source, graph-copy and CLI acceptance coverage. Update current
   design docs, guides and examples when the behavior lands; amend only RFC
   0266's status metadata to mark the superseded portions.

## Executable Acceptance Criteria

1. Test constructors never call their thunk/factory or read sources. `check`,
   query and mere imports do not execute deferred Tests, including when fixture
   files are absent or their contents are malformed.
2. Only direct nominal Test exports are selected. Helpers, namespace exports,
   erased values and lookalike records are not implicitly executed. Reexports
   use the root's public name; a root with no Tests fails discovery.
3. `should_ok` accepts normal returns, including False and Err, and rejects
   recoverable failures. `should_fail` has the inverse execution expectation.
   `should_fail_with` matches only the primary message and reports mismatches.
4. Expected errors are consumed, warnings remain associated with their case,
   and a passing expected-failure case does not poison siblings or the summary.
5. Fuel, allocation, stack and cancellation failures abort even inside
   `should_fail`; new fixtures/groups do not reset the shared account.
6. Fixture cases run in declared order, have stable index identities, and retain
   those identities with duplicate locators or nested groups. Empty groups fail.
7. File paths use the constructor's declaring module and permitted crate root,
   including helper constructors and reexports. Changing CWD does not change
   resolution; escapes and unsupported transports are rejected.
8. JSON, YAML and TOML fixtures retain precise data provenance under
   `@test-ctx/`. They create no module edge, ModuleId or module-query entry.
9. Each group's immediate sources are prepared before factories execute.
   Failed sources do not invoke factories; healthy siblings still run. A
   preparation or factory failure cannot satisfy a child failure expectation.
10. Imported, nested and copied Test closures retain valid Worlds and captured
    Values. Multiple cases and failed scopes do not leave dangling handles or
    leaked Fail roots. Bounded group expansion cannot run indefinitely.
11. Module initialization errors prevent deferred execution and remain visible.
    `check @test/name` retains its old evaluation protocol; production imports
    and test catalogs remain governed by RFC 0266.
12. CLI records have stable ordering and case identities, one summary after
    evaluation, consistent counters and correct exits for success, failures,
    warnings, empty discovery and terminal aborts. No application effects run.

Acceptance coverage is maintained as pure Telora testees and checkers under
`tests/language/src/test/`, with lazy check/query cases in the corresponding
mode directories. `scripts/test-language.sh` runs the new `test` mode without
recompiling Rust. Focused Rust tests cover Host preparation order, duplicate
reads, shared execution quota, and aggregate/expansion limits.
