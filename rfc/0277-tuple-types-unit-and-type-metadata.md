# RFC 0277: Tuple Types, Unit, and Explicit Type Metadata

- Status: Stage 1 committed locally. Stage 2 accepted; implementation in progress.
- Baseline: Current [language design](../docs/design/LANGUAGE.md), sections 3,
  4, 6 and 7, and [type concepts](../docs/design/CONCEPT.md).
- Related: RFC 0219 (Function notation and Tuple contracts), RFC 0272 (Tuple
  spread), RFC 0276 (shared inference and static property evidence).

## Motivation and Scope

Write tuple types with the same positional structure as tuple data, including
the empty tuple, while making the boundary between types and metadata values
explicit:

```telora
type Pair = (Int, String);
let pair: (Int, String) = (1, "hello");
let metadata = Pair.type;
let metadata_pair = (Int.type, String.type);
let unit: () = ();
let also_unit: Unit = ();
```

Examples containing `let` are ordinary block fragments, not module-level
bindings. This proposal does not change the module declaration boundary.

The proposal also lets normally completing blocks without a tail expression
return `()`. Function type notation retains `Fn` unchanged.

This is a semantic language change, not a parser-only abbreviation. RFC 0219
deliberately preserved `(Int, String)` as data containing two metadata values.
This proposal replaces that decision by separating bare types from data.
It does not change the current design SSOT until accepted and implemented.

## Staged Acceptance

Stage 1 is accepted: add the `Unit` alias, `()` in explicit type slots, and
normal-fallthrough block results with expression statements. Preserve `Fn`,
Never, current metadata values and nonempty tuple semantics. This stage does
not wait for the type/data redesign below.

Explicit type slots include annotation roots, type initializer roots, declared
member types, explicit type arguments, and restricted contracts (including their
nested arguments).
An ordinary expression call still passes data arguments: `Array(Unit)` remains
the spelling usable in general metadata expressions; this stage does not
reinterpret `Array(())` there or rewrite arguments to user metadata helpers.
Parenthesized grouping around an explicit empty type is transparent. The `()`
type notation must not depend on whether the public name `Unit` is shadowed.

Stage 2 is accepted with the following boundaries. Types are produced only by
declarations, structural type constructors and parameterized type families.
Ordinary metadata computations cannot become static types, including in a
`type` initializer or an annotation. `T.type` is a one-way bridge with result
`TypeOf(T)`, assignable to `Type`; reflection does not provide the reverse bridge.
`let` and `def` bind data, while `type` binds types. Metadata-valued locals and
ordinary function parameters remain data even when their static type is TypeOf.

Type roles follow resolved declarations and module interfaces, not spelling or
runtime values. Nominal value constructors keep their existing callable facet;
this is distinct from passing their type metadata. Function notation retains
`Fn`. Empty tuples follow their explicit enclosing type/data role. Nonempty
tuple type items recursively occupy type slots. Type-tuple spread is deferred;
ordinary data tuple spread remains unchanged. Ordinary metadata APIs consume
explicit `.type` values and cannot modify type skeletons or create static types.

Existing tests for programmable metadata-to-type conversion must become focused
rejection cases; their existence does not justify preserving the capability.
Internal canonical type construction may reuse the existing representation and
trusted machinery, but must not execute arbitrary user helpers to determine a
type skeleton. The audit and open questions below record the earlier discussion;
these accepted rules supersede the suggested reverse bridge.

### Stage 1 Implementation and Verification

The accepted RFC was committed as `e694387` before implementation. Local changes
add the empty-tuple metadata alias to both prelude projections, lower explicit
empty type syntax through a non-shadowable internal binding, and admit Unit
contracts without changing Function arity. Ordinary metadata call arguments
retain their prior data meaning.

Module and local body grammar rules are now separate. Local expression
statements lower to hygienic sequential `let` bindings, and normal fallthrough
synthesizes the existing empty-tuple value. Strict inference and provisional
projection retain Never from nonreturning initializers. No new runtime value
kind, instruction or inference-environment copy is introduced.

