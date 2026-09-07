# RFC 0275: Construction Checks and Unchecked Values

- Status: Accepted core contracts; implementation in progress.
- Implementation: `std/blame.BlameError` is an opaque native type. The blame,
  raise and warn intrinsics are implemented. Dyn returns preserve erased-value
  origins across call boundaries. Codec and JSON/TOML/YAML now return the same
  opaque BlameError without an error type witness. Unchecked(T) is idempotent and
  uses the high TypeId bit while sharing T's type slot. Runtime construction checks
  cover structs, newtypes, payload variants, first-class constructors, generic
  instances, merge-update and projection. Codec decoding executes nested and variant
  checks through VM continuations; untagged trials require exactly one successful
  branch and propagate checker execution failures. Checked casts enforce nested
  construction checks and unchecked-to-checked conversion. String parsing and its
  codec bridge enforce nested checks and preserve input-string provenance.
  Tool-stage construction prepares checks and their forward value dependencies
  before metadata and binding evaluation; pending checks block construction.
  Generic calls retain resolved parameter context for unchecked conversion.
  The final generic-conversion and recursive-module acceptance audit remains pending.
- Validation: debug build and workspace tests pass; 368 language fixture groups
  pass, including opaque access rejection, intrinsic argument contracts, deferred
  error construction, warnings returning None, cross-module subject origins,
  unchecked identity and fields, generic conversion, Dyn isolation, check signature
  rejection, construction rejection and observable check invocation counts, plus
  codec rejection, nested untagged trials, ambiguity, checker execution failure,
  checked cast rejection, same-type cast invocation counts, parsing checks,
  text-bridge untagged trials, cross-module parsed-field provenance, early and
  source-order tool construction rejection and forward checker dependencies.
  Reexported generic constructors, codec/Dyn boundaries and unchanged schema/wire
  contracts are covered. Direct and untagged recursive checks preserve fuel,
  stack, allocation and call-depth limits in a dedicated quota harness.
