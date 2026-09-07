# RFC 0275: Construction Checks and Unchecked Values

- Status: Draft; design questions below must be resolved before dependent implementation.
- Tracking: [#168](https://github.com/hh9527/telora/issues/168)
- Branch: `feat/0161-unified-constructors`
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

Support `@check(func)` on named-field structs, newtype structs and individual
enum variants. Whole-enum checks remain deferred. Newtypes retain positional
payload access through `.0`; general named tuples remain outside this scope.

Members remain public. Validation is part of construction, so users do not need
to remember to invoke a separate trait method. Exporting or passing a constructor
as a function must preserve its check.

## Unchecked Representation

`Unchecked(T)` is a distinct builtin type derived from T's representation. It
removes only the outer check guarantee: a field declared as another checked
type still contains a valid value of that field type. It must preserve the
candidate's nominal origin and generic arguments while remaining distinct from T.

An unchecked value may not become T through implicit conversion, an arbitrary
metadata witness, static ascription or Dyn projection. Successful construction
performs the final wrapping under compiler/runtime control.

The exact construction and observation API needs to be settled below. In
particular, supporting the same shape does not by itself authorize every field
projection, pattern or codec operation available on T.

## Construction Protocol

Construction evaluates and type-checks inputs, forms an unchecked candidate,
invokes the declaration-bound check, and publishes T only after success.
Ordinary construction failure produces a diagnostic. Codec decoding returns
its typed DecodeError on validation failure, preserving the original input Value.

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

## Decisions Required Before Implementation

The following questions are carried forward from RFC 0274; moving this work to
a separate RFC does not silently decide them.

1. Define the supported T domains and exact unchecked API. Named-field access,
   newtype `.0`, constructor patterns, reflection, equality and explicit conversion
   need consistent rules. Decide whether unchecked values can be authored outside
   check functions and what public operations, if any, construct them.
2. Decide whether newtype checks receive `Unchecked(T)` or the payload directly.
   Specify variant check inputs without inventing standalone variant types, and
   define when a unit variant's check runs.
3. Choose a concrete error contract. `Option(Error)` is a design placeholder;
   no standard Error type has been established by that notation. Specify success,
   rejection and a check function that itself raises a diagnostic.
4. Specify ordinary failure diagnostics and conversion into codec DecodeError,
   including which input Value is retained for nested and untagged decoding.
5. Define registration and execution phases. Check expressions may reference
   declarations, generic parameters, imported helpers and recursive types. The
   module/metadata scheduler must make the required evidence available before use.
6. Define recursion behavior when checks construct further checked values.
   Reentrancy must neither bypass checks nor create an unbounded host recursion
   path outside normal quota accounting.
7. Define how generic metadata, reflection, publication and codec plans retain
   check functions and their captured values across heaps and module boundaries.
8. Audit all dynamic and metadata operations that could manufacture T, including
   checked casts and Dyn projection. Reading a valid checked value and creating a
   new one must remain distinguishable.

## Implementation Plan

First settle the above contracts and add focused language examples for their
observable results. Then implement unchecked identity and observation, followed
by check registration and constructor enforcement. Integrate merge-update,
projection, codecs, tool evaluation and dynamic boundaries before acceptance.

Use the repository's existing declaration identity, typed-property scheduling,
constructor evidence and Val provenance mechanisms where they satisfy the
contract. Additional metadata must preserve canonical identity through generic
substitution, imports, reexports and module graph cycles.

## Acceptance

- Positive and negative `.telora` cases cover structs, newtypes, payload variants
  and unit variants, with explicit and inferred generic arguments.
- Exported, imported, reexported and first-class constructors enforce the same check.
- Every creation path listed above is covered, including nested checked fields,
  intermediate merge failures and projection into a checked target.
- Copying and reading checked values do not repeat checks; unchecked values cannot
  escape as checked values through any static, dynamic or metadata operation.
- Diagnostics preserve input and construction provenance. Codec rejection retains
  the appropriate original Value and supports untagged trial decoding.
- Recursive checks obey normal fuel and resource limits. Cross-heap publication
  preserves captured check functions and nominal identities.
- Guides describe implemented behavior positively. Historical RFCs are not rewritten
  as migration guides. New behavioral tests primarily use `.telora`.
- Debug builds and workspace tests pass. Report evidence in Chinese on #168;
  close the issue only after its full implementation scope is complete.