`cargo test --workspace` passed: 314 core tests, 41 CLI tests including the
language acceptance suite, and the remaining workspace suites. Five new language
cases cover metadata identity, block tails, branches and discarded/bound failure.
Compiler tests check ordered, once-only diagnostic execution and explicit return;
additional focused Never tests cover failure after tuple destructuring. The
tree-sitter submodule grammar and generated artifacts are updated; all 12 corpus
tests pass using a writable temporary cache. Parent and submodule diff checks
pass. The language design, implementation design and public guide now document
stage 1 only.

No release performance measurement was made for stage 1; the broader type/data
inference changes and their performance acceptance remain part of stage 2.
Implementation and editor-submodule changes remain uncommitted and unpushed.

## Current Baseline

Today `Tuple([A, B])` calls an ordinary metadata constructor with one Array
argument. Bare types also serve as first-class metadata values. `Type` describes
arbitrary valid metadata; `TypeOf(A)` retains the described instance type.
Families expose both a type application and a callable metadata witness facet.
Annotations and metadata computations share the ordinary tool-stage VM.

Parentheses already support data tuples, grouping, singleton tuples and empty
tuples. Ordinary block lowering currently requires a result expression; the
module-body path can synthesize an empty tuple. General `expression;` statements
are not currently part of the block grammar. There is no dedicated `.type`
projection or public `Unit` alias.

Consequently the proposal affects metadata helpers and family witnesses as
well as authored contracts. It must not silently replace a runtime tuple with
a tuple type after inspecting its elements at runtime.

## Proposed Semantics

### Types and Data

Bare `Int`, `String`, type aliases and bound type parameters denote types.
They do not implicitly become ordinary data values. Name resolution and the
resolved binding's role determine this distinction, never capitalization.

For nonempty tuple syntax, type operands construct a tuple type; data operands
construct tuple data. Mixing these roles is an error:

```telora
(Int, String)              // tuple type
(1, "hello")              // tuple data
(Int.type, String.type)    // tuple data containing metadata
(Int, 1)                  // error: mixed type and data operands
(Int.type, String)         // error: mixed data and type operands
```

An ordinary data parameter cannot accept a bare type by implicit metadata
conversion. A function receiving metadata values and returning `(x, y)` always
returns tuple data, even when both values describe types. Expected types must
not reinterpret that function's body or runtime result as a type constructor.

Phase classification is a static frontend responsibility. Incomplete evidence
must remain unresolved or produce a diagnostic, not select a meaning based on
evaluation order. The precise binding and elaboration rules are acceptance
blockers listed below; this draft does not introduce a second evaluator.

### Grouping, Arity, and Contracts

```telora
(A)                       // grouped type A
(A,)                      // one-element tuple type
(a)                       // grouped data expression a
(a,)                      // one-element tuple data
((Int, String), Bool)      // nested tuple type
```

Tuple types are fixed-length structural products with ordered elements. Aliases
do not create nominal identity. Existing positional checking, nesting,
projections and pattern behavior remain intact.

Every contract position must accept the notation consistently: type aliases,
local annotations, closure annotations, `def`, `decl`, `native`, trait contracts,
family arguments, and nested Function parameters and results.

```telora
Fn(Int, String) -> Bool              // two parameters
Fn((Int, String)) -> Bool            // one tuple parameter
Fn() -> ()                          // no parameters, Unit result
Fn(()) -> ()                        // one Unit parameter
Fn(A, B) -> Fn(C) -> Fn((D, E)) -> R
```

`Fn` remains required, with the existing arity and arrow association. Its
internal elaboration may need explicit metadata conversion, but this proposal
does not change Function descriptors, calling convention or variance.

### Explicit Metadata

`T.type` converts a type-layer operand into metadata data describing precisely
`T`. It preserves canonical type identity and is not an arbitrary dictionary
of fields. Metadata tuples are therefore written `(A.type, B.type)`.

The suggested static result is the existing `TypeOf(T)`, assignable to `Type`;
the final witness/API contract must be settled before implementation. This
operation does not mean "the type of an arbitrary data value". Applying it to
ordinary data should produce a targeted diagnostic.

`type` is already a keyword. `.type` is dedicated suffix syntax, distinct from
ordinary field lookup, including on module-qualified types and parenthesized
tuple types such as `(Int, String).type`. It must not dispatch through a field
named `type`. Raw identifiers such as `r#type` are deferred.