- Tracking: [#168](https://github.com/hh9527/telora/issues/168)
- Supersession target: [#143](https://github.com/hh9527/telora/issues/143),
  codec field constraints; see the compatibility analysis below.
- Branch: `feat/0168-construction-checks`
- Depends on: [RFC 0274](0274-unified-constructors-and-checks.md).
- Related: RFC 0237, RFC 0248, RFC 0258, RFC 0259, RFC 0269, RFC 0270, RFC 0271.
- Origin: the construction-check stage of RFC 0274 is moved here at the user's
  request. RFC 0274 was completed under #161; this follow-up is tracked independently.
- Delivery: staged commits and pushes, with Chinese progress comments on #168.
  Do not merge into main automatically. Do not build a release binary.

## Objective

Bind value validation to nominal declarations while keeping members public.
Every successful construction of a checked type must satisfy its declaration's
check. A distinct `Unchecked(T)` supplies the check with the candidate's shape
without already claiming that the check has succeeded.

This RFC specifies construction guarantees. RFC 0274 supplies named newtype and
enum constructors, member name resolution and generic inference independently.

## Scope

Support `@check(func)` on named-field structs, newtype structs and payload-bearing
enum variants. Unit variants are unconditionally valid and reject `@check`.
Whole-enum checks remain deferred. Newtypes retain positional
payload access through `.0`; general named tuples remain outside this scope.

Members remain public. Validation is part of construction, so users do not need
to remember to invoke a separate trait method. Exporting or passing a constructor
as a function must preserve its check.

## Unchecked Representation

`Unchecked(T)` is a distinct builtin type derived from T's representation. It
removes only the outer check guarantee: a field declared as another checked
type still contains a valid value of that field type. It must preserve the
candidate's nominal origin and generic arguments while remaining distinct from T.

Unchecked is idempotent: `Unchecked(Unchecked(T)) = Unchecked(T)`. Runtime TypeId
uses its high bit for the unchecked state and its low 31 bits for T's existing
identity. Shape lookup shares T's type slot; identity comparison and generic
arguments retain the flag. Only successful construction clears it. The flag
does not alter Val provenance. Ordinary type IDs cannot occupy the reserved bit.

For named-field structs, `Unchecked(T)` exposes T's statically checked fields
without claiming its outer invariant. It can be used as a public intermediate
value. A context requiring T implicitly completes construction: invoke T's check
if present, then publish the original candidate as T. Without a check, this
conversion succeeds directly. Static ascription, Dyn and metadata may not bypass
this construction step. No extra runtime wrapper allocation is required merely
to distinguish unchecked and checked identity.

Newtypes and payload variants use their payload type U directly as the check
input; they do not need an Unchecked(T) wrapper. U already satisfies its own
type's guarantees. The outer check decides whether U may constitute the newtype
or selected variant and does not rerun U's checks.

## Construction Protocol

Construction evaluates and type-checks inputs, forms an unchecked candidate,
invokes the declaration-bound check, and publishes T only after success.
Check functions return `Option(BlameError)`: None accepts the original candidate;
Some(error) rejects it. Checks do not transform or replace the candidate.
The exact signatures are:

| Declaration | Check contract |
| --- | --- |
| `type T = struct { ... };` | `Fn(Unchecked(T)) -> Option(BlameError)` |
| `type T = struct(U);` | `Fn(U) -> Option(BlameError)` |
| `type T = enum { A(U) };`, on A | `Fn(U) -> Option(BlameError)` |
| Unit variant | No check; direct construction |

Ordinary construction rejection raises the returned error at the construction
boundary. Codec decoding returns `Result(T, BlameError)` and can try another
untagged alternative without generating a diagnostic for each rejected candidate.

All creation paths must enforce this protocol:

- Callable newtype and enum constructors, including specialized and first-class uses.
- Contextual struct literals, including those within generic calls and callbacks.
- Struct merge-update and projection into a target struct.
- Codec decoding, including nested values and untagged alternatives.
- Construction performed during tool-stage evaluation.

Passing, reading or copying an existing checked value does not repeat its check.
Each merge-update produces a checked result, so a chain checks each intermediate
result. An optimization cannot remove observable failures. One patch containing
several fields validates their combined result once.

References to candidate fields retain their Val source positions. Constructing
the candidate records the construction position for its container. A check
failure must preserve the distinction between input origin, construction site
and the check's own failure site wherever those locations are available.

## BlameError and Provenance

BlameError is an opaque native error value, distinct from an emitted diagnostic:

```telora
blame!(message: String, values: Dyn...) -> BlameError
raise!(error: BlameError) -> Never
warn!(error: BlameError) -> Option(T)  # always None; T comes from context
```

It replaces DecodeError in codec and the JSON/TOML/YAML error interfaces. The
initial public definition lives in `std/blame`, and codec/format modules expose
the same declaration through reexports. AccessError and ResolveError are outside
this replacement unless integration proves a change necessary.

The intrinsic erases subject types without requiring explicit Dyn packing.
Internally, each label is the original Val, retaining its source location and
value identity. Dyn packing, copying an error, and publishing it across heaps
must preserve that origin. The location of the error object or labels array
must not overwrite any individual label. Users cannot access message or labels
as fields; values are traced runtime references within the opaque object.
This works for original semantic Value
inputs and for candidate fields of arbitrary static types.

Creating or returning BlameError does not emit a diagnostic and does not attach
the current rule location. `raise!(error)` emits an error using its message and
ordered labels, adding the actual failure boundary's rule location. `warn!(error)`
emits a warning at its boundary and returns None without terminating execution.
The error value itself has no severity; it can be raised or warned without mutation.
`fail!(message, subjects...)` is equivalent to `raise!(blame!(message, subjects...))`.
Its first argument remains String, not BlameError. Existing must_ok! behavior
on Result(T, String) remains unchanged; Result(T, BlameError) can be explicitly
matched and raised. No implicit multi-error-protocol adaptation is introduced.
Empty labels are valid and produce a rule-only failure. Multiple labels retain
their individual origins; unavailable locations remain unavailable rather than
being replaced with the error allocation site.

Codec structural errors retain the Value at the point decoding failed as an
erased label. Check rejections retain the labels selected by the checker, so
cross-field constraints can identify each relevant field. Untagged trials keep
errors as values until the enclosing boundary chooses to return or raise one.
An explicit fail inside a checker remains an execution failure; normal rejection
uses Some(BlameError). Quota exhaustion is not a candidate mismatch.

## Integration Requirements

1. Define registration and execution phases. Check expressions may reference
   declarations, generic parameters, imported helpers and recursive types. The
   module/metadata scheduler must make the required evidence available before use.
2. Define recursion behavior when checks construct further checked values.
   Reentrancy must neither bypass checks nor create an unbounded host recursion
   path outside normal quota accounting.
3. Define how generic metadata, reflection, publication and codec plans retain
   check functions and their captured values across heaps and module boundaries.
4. Audit all dynamic and metadata operations that could manufacture T, including
   checked casts and Dyn projection. Reading a valid checked value and creating a
   new one must remain distinguishable.

## Implementation Plan

### Relationship to Issue 143

Issue 143 proposes independent decode/encode field constraints that preserve
field types and wire shape. This RFC is intended to supersede that proposal
through construction invariants, rather than implement its original API verbatim.
The issue remains open until this replacement is implemented and accepted.

A struct check can validate its fields without changing their declared types;
a checked newtype can provide a reusable constrained field type with transparent
payload encoding. Both approaches must preserve the successful value and codec
schema shape when only a validation rule is added.

Unlike #143, ordinary language construction also validates candidates. Encoding
an already checked value does not repeat its check. Independent decode-only and
encode-only policies are not implied by construction invariants and are not part
of this replacement. Any continuing need for direction-specific wire policies
must be treated separately before closing #143 as superseded.

Carry forward #143's requirements for checker signature diagnostics, original
field provenance, atomic failure without partial object publication, consistent
shared codec behavior, and stable check-function identity across module/heap
publication and codec plan reuse. BlameError labels identify original fields
for cross-field checks without encoding locations into message strings. Preserve
a structured diagnostic boundary suitable for future documentation references
tracked by #138.

Acceptance must demonstrate unchanged successful values and schema shape,
rejection of invalid decoded and directly constructed candidates, preserved
field provenance, and valid-value encoding without repeated checks. Record the
semantic replacement explicitly when closing #143; do not claim that its
independent directional constraint API was implemented.

### Delivery Sequence

First implement BlameError and its provenance/failure protocol, migrate codec
errors, and add focused language examples. Then implement unchecked identity and observation, followed
by check registration and constructor enforcement. Integrate merge-update,
projection, codecs, tool evaluation and dynamic boundaries before acceptance.

After the complete functional implementation is committed and pushed, compare
performance against a recorded baseline and profile the observed regressions.
Cover frontend/module loading and runtime construction/codec workloads, recording
the causes and measurements before deciding on optimizations.

Use the repository's existing declaration identity, typed-property scheduling,
constructor evidence and Val provenance mechanisms where they satisfy the
contract. Additional metadata must preserve canonical identity through generic
substitution, imports, reexports and module graph cycles.

## Acceptance

- Positive and negative `.telora` cases cover structs, newtypes and payload variants
  with explicit and inferred generic arguments. Unit variants construct directly
  and reject check decorators.
- Exported, imported, reexported and first-class constructors enforce the same check.
- Every creation path listed above is covered, including nested checked fields,
  intermediate merge failures and projection into a checked target.
- Copying and reading checked values do not repeat checks; unchecked-to-checked
  conversions execute the required check at every static, dynamic or metadata boundary.
- Diagnostics preserve input and construction provenance. Codec rejection retains
  the appropriate original values as Dyn labels and supports untagged trial decoding.
- Recursive checks obey normal fuel and resource limits. Cross-heap publication
  preserves captured check functions and nominal identities.
- Guides describe implemented behavior positively. Historical RFCs are not rewritten
  as migration guides. New behavioral tests primarily use `.telora`.
- Debug builds and workspace tests pass. Report evidence in Chinese on #168;
  close the issue only after its full implementation scope is complete.

### Evidence Under Review

| Requirement | Current evidence |
| --- | --- |
| Direct struct, newtype, variant, merge and projection checks | `test/construction-check` |
| Generic constructors across imports and reexports, callback construction | `test/construction-boundaries` |
| Checked/unchecked identity and Dyn separation | `test/unchecked`, `test/construction-boundaries`, TypeStore representation test |
| Nested checked cast conversion | `test/cast-construction-check` |
| Codec trials and explicit checker failure | `test/codec-construction-check` |
| Parsing, text bridge and original string provenance | `test/parse-construction-check`, `test/parse-check-provenance` |
| Metadata and source-order tool construction | `check/diag-check-early-construction`, `check/diag-check-tool-construction`, `check/check-tool-dependencies` |
| No repeated checks during copy, cast, encoding, schema and Dyn roundtrip | `test/construction-check-once` |
| Stable schema and wire output after adding a check | `test/construction-boundaries` |
| Recursive construction resource limits, including untagged trials | `module/tests/construction.rs`, with the program in `fixtures/construction-recursion.telora` |

This evidence does not yet close acceptance. Remaining review includes unchecked
conversion with unresolved generic parameters and recursive module publication,
followed by the requested performance analysis after the functional commit/push.
