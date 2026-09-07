# RFC 0119: Typed pattern analysis

- Status: Implemented
- Partial supersession by [RFC 0269](0269-remove-any.md): Missing subpattern type evidence is explicit absence, not Any.
- Depends on: RFC 0118

## Summary

Forma introduces one shared, pure analysis for the existing Pattern AST. Given
a pattern and a resolved scrutinee `TypeDescriptor`, it derives:

- every lexical binding and its selected type;
- known-compatible, known-incompatible, or unknown compatibility;
- whether the pattern is irrefutable for that static type;
- repeated binding locations; and
- conservative whole-variant coverage for a closed Enum.

This RFC adds no syntax and enables no new rejection by itself. Type inference
uses the shared binding result in place of its private recursive helper. Later
RFCs consume the remaining facts to add Struct patterns, exhaustiveness,
redundancy diagnostics, and destructuring `let` without implementing competing
pattern semantics.

## Analysis contract

The analysis is structural and deterministic. Wildcard and binding patterns
are always compatible and irrefutable. Tuple and Tagged patterns recursively
select child types when the scrutinee shape is known. Literal patterns are
irrefutable only for an exact singleton static type, such as the matching Atom.

`Any`, unresolved inference variables, bound parameters, and open unions do
not justify a negative conclusion. Their compatibility is Unknown, nested
bindings conservatively receive Any where no selected type is known, and the
pattern is not considered irrefutable unless it is a wildcard or binding.

For a known Enum, a unit Atom covers its variant. A Tagged pattern covers its
whole payload variant only when its payload subpattern is irrefutable for the
declared payload type. A catch-all covers all variants. Several refutable
payload patterns are not combined into a proof.

Repeated names are recorded in authored traversal order. The first occurrence
defines the binding type; each later occurrence is a duplicate fact and does
not silently replace it.

## Integration

The analysis lives beside the AST and type descriptor rather than in parsing,
compilation, or workspace projection. Type inference populates arm environments
from its binding facts. HIR continues to own lexical definition/reference
identity; subsequent RFCs will align its diagnostics and semantic facts with
the same traversal.

The result contains no VM handles, workspace IDs, or Host state. It is safe to
recompute per snapshot and observes cancellation only through its caller's
existing bounded AST traversal.

## Compatibility

Existing well-formed and malformed matches keep their current runtime and
diagnostic behavior in this RFC. In particular, known-incompatible patterns,
duplicates, missing Enum variants, and redundant arms are facts only until the
child RFC that defines their user-visible diagnostic.

## Acceptance criteria

1. one analysis handles every existing Pattern variant;
2. exact Tuple and Tagged child types flow into nested bindings;
3. unknown shapes give nested bindings Any without an unsound negative claim;
4. irrefutability distinguishes catch-alls, exact structural patterns, and
   refutable literals;
5. whole-variant Enum coverage requires an irrefutable payload pattern;
6. duplicate bindings retain the first binding and record later locations;
7. inference uses the shared binding facts with no behavior regression;
8. no syntax, bytecode, runtime, or public language API changes; and
9. full tests and strict static checks pass.

## Non-goals

- emitting new diagnostics;
- Struct patterns or destructuring `let`;
- complete nested-pattern usefulness analysis;
- flow-sensitive narrowing;
- row polymorphism or open-record reasoning; or
- exposing pattern-analysis internals as an embedding API.

## Implementation result

Added a dedicated pure pattern-analysis module over the existing AST and
`TypeDescriptor`. It reports ordered first bindings, duplicate occurrences,
three-state compatibility, irrefutability, and conservative whole-variant Enum
coverage. Focused tests cover precise nested Tuple/Tagged selection, unknown
shape fallback, payload-sensitive Enum coverage, and duplicate preservation.

Type inference now builds match-arm environments from the shared binding facts
instead of its former private recursive helper. The other facts remain inert
until their owning child RFC enables diagnostics or syntax. All existing core
tests and strict Clippy pass, confirming no language or runtime behavior
changed in this foundation phase.