Metadata access preserves the separation between the type skeleton and the
out-of-band property registry. It must not force all property providers merely
to obtain a type identity. Static property evidence continues to follow provider
return contracts; property queries and successful publication retain their
existing evaluation and failure requirements.

### Empty Tuple and Unit

In an explicit type position, `()` denotes the zero-element tuple type. In a
data position, it constructs the sole empty-tuple value. `Unit` is a prelude
type alias for `()`, not a separate nominal type or runtime representation:

```telora
type Empty = ();
let a: Empty = ();
let b: Unit = a;
```

The zero-element case cannot be classified from its operands. Explicit type
slots and ordinary data slots supply its role; the complete rule for ambiguous
nested expressions remains to be specified. `Unit` itself is still a type, so
`Unit.type` obtains metadata. Compatibility with user bindings named `Unit`
must follow the existing prelude resolution/shadowing policy.

### Block Tails

Allow sequential expression statements terminated by `;`. A block with a tail
expression returns that expression; a block that reaches its end without one
returns `()`:

```telora
do { let a = f(); }        // Unit on normal completion
do { f(); }               // evaluate f once, discard its result, return Unit
do { f() }                // return f's result
do {}                     // Unit
do { f(); g() }            // evaluate in order, return g's result
```

Apply the same rule to function bodies and ordinary blocks used in branches.
Do not add an implicit `else`, change `return` syntax, or turn module bodies
into sequential statement blocks. Bare `{}` retains its dictionary meaning
where expression syntax is ambiguous; `do {}` explicitly selects a block.

The implicit Unit result exists only on normal fallthrough. `return`, failure,
and other `Never` paths do not become Unit because of a trailing semicolon.
For example `do { fail!("Stopped", (), ()); }` does not return normally. Branch
joining must continue to distinguish bottom compatibility from type equality.
Discarded expressions are still checked and evaluated, retaining failures,
effects, provenance and execution quotas. A semicolon is not an erasure request.

## Compatibility and Open Decisions

### Stage 2 Compatibility Audit

Implementation preparation confirms that the metadata-to-type bridge is used
by existing programs, not merely a hypothetical reflection feature.
`tests/language/src/test/newtype-tool-stage/testee.telora` constructs types
through ordinary helpers, projections, imported constructors and conditionals.
`std/type-desc` consumes `Type`, while codec and property APIs consume exact
`TypeOf(A)` witnesses. Removing computed type initializers would remove tested
capabilities in addition to changing tuple notation.

The recommended migration boundary below is pending confirmation:

| Surface | Proposed stage 2 treatment |
| --- | --- |
| `type Pair = (Int, String)` | Construct a structural tuple type. |
| `let metadata = Pair.type` | Bind data with the exact `TypeOf(Pair)` witness. |
| `let Pair = (Int, String)` | Reject a type in a data binding; suggest `type`. |
| `codec.decode(Target, value)` | Pass `Target.type` as metadata data. |
| `type_property.get_type_prop(T, P)` | Pass `T.type` and `P.type`. |
| `type T = metadata_factory(Int)` | Preserve tool-stage computation, with explicit `Int.type` input. |
| `type T = computed_metadata` | Validate and seal metadata at the existing tool-stage declaration boundary. |
| A runtime metadata value | Do not reinterpret it as an unchecked static type. |

The last three rows require an explicit decision: separating bare types from
metadata data does not itself specify the reverse bridge. The recommendation
preserves the existing `type` declaration boundary without inventing another
conversion operator. It does not authorize implicit conversions in ordinary
data calls or runtime-dependent static types. Constructor/family callable
facets, legacy computed `Tuple` calls and type-tuple spread still need their
detailed migration rules before stage 2 can be marked implemented.

The following must be resolved before accepting the implementation plan:

1. Define static type/data roles for bindings, calls, imports, aliases, generic
   parameters and empty tuples. In particular, decide whether
   `let Pair = (Int, String)` can bind a type or must use `type Pair = ...`.
   Do not decide this using capitalization or runtime metadata inspection.
2. Finalize the `T.type` witness contract and the bridge, if any, from computed
   metadata back into a type position. Retain programmable metadata without
   allowing runtime values to determine unchecked static types.
