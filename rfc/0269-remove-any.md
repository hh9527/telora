# RFC 0269: Remove Any

- Status: Accepted
- Tracking: [#156](https://github.com/hh9527/telora/issues/156)
- Baseline: `cccb8c3`
- Scope: Remove public Any and its permissive semantics; give affected boundaries explicit contracts.
- Partial supersession: RFC 0268's preservation of explicit Any. Index other affected historical provisions during implementation; historical bodies remain unchanged.
- Implementation: Completed locally and verified; acceptance evidence
  is recorded in the implementation audit below. The design and baseline
  inventory sections retain their drafting context.

## Summary

Remove `Any` from source-visible types, published module interfaces, canonical
runtime types, reflection and accepted type metadata. Remove implicit widening
to Any and operations whose validity depends on erased Any operands.

Use concrete types and parametric polymorphism for ordinary values, declared
enums for closed alternatives, `std/value.Value` for external data, and explicit
witness-carrying `Dyn` for open runtime inspection. Preserve `Never`. Do not
introduce implicit packing into Dyn, a replacement top type, or a universal
codec target.

This is an accepted breaking design, not implemented behavior. Remove Any
directly without a compatibility mode. The implementation checkpoints below
distinguish accepted design decisions from
behavior that must still be verified in code.

## Motivation

Any currently combines several different responsibilities: accepting any input,
erasing a value's static type, admitting dynamic operations, approximating types
during analysis, and bypassing codec restrictions. These responsibilities need
different contracts. A generic input need not lose its relationship with the
output, and an unfinished analysis result must not become a valid runtime type.

Public Union removal already requires an explicit common contract for unrelated
alternatives. Removing Any would make that principle consistent: an unresolved
relationship should be resolved or diagnosed, and an intentional dynamic
boundary should explicitly carry the evidence needed to inspect its value.

Any is not merely dead code. Existing tests intentionally exercise its behavior.
This proposal is a language design choice, not a claim that every explicit use
of a top type is unsound or that all replacements are source compatible.

## Current Implementation Evidence

The following inventory comes from source inspection at the baseline. Examples
in this RFC have not been executed as part of drafting.

| Area | Current evidence | Consequence |
| --- | --- | --- |
| Public type | `types/prelude.rs`, `types/tool.rs`, `core.rs`, `std/type-desc.telora` | Removing only a prelude name leaves other admission paths. |
| Type relationships | `types/relations.rs`, `types/inference-unify.rs` | Any participates in joins, compatibility, unification and projection. Helpers have different roles; their wildcard branches cannot be changed mechanically. |
| Analysis | `types/expression.rs`, `types/graph.rs`, `types/analysis.rs`, `semantic.rs` | Coarse inference, authoritative checking and query recovery all need review. |
| Bootstrap | `types/prelude.rs` | Hidden struct/enum constructors, cast, pack, warnings and validation have Any-bearing signatures or approximations. |
| Dictionary helpers | `modules/std/dict.telora` | keys accepts Any; values/pairs/merge erase result information. |
| Validation | `modules/std/prelude.telora` | `for(A) Fn(TypeOf(A), Any) -> Result(A, BlameError)`. |
| Error values | `blame_error_descriptor` in `types/prelude.rs` | Both data and rule are Any. |
| Schema | `modules/std/json.telora`, `vm/codec-schema.rs` | schema returns a raw graph under Any; merely changing its annotation to Value would be incorrect. |
| Codec properties | `modules/std/_codec.telora` | JsonRenameAll.case is Any although the body accepts only `'CamelCase`. |
| Codec admission | `vm/codec-type.rs` | Any, Bound and Named metadata have paths producing CodecKind::Any. |
| Runtime transform | `vm/codec-transform.rs` | CodecKind::Any returns the existing input without target checking. |
| Reflection and Host | `type_store.rs`, `vm/type-desc.rs`, `vm/dyn.rs`, `vm/public.rs`, `heap/value-builder.rs` | Canonical identity, dynamic witnesses and Host construction also reference Any. |

Existing pure Telora fixtures include `Array(Any)`, fields projected through Any,
`Fn(Any) -> Any`, dynamic calls and negation, erased Test exports, and an untagged
enum payload of Any. `tests/language/src/test/compiler-semantics/testee.telora`
and the query fixture `inference-contracts` explicitly depend on these semantics.

For example, the baseline query fixture contains:

```telora
def array_dynamic: Array(Any) = [1, "x"];
```

Under this proposal this is rejected because Any is not a type. The replacement
depends on intent: a Tuple, an Array of a declared enum, an Array(Value), or an
Array(Dyn) whose elements are explicitly packed. The compiler must not choose
one of these representations on the author's behalf.

## Language Rules

1. There is no built-in Any type, metadata kind or implicit erasure conversion.
2. A generic parameter denotes one consistently instantiated type. It does not
   permit arbitrary operations merely because its instantiation is unknown.
3. Each reachable contributor to a collection, branch, return or shared generic
   argument must satisfy its common contract. Preserve contextual construction,
   explicit `.ty!(Ty)` and `func@[Ty]`, and argument-order independence.
4. Incompatible types are errors. Unsolved types require context where existing
   generalization or empty-collection rules do not settle them. Neither case
   yields Dyn, Value, a synthesized enum or a permissive internal placeholder.
5. Never remains the type of non-returning expressions. It is not a replacement
   for unknown information or an inhabitant-bearing universal type.
6. Field access, indexing, calls, patterns and operators require their existing
   concrete contracts or supported generic obligations. Removing Any must not
   introduce method-name-specific inference rules.
7. `ty!` remains a compile-time proof/context operation. Runtime representation
   validation, codec transformation and Dyn projection remain distinct.

For example, an identity function should have `for(A) Fn(A) -> A`, while a
function ignoring its first argument can quantify that argument independently.
Replacing both positions with Dyn would require callers to pack and project and
would discard a relationship that parametric polymorphism can express directly.

Comparison of different variants under one declared enum remains supported.
Cross-type comparison made possible solely by Any is removed. This RFC adds no
universal equality or dynamic call operation on Dyn.

## Standard Library Contracts

### Dictionary Operations

Accepted signatures:

```telora
native keys: for(A) Fn(Dict(A)) -> Array(String);
native values: for(A) Fn(Dict(A)) -> Array(A);
native pairs: for(A) Fn(Dict(A)) -> Array(Tuple([String, A]));
native merge: for(A) Fn(Dict(A), Dict(A)) -> Dict(A);
```

Retain deterministic key ordering and right-hand precedence for duplicate keys
in merge. The VM already traverses dictionary entries and implements this merge
policy in `vm/dict.rs`; changing static contracts does not justify changing it.

Struct and Dict can share a runtime representation without sharing a static
contract. A heterogeneous struct cannot generally yield Array(A). Callers must
use a Dict with a common value type, construct a known struct explicitly, or use
the existing `dyn.fields` observer after packing a struct. No new implicit
struct-to-Dict coercion is proposed. Existing supported conversions, if any,
must be inventoried and tested rather than inferred from heap representation.

This deliberately does not preserve arbitrary heterogeneous struct merging as
a dynamic operation. A future static record-composition facility can address
that use case separately. Empty Dict inputs still require enough context to
settle A under the ordinary inference rules.

Static struct spread, such as `{...record, k1: 2}`, should be supported in a
future proposal; this RFC preserves existing Dict spread and does not add that
capability as part of Any removal.

### Remove validate

Decision: remove the built-in validate binding, native registration, bootstrap
scheme and standard prelude export directly, without a compatibility alias.
This does not reserve the identifier; user-defined bindings named validate remain
legal. Update the prelude export-consistency check accordingly.

At the baseline, native_validate and native_checked_cast in types/prelude.rs
call the same validate_value_ref and have identical successful-result wrapping.
Their principal difference is BlameError versus String error payloads. There is
no separate validation capability that requires retaining both public routes.

Migrate representation-checking callers to cast!, external data conversion to
codec.decode, and statically provable contexts to ty!. Preserve nominal and
recursive checks; cast does not parse strings, apply codec properties or unpack
Dyn/Value. The current cast! contract is Result(A, String). Changing cast! to
return A or fail is a separate pending decision, not implied by this removal.

### Decoding Returns a Typed Error

Decision: codec.decode and json.decode return Result(A, DecodeError). This
replaces the earlier proposal to return A or immediately produce a diagnostic.
DecodeError contains value: Value, retaining the offending input node and its
source provenance. It needs neither Any nor Dyn. Its shape is:

```telora
type DecodeError = struct {message: String, value: Value};
```

Define and export DecodeError from std/codec alongside Value. json re-exports
the same type identity, rather than declaring a second type. Ordinary Telora
code can construct and propagate it without native privileges. Keep exactly
message and value in the initial contract; paths and expected-type descriptions
are included in message. Do not expose arbitrary rule objects. The native
decode_with functions and their wrappers share this result contract:

```telora
# std/codec
native decode_with: for(A) Fn(Properties, TypeOf(A), Value) -> Result(A, DecodeError);
def decode: for(A) Fn(TypeOf(A), Value) -> Result(A, DecodeError) = fn(target, value) {
    decode_with(properties, target, value)
};

# std/json
native decode_with: for(A) Fn(codec.Properties, TypeOf(A), String) -> Result(A, DecodeError);
def decode: for(A) Fn(TypeOf(A), String) -> Result(A, DecodeError) = fn(target, text) {
    decode_with(codec.properties, target, text)
};
```

The snippets belong to separate modules. A decoding mismatch returns Err without
emitting a diagnostic. Callers can recover, try another decoder, propagate the
error as a value, or explicitly produce a failure:

```telora
match codec.decode(Target, input) {
    'Ok(value) => value,
    'Err(error) => fail!(error.message, error.value),
}
```

At that fail! boundary, derive the data location from error.value using the
ordinary subject-provenance mechanism. Passing the error record itself or only
its message does not establish the offending node's location. No special
implicit conversion of DecodeError to a diagnostic is proposed.

Retain the specific failing child Value, rather than always storing the input
root. For a missing field, retain its parent object and identify the missing
field/path in the error information. Preserve the original source association
through candidate trials and error propagation; rebuilding Value from a raw
graph without provenance is insufficient. Audit the existing codec unwrapping
and transformation paths so they retain the current public Value directly.
Reuse that value rather than recovering it from a path or copying the input.
For generated or Host data without source locations, preserve that absence.

The value field supplies data provenance, not the target or codec property's
rule location. fail! retains its normal rule/call boundary. If decode errors
must additionally preserve a target/property location, define explicit typed
rule information; do not infer it from value or reintroduce arbitrary subjects.

Untagged decoding tries candidates using recoverable results, preserving the
existing success and ambiguity policy. Candidate errors do not emit diagnostics,
even when all candidates fail; the final result is still Err(DecodeError).
Preserve useful failing-node provenance when choosing or combining candidate
errors. This protocol must also be expressible by ordinary Telora decoders;
Rust's internal Result alone is not a substitute for language-level composition.
No separate try_decode entry point or diagnostic capture is needed for trials.

JSON syntax errors have no parsed child Value. Use a Value.String containing
the original parser input and retaining its existing Val location. Keep parser
line/column or offset details in message; do not manufacture a parsed node or
derive a source-file range by adding an offset to a String literal location.
This does not introduce a new runtime text-source registration mechanism.

Keep caller-side Ok/Err handling; migrate error-field access to DecodeError and
use error.value when intentionally producing a diagnostic. Existing unwrap
helpers must not be assumed to accept DecodeError or preserve its provenance.
Successful transformation, nominal identity and recursive codec behavior remain
unchanged. Update shared native result helpers carefully: these
decisions do not automatically change codec.encode, json.parse, YAML/TOML parse,
string.parse, Dyn observers, type-desc.resolve or cast! return contracts.

### JSON Schema and Properties

Change `json.schema` and `schema_with` to return `std/value.Value`. Materialize
the generated raw schema as the existing tagged recursive Value representation;
an annotation change alone leaves an invalid ABI. Callers can then use
`json.stringify(json.schema(Target))` directly. Preserve schema contents,
recursive references and supported enum wire formats.

Using Value avoids adding a second recursive JSON model or prematurely fixing
a closed language API for every JSON Schema keyword. It does not authorize
schema generation for arbitrary non-codec types. Keep existing error behavior
for unsupported targets unless a separately reviewed API change is needed.

Introduce a concrete case enum with the currently supported `'CamelCase`
variant for JsonRenameAll.case and rename_all. Expose it through the same codec
and JSON API boundaries as the existing property types. Expected enum context
should keep `@codec.rename_all('CamelCase)` usable; invalid cases become static
errors when known. Runtime property admission must still validate Host input.
Adding new naming conventions is outside scope.

## BlameError and Diagnostic Subjects

BlameError is a central design dependency, not incidental cleanup. Currently
codec failures carry raw data and rule values; Dyn observer errors carry the
input Dyn and a String operation name; type-desc resolution errors carry a type
descriptor value and a String. A uniform replacement with Value cannot express
all of these, including functions, opaque values and recursive runtime graphs.

The earlier recommendation to replace data/rule with Dyn is withdrawn. Runtime
diagnostic subjects need not become ordinary language values and therefore do
not inherently require canonical Dyn witnesses. Keep their raw representation
and provenance internal; expose a defined diagnostic snapshot at observation
boundaries. The baseline hides the BlameError type binding from ordinary modules
but permits inferred native error values and ABI field access. It is not an
ordinary public source type despite its structural internal descriptor.

Opening the existing _rt diagnostic scope to ordinary modules is explicitly
deferred to the next design iteration. A new public capture or re-propagation
API is not a prerequisite for deleting validate or changing decode results.
Preserve the existing access restriction, nested-scope ownership, warning order,
consumption without duplicate Host reporting, and terminal resource failures.

For complete Any removal, use the concrete privileged Diagnostic snapshot below.
At the baseline diagnostic-scope.rs re-encodes general diagnostics but directly
returns a supplied raised value. Normalize both paths to that snapshot without
changing access rights. A public capture or re-propagation interface remains
outside this iteration.

Removing validate and changing the two decode APIs eliminates their returned
BlameError values. The producer table below specifies the remaining replacements.
Result remains valid for ordinary domain alternatives; this RFC does not make
every Err a diagnostic failure.

## Concrete Error and Provenance Plan

The following accepted choices complete the error-contract design. They require
implementation and tests; they do not retain Any or introduce an untyped substitute.

### Remaining Native Error APIs

| Producer | Result/error contract | Error subject |
| --- | --- | --- |
| codec.decode/decode_with | Result(A, codec.DecodeError) | Original failing Value, or parent for a missing member. |
| json.decode/decode_with | Result(A, codec.DecodeError) | Current parsed Value; original text wrapped as Value.String with its existing location for syntax failure. |
| json.parse, yaml.parse, toml.parse and parse_raw | Result(Value, codec.DecodeError) | Original parser input wrapped as Value.String with its existing location; parser position details in message. |
| codec.encode/encode_with | Value; failure produces a runtime diagnostic | Existing failing Val and codec rule locations, with no public error value. |
| string.parse/parse_with | Result(A, string.ParseError) | Original String input. |
| dyn.field/fields/array_items/tuple_items/tag/payload | Result(existing success type, dyn.AccessError) | Existing Dyn input, reused without double packing. |
| type-desc.resolve | Result(Type, type-desc.ResolveError) | Original Type input. |
| _rt.call_with_diagnostics/with_diagnostics | Existing Result/tuple shape with Array(_rt.Diagnostic) | Encoded labels and messages, no raw subject field. |
| validate | Removed | Migrate to cast!, ty! or decoding as appropriate. |

Define ParseError, AccessError and ResolveError locally in their respective
modules with message: String and value: String, Dyn and Type respectively.
Export these types from their owning module. No dependency on std/codec is
needed for Dyn or type reflection. These value fields serve
as diagnostic subjects to recover original positions; callers need not project
or inspect the original business values. Existing Dyn operations remain available,
but recovery of business values is not the purpose of this error contract.

Encoding returns Value directly. There is no EncodeError and no Dyn packaging
or additional input type witness needed for error reporting. The encoder retains
the failing Val internally and reports its existing location together with the
codec rule location. Nested paths remain in the diagnostic message where available.
Other returned error values can be reported with fail!(error.message, error.value).

Telora Float values are finite: literal parsing, arithmetic and supported Host/native
construction reject non-finite values before encoding. Defensive non-finite checks
in the encoder do not justify a public error type. Unsupported input types and
invalid codec configurations currently produce diagnostics; moving their detection
to static codec-capability validation is a separate improvement, not a prerequisite
for this ABI change. Internal representation failures also remain runtime failures.
Custom Display failures and terminal resource failures propagate normally.

codec.format_error becomes a generic function over an argument with a supported
message-field obligation, returning that String. Verify its published scheme
uses the existing field-obligation machinery; it must not use Any. If that
scheme cannot be exported by the existing language, remove this trivial helper
and migrate callers to error.message rather than expanding type-system scope.

Keep cast!'s Result(A, String), Option-based Dyn projection, and existing domain
Result values unchanged. Keep unsupported Host metadata and resource failures
on their existing runtime rejection paths; ordinary error contracts do not
turn invalid bytecode or exhaustion into recoverable data errors.

### Current Value and Its Location

Source inspection confirms heap/semantic.rs preserves locations while wrapping
and unwrapping Value. However, codec-entry.rs decodes an allocated raw graph,
and CodecFailure currently carries a raw Val, not the original Value node.
DecodeError must instead retain the Value currently being decoded. Its runtime
Val already contains the location used by Telora's normal diagnostic mechanism.
No separate error location field, structured input cursor, path lookup table or
new provenance system is required.

Pass the current Value through recursive decoding. When visiting a child, use
that child Value; when it fails, store it directly in DecodeError.value. A raw
view can still be used internally, but must not discard the current Value and
later try to reconstruct its identity by scalar equality, location or a path.
Paths in message explain the failure; they do not locate the error subject.

Missing members use the current parent object. Temporal variants use the current
semantic Value even when their raw view is a dictionary. Synthetic defaults use
their existing value provenance where available; an input-level missing/default
error uses the parent input and describes the default issue in message. Preserve
existing cycle guards, sharing and allocation accounting without copying a Value
tree per candidate. fail!(error.message, error.value) reads the retained Val's
location using the existing subject handling.

codec-enum.rs currently collects candidate error messages and uses the whole
candidate input on total failure. Preserve this deterministic aggregation policy:
zero matches returns the Value at the untagged choice point, with candidate
paths/reasons in message; multiple matches returns that same Value with an
ambiguity message. An ordinary nested mismatch retains the specific child.
This is an explicit aggregate-error exception to the failing-child rule, not
an arbitrary selection of whichever candidate happened to fail last.

### Existing Provenance Mechanism

Position handling uses the locations already carried by Val and Telora's
existing source/Host facilities. Verify that retaining and returning the current
Value preserves those locations; do not make a new source registry, ownership
protocol or path-to-value map a prerequisite for DecodeError.

For text syntax errors, preserve the input String's existing location when
wrapping it as Value.String. Include the parser's available position details in
message instead of dropping them as vm/json.rs currently does when extracting
only the error message. An offset within decoded text is not a source-literal
offset, especially for escapes or generated strings. Preserve absent locations
as absent. A later investigation may address independent source rendering or
lifetime defects if reproduced; this RFC does not assume or mandate a redesign
of that infrastructure.

### Privileged Diagnostic ABI

Define the following concrete types in std/_rt, keeping current entry-only
access checks. These fields are snapshots, not handles to runtime subjects:

```telora
type Severity = enum {'Error, 'Warning, 'Info};
type SourceRange = struct {
    source: String,
    start: Int,
    end: Int,
};
type Label = struct {
    location: SourceRange,
    message: String,
    primary: Bool,
};
type Diagnostic = struct {
    severity: Severity,
    message: String,
    labels: Array(Label),
    notes: Array(String),
};
```

SourceRange uses UTF-8 byte offsets, end-exclusive, and the registered source's
display name. It is descriptive and grants no file access or source mutation;
Host rendering uses owned source identities, never trusts this display name as
a unique lookup key. Runtime frames can be summarized in notes in stable order;
adding a public structured stack API is deferred. A diagnostic with no location
has an empty labels array, not a fabricated location. Preserve all available
labels and notes from source::Diagnostic. Export helper types with Diagnostic
inside the already restricted module.

Remove the current raised-value pass-through in diagnostic-scope.rs. Extract
the diagnostic record using the same conversion as Host reporting, then encode
the snapshot. This is a deliberate ABI migration: _rt consumers inspecting
data/rule move to labels. Preserve capture boundaries, severity, warning order,
consumption and terminal-failure rules. Tests using _rt must use its established
privilege setup, and ordinary import denial remains covered.

fail!/warning lowering and Opcode::Raise must no longer rely on the language
BlameError structural descriptor. Use a dedicated internal diagnostic emission
operation carrying message plus subject registers and existing rule-boundary
metadata. Raw subjects are observed for provenance internally and never become
a published type. Preserve one evaluation per subject, failure propagation,
best-effort behavior and allocation charging. Native failure helpers and compiler
lowering share this path; they must not maintain divergent diagnostic semantics.

### Native ABI and Bootstrap

Remove validate from both core_prelude_schemes and the exact prelude export
check. Remove blame_error_descriptor after all native declarations and diagnostic
lowering have migrated. Error declarations live in the owning standard modules,
so bootstrap does not need to import codec or Dyn error types.

Native functions must construct the declared error type, including its canonical
identity. Retain result-type evidence at native dispatch for the relevant Result
instantiation, or pass an explicit hidden Type witness in a non-exported native
wrapper. This evidence is resolved from checked module declarations; never look
up types globally by display name. Encoding diagnostics use the original Val
internally without constructing a typed error subject. Keep error-node
construction distinct from raw CodecFailure, which may remain a Rust-only type.

Prefer explicit private witness parameters for module-local helper natives if
the current dispatch cannot supply result evidence. This is an implementation
choice with the same public ABI, not a reason to reintroduce bootstrap Any.
Cycle-sensitive hidden type constructors remain compiler intrinsics with exact
checking contracts; their internal register payloads need not be universal
language values. Document every remaining bootstrap approximation as pending
analysis, never as a valid canonical type.

## Dyn, Value and Host Boundaries

`entry.run` and `entry.serve` take an explicit leading `TypeOf(State)` witness:
`entry.run(State, config, ees_config, prepare)`. Pass that witness to
`actor.service` when packing and projecting state. Delete `_rt.state_type` and
its runtime value-shape inference: empty containers and function fields cannot
recover the declared State contract from their current contents.

Dyn remains explicit packing with a canonical witness, followed by projection
or checked observers. There is no implicit T-to-Dyn assignment or automatic
packing of heterogeneous collections. Keep existing trait/evidence and nominal
identity checks: runtime shape alone cannot recover a named type.

Value remains the existing enum for external data. It does not become a top
type or accept arbitrary language objects by assignment. Source admission and
`--source` handling retain their existing parsing, budgets and provenance.

Host builders must provide a valid type contract where the boundary requires
one. Rust's raw VM value representation may remain heterogeneous internally;
that representation is not a language type. Audit public Rust constructors and
metadata import/export before deleting Any variants. Report actual API or
format changes; do not assume persistence merely because TypeId exists.

Reject authored Any metadata, including nested occurrences and old type-desc
variants, through both tool-stage and runtime admission. Resolve legitimate
named references and bound parameters in their proper environment. Reject
unresolved ones instead of mapping Bound/Named to CodecKind::Any. Recursive
references must still resolve by graph identity and must not be rejected merely
because the graph contains a cycle.

Delete CodecKind::Any's accept-all transform and empty-schema behavior. A valid
schema may still contain `{}` for independent reasons; do not ban schema syntax
by textual search. Preserve `@codec.untagged` enum handling. Rewrite the enum
ambiguity test using two overlapping supported payload contracts rather than
an Any payload.

## Compiler Architecture and Invariants

Classify each internal Any occurrence before replacement:

| Intent | Required representation |
| --- | --- |
| Universally quantified input | Bound generic variable with an authoritative scheme. |
| Pending inference | Existing inference variable/obligation or an explicitly private pending state. |
| Incompatible contributors | A diagnostic, retaining contributor origins. |
| Recovery after an error | Non-authoritative recovery state, excluded from executable publication. |
| Coarse module discovery | Explicitly incomplete fact, refined before authoritative checking. |
| Hidden compiler intrinsic | Accurate scheme or dedicated intrinsic checking contract. |
| Runtime dynamic subject | Dyn with valid evidence. |

Do not rename Any to Unknown while preserving wildcard compatibility. Recovery
may suppress cascading diagnostics; it must not prove an assignment, resolve a
trait instance, select a method, or provide a runtime witness. Valid generalized
module schemes may contain bound variables, but unresolved inference candidates
and error states may not escape as valid exports. Runtime TypeId interning must
accept only fully supported canonical types.

Both coarse descriptor inference and GenericInference need auditing. Preserve
the module graph's import/export discovery and recursive definitions without
promoting preliminary facts into checked interfaces. Workspace queries may
report Pending/Unknown or errors, but must not manufacture valid-looking types
for completion or hover. Partial editor queries and executable publication have
different completion requirements.

Bootstrap cannot simply import a standard-library type that depends on the
bootstrap itself. Hidden struct/enum constructors need exact metadata contracts
or dedicated intrinsic handling; warning/debug operands need generics; cast and
pack need their target/source relationship and evidence. Compare bootstrap
schemes with loaded native declarations, including generic arity and evidence
layout, so initialization order does not change semantics.

Remove Any from final descriptor/node/type-store/reflection variants once all
uses are classified. Keep raw VM representation checks at Host and bytecode
boundaries; stronger static contracts do not make malformed external input safe.

## Compatibility and Migration

| Existing use | Intended replacement |
| --- | --- |
| Any identity/helper parameter | Generic parameter, with shared variables only where a relationship is intended. |
| Array(Any), Dict(Any) | Explicit enum, Value, Tuple where positional, or explicitly packed Dyn elements. |
| Dynamic field/index/call/operator | Concrete contract, enum match, or an existing checked Dyn observer. No universal Dyn call is added. |
| Heterogeneous struct passed to dict helpers | Explicit struct logic or Dyn reflection; homogeneous dictionary callers retain typed helpers. |
| json.schema raw result | Tagged Value result; stringify directly or match Value variants. |
| validate(Target, input) | cast! for representation checks, ty! for static context, or codec.decode for data conversion. |
| Match/unwrap codec.decode or json.decode | Retain Result handling with DecodeError; use fail!(error.message, error.value) when a diagnostic is intended. Audit unwrap contracts. |
| Inspect diagnostic data/rule | Existing privileged consumers use Diagnostic.labels/notes; opening capture to ordinary modules is deferred. |
| Any codec target or metadata | A concrete target or Value data contract; removed metadata is rejected. |
| Export Test erased as Any | Use an existing non-discovered typed container or explicit Dyn to test discovery boundaries. |

Tests that exist solely to demonstrate removed Any behavior should become
static rejection tests. Tests of generic closures, compiler execution, failure
propagation or nominal behavior should use concrete/generic contracts preserving
their original purpose. Do not replace the whole suite with Dyn: that would
change what is exercised and conceal missing static relationships.

Non-RFC documents are updated only when the behavior is implemented, using
positive present-tense rules and examples. Migration explanations belong here.
Historical RFC bodies remain unchanged; acceptance can add precise partial
supersession status entries to the affected RFCs, including 0268.

## Implementation Sequence

1. Retain the current Value throughout decode and check its existing location
   survives error return and fail!. Record baseline and target behavior.
2. Separate pending/recovery facts from valid type contracts, and make bootstrap
   schemes and intrinsic evidence explicit. Add publication invariants.
3. Remove validate; migrate dictionary, decoding, properties, schema and diagnostic contracts
   together with their native implementations and callers. Temporary internal
   transition code is acceptable but cannot be the final removal result.
4. Remove public Any, all metadata admission and runtime canonical variants;
   remove dynamic operation and codec fallbacks. Audit reflection and Host APIs.
5. Migrate fixtures, verify generic/contextual behavior and boundary failures,
   update current documentation and historical status metadata.

Changes may be split into reviewable commits. Do not publish an intermediate
release whose Telora declarations and native representations disagree.

## Validation and Acceptance Criteria

Prefer pure Telora tests for observable behavior:

- Generic identity, ignored parameters, higher-order functions and shared type
  variables preserve relationships across imports and argument permutations.
- Dict keys/values/pairs/merge preserve A, ordering and overwrite policy;
  incompatible value types and unsupported struct callers are diagnosed.
- Contextual enums, nested collections, empty inputs and Never retain the RFC
  0268 rules, without implicit Any/Dyn/Value fallback.
- Built-in validate is unavailable through the prelude and explicit imports;
  user-defined validate bindings remain legal, and migrated checks retain their purpose.
- codec.decode and json.decode publish Result(A, DecodeError). Recoverable
  decoding errors produce no diagnostics until the caller explicitly fails.
  Pure Telora candidate composition can recover from an Err and succeed.
- DecodeError.value retains nested input provenance through propagation and
  untagged candidate trials. fail!(error.message, error.value) reports that data
  location; missing fields report their parent and field/path information.
  All-candidate failure remains an ordinary Err. Generated data gains no invented
  location. JSON syntax failures retain the input location and include the
  parser's available position details in message.
- Host diagnostics and existing privileged capture retain source information,
  warning order and scope ownership, including best-effort behavior; terminal
  limits remain uncaught and ordinary modules gain no new capture access.
  Remaining ordinary error APIs follow explicitly chosen contracts.
- schema returns actual Value, stringifies directly, and preserves recursive
  schemas, enum ambiguity policy and codec properties.
- Explicit Dyn packing/projection retains nominal identity and recursive
  evidence. Value still handles heterogeneous external data.
- Removed Any annotations/metadata and formerly erased operations are rejected
  with useful diagnostics. Unresolved codec metadata is rejected separately.
- Test discovery ignores the supported container and Dyn boundaries, while
  direct exported Test values are discovered as before.

Add explicit lifetime/provenance coverage for a parsed value returned from a
function, exported by an initialized module, and consumed after the parsing
evaluation finishes. Include repeated equal scalar nodes, escaped and Unicode
input strings, empty/EOF errors, missing fields, rename/flatten/default paths,
and relocation across heaps. Check that error propagation retains the original
Val locations and ordinary Host reporting uses them. Add terminal allocation-limit
coverage for error materialization; no new source registration is part of this change.

Verify each error contract in the producer table, including direct encode results
and failures for unsupported inputs, Dyn observers without double packing, and Type reflection
without codec imports. Test both warning records and raised errors through the
existing privileged snapshot path. Ordinary access to _rt remains rejected.
For encoding failures, verify diagnostics retain the failing input location and
codec rule location without an intermediate error value or Dyn wrapper.

Use focused Rust tests for bootstrap/native ABI agreement, malformed Host
metadata, canonical interning, query recovery versus executable publication,
resource accounting, and recursive diagnostic handling. Complete the normal
workspace and language suites using debug builds; no release binary is needed.

Diagnostics should distinguish removed Any, incompatible types, unresolved
context and invalid runtime metadata. Suggestions should match intent instead
of always recommending Dyn. Final source auditing must find no valid public Any
representation or permissive replacement; historical RFC mentions and negative
fixtures are expected to remain.

## Alternatives and Tradeoffs

Keeping explicit Any preserves familiar erased APIs and minimizes migration,
but retains a second dynamic boundary and the current permissive paths.
Removing only dynamic operations while keeping a strict top type is coherent,
but retains canonical/codec/reflection surface for a type with few usable
operations. This RFC removes that surface entirely.

Replacing every Any with Dyn loses parametric relationships and can add packing
cost to otherwise ordinary code. Replacing every Any with Value excludes valid
language objects and conflates external data with language reflection. Neither
is a systematic migration strategy.

The principal risks are bootstrap cycles, lost diagnostic provenance or duplicate
reporting, accidentally accepting unresolved types, changes to nominal checking,
and tests that cease to exercise their intended behavior. The staged inventory
and invariants address these risks; a smaller grep count alone does not.

## Readiness and Implementation Checkpoints

The user has decided to remove validate and make codec.decode/json.decode return
Result(A, DecodeError), with value: Value retaining data provenance for later
explicit fail!. The earlier direct-failure decode proposal is superseded.
Encoding returns Value directly and failures produce diagnostics; EncodeError is
cancelled. Also accepted are typed ParseError,
AccessError and ResolveError, the existing privileged _rt Diagnostic replacement,
typed Dict-only helpers, Value schema output and concrete rename cases. Opening
diagnostic capture is explicitly deferred and does not gate the decode change.
This RFC specifies the remaining error contracts, DecodeError ownership,
syntax-error representation, privileged snapshots, native witness strategy and
runtime provenance requirements. Design decisions are accepted; they have not
been implemented or tested during drafting.

The first implementation checkpoint is retaining the current Value: verify its
existing Val location survives error return and ordinary fail! handling. No new
source ownership system or path-to-value reconstruction is required. The next
checkpoint is native canonical construction and input type evidence; failure
to obtain it must use the documented private witness-parameter strategy rather
than approximate types. These are implementation tasks with defined outcomes,
not unresolved choices about the public error model.

Migration searches already identify validate callers in examples/mvp and the
display, type-families and enum-codec language fixtures. Replace static property
checks with existing type evidence where appropriate and runtime representation
checks with cast!. Negative checks retain Err assertions. Dictionary callers in
stdlib-semantics, codec-schema and std/_entry use dictionary data; compiler-semantics
also tests record construction and needs explicit dictionary context or record
field assertions. Preserve the original test purpose, not merely its pass status.
Complete alias-aware caller searches during implementation, including Rust inline
Telora fixtures and prelude import/shadowing tests.

Before completion, audit public Rust constructors, metadata decoders, type-store
and reflection APIs for removed Any, and add precise supersession status to
historical RFCs. No compatibility decoder or hidden Any is permitted. This
inventory is part of implementation review; it does not authorize rewriting
historical RFC bodies or unrelated Host formats.

The scope is broader than changing a return annotation: it includes preserving
the current Value through decoding and an existing privileged ABI migration. Start implementation
only against the reviewed design, and report provenance/evidence test failures
as failures to meet its contract, not as permission to silently weaken it.

## Implementation Audit

The implementation removes `TypeDescriptor::Any`, `TypeExprId::Any`,
`TypeNode::Any`, `WorkspaceTypeNode::Any`, `TypeId::ANY`, and `CodecKind::Any`.
The three metadata decoders explicitly reject the `Any` tag. Remaining source
mentions are rejection tests, rejection messages, Rust's `std::any::Any` for
opaque Host payloads, and the unrelated `array.any` operation.

Coarse expression inference returns optional evidence. Generic schemes retain
their Bound variables; solved expression records retain generalized parameter
relationships. Unresolved solver descriptors cannot be interned or published
as known expression/definition types. Namespace exports carry member interfaces
with separate schemes, including nested, selective and open re-exports. A
namespace is not published as an unbound ordinary value scheme.

Runtime parsing uses the original input text's provenance for each resulting
Value node. Parser-local source IDs and offsets do not enter the caller's source
database. Semantic wrappers preserve the node's provenance status, and untagged
all-candidate failure retains the first failed payload's current Value while
including every candidate message in deterministic variant order. Static data
modules continue to preserve precise child spans. Unsourced Host arguments
remain unsourced at error materialization.

Private native declarations carry explicit witnesses where necessary:
`codec.decode_with` receives `TypeOf(DecodeError)`; parser natives receive Value
and DecodeError witnesses; schema receives its Value witness. Public `json.decode`
composes `json.parse` and `codec.decode` in Telora, replacing the private combined
JSON decode native. No public decode-with API existed at the baseline. Encoding
returns Value directly and creates no EncodeError or Dyn error wrapper.

### Acceptance Evidence

Paths in the fixture column are relative to `tests/language/src/`.

| Requirement | Implementation and verification evidence |
| --- | --- |
| Generic relationships and contextual inference | `test/explicit-boundary-types`, `test/type-inference`, `test/module-interfaces`, `test/forward-type-contracts`; solver publication and type graph tests in `types/tests/`. |
| Typed Dict operations | `test/stdlib-semantics`, `test/stdlib-collections`, `check/diag-dict-struct-input`, `check/diag-dict-merge-mismatch`. |
| Common contexts, empty collections, Never | `test/explicit-common-type`, `check/diag-common-type-*`, partial/recursive-result-context rejection fixtures. |
| validate removal and identifier reuse | `check/diag-removed-validate`, `check/diag-removed-validate-import`, `test/explicit-boundary-types`. |
| DecodeError, ordinary recovery, composition | `test/decode-errors`; codec and JSON share the same declared error identity. |
| Exact data provenance and lifetime | `test/decode-provenance` checks repeated scalars, Unicode, missing fields, renamed fields, ordinary Telora flatten/default policies, untagged trials, empty/escaped syntax errors, function returns and initialized-module exports. |
| No fabricated Host source | `decode_error_labels_json_data_and_explicit_failure` also invokes the parser with an unsourced Host string and inspects DecodeError.value. |
| Remaining error interfaces | `test/native-errors` checks AccessError, ResolveError and ParseError without a codec import; `test/decode-errors` checks YAML/TOML errors. |
| Direct encode and diagnostic rule/data locations | `test/encode`; `encoding_failure_retains_nested_subject_and_rule_locations` and prepared-display diagnostic tests. |
| Privileged snapshots, warnings, nesting and quotas | `module/tests/part-09.rs` tests snapshot contents, nested capture, warning order and terminal fuel failures; resolver tests check ordinary `_rt` import denial and CLI tests check its query visibility. |
| Error allocation accounting | `semantic_value_parsing_and_encoding_charge_complete_wrapper_graphs` checks exact and one-byte-short budgets for successful outputs and parse/decode error materialization. |
| Value schema, recursive targets and enum policy | `test/codec-schema`, `test/enum-codec`, recursive metadata/codec tests in `module/tests/`. |
| Explicit Dyn and nominal identity | `test/reflection`, `test/nominal-equality`, `test/explicit-boundary-types`, existing recursive Dyn/Host boundary tests. |
| Removed type and invalid metadata | `check/diag-removed-any`, `check/diag-removed-any-metadata`, `check/diag-removed-any-nested-metadata`, `check/diag-removed-blame-error`; canonical interning rejects unresolved metadata. |
| Deferred test discovery boundaries | `test/basic` and its checker preserve direct Test discovery and ignored typed-container/Dyn exports. |
| State witness boundaries | `test/entry-state` exercises empty arrays and function-bearing State through run/serve transitions. |
| Documentation and historical status | Current README, VISION, guide, design and discussion documents use explicit contracts; affected historical RFC status entries link to this RFC without rewriting their bodies. |

The supported built-in JSON properties are RenameAll and Untagged. Flatten and
default policies in the provenance fixture are ordinary Telora data preparation;
this change does not introduce additional codec decorators or struct spread.
Opening diagnostic capture remains deferred.

Final verification (2026-09-07): `mise x -- cargo test --workspace --quiet`
passed, including 269 core tests, 41 CLI tests and the 229 language acceptance
fixture groups exercised by the CLI suite. `git diff --check` passed. Final
searches found none of the removed descriptor/node/runtime variants or erasure
helpers, and no obsolete Any/BlameError/EncodeError references in non-RFC
Markdown documents. All builds used the debug profile; no release binary was
built. The 26 affected historical RFC files each add only one status line.

## Non-Goals

No opening of with_diagnostics to ordinary modules or new public diagnostic
re-propagation API in this iteration. No new dynamic invocation engine, universal equality, record-composition
system, implicit conversion, schema language, source admission policy, or removal
of Dyn/Value/Never. Unrelated trait-import and callback-context defects remain
separate unless evidence shows they directly block this change.