3. Audit ordinary metadata constructors, `Func`, parameterized family callable
   facets, generic witnesses, codecs, Dyn and property APIs. Specify which
   arguments are type arguments and which are explicit metadata data. Keeping
   `Fn` notation does not by itself preserve explicit `Func` calls.
4. Decide the migration lifetime of `Tuple([A, B])`. Literal authored type
   contracts can become `(A, B)`, but computed metadata constructors such as
   `Tuple(items)` need a supported bridge. Do not remove them by textual rewrite.
5. Preserve RFC 0272 data spread, including once-only ordered evaluation and
   fixed shape. Define type-tuple spread separately or explicitly reject it in
   the initial version; runtime metadata tuples must remain data when spread.
6. Specify the role of `()` in nested forms such as `((), Int)` and `((), 1)`
   without expected-type-driven reinterpretation of ordinary helper calls.

Existing data `(Int, String)` must migrate to `(Int.type, String.type)` when
its intent is a metadata collection. Arrays of metadata similarly need explicit
conversion under the proposed boundary. Diagnostics should identify the offending
bare type and suggest `.type` only in a known data position.

## Implementation Plan

After the open decisions are accepted and this RFC is committed:

1. Audit repository and standard-library metadata consumers; record an explicit
   compatibility/migration matrix before changing name resolution.
2. Extend the authoritative grammar and tree-sitter grammar together for tuple
   contracts, `.type` and expression statements. Preserve grouping, delimiters,
   trailing commas, recovery and authored spans.
3. Represent type/data roles explicitly through resolution and HIR elaboration.
   Keep inference on the shared slot graph; do not re-evaluate every expression
   or clone environments to guess whether a tuple denotes a type.
4. Lower tuple types to the canonical type structure and data tuples to ordinary
   tuple construction. Add the Unit alias and normal-fallthrough block lowering.
5. Migrate metadata bridges and consumers, then update the design SSOT and
   public guide in the same implementation work. Preserve historical RFC text.
6. Verify syntax, semantic queries, inference, execution and recovery; measure
   inference-heavy checks against the unchanged baseline.

## Executable Acceptance Criteria

Implementation must add fixtures and tests demonstrating:

- Empty, singleton, nested and multi-element tuple types work in every contract
  position; corresponding values check with exact arity and element types.
- `Unit`, `()` and an empty-tuple alias have identical structural type identity;
  nominal wrappers remain distinct. Grouping never adds a tuple layer.
- Type tuples produce tuple metadata only through the explicit bridge; metadata
  data tuples retain ordinary tuple runtime construction and precise witnesses.
- Mixed type/data operands and bare types passed to data parameters fail locally;
  aliases, lowercase type names, uppercase data names and imports behave alike.
- `.type` preserves exact identity, parses after qualified/parenthesized types,
  rejects data operands and does not trigger unrelated property evaluation.
- `Fn(A, B)` and `Fn((A, B))` retain distinct arities; zero arguments and one Unit
  argument remain distinct. Missing `Fn` arrows remain unsupported.
- Empty and semicolon-ended blocks return Unit only on normal completion;
  expression statements execute once in order. Failures, explicit returns and
  Never joins preserve existing behavior, including in function/branch bodies.
- Existing tuple spreads, patterns, projections, generic inference, nominal
  identity, metadata reflection and module publication do not regress.
- Malformed tuple contracts, suffixes and statements recover locally without
  publishing invented successful types. Hover/display reflect the new syntax.
- Formatting, workspace tests and relevant editor grammar tests pass. Release
  measurements include a large tuple/type workload and the existing ontology
  `check @test/query` workload; record time and peak memory without claiming an
  unmeasured speedup.

## Deferred Alternatives and Non-Goals

No bare-arrow Function syntax, quote-prefixed tuple types, raw identifiers,
variadic functions or new tuple runtime layout are introduced. Metadata values
remain available explicitly; eliminating reflection or programmable metadata
is not a goal. Runtime "all elements are Type" dispatch is rejected because it
would change ordinary data construction based on values. A broad redesign of
type-level computation requires a follow-up decision, not an incidental parser
change hidden in this RFC.
