# RFC 0249: Flat-Val Copy and Boundaries

- Status: Proposed
- Tracking issue: #88
- Depends on: RFC 0245, RFC 0246, RFC 0247, RFC 0248, RFC 0244

## Summary

Telora will complete the flat-Val migration in Work/Work and Work/Main copy,
recursive graph sealing, native callbacks, and legacy Host projection. The
phase removes transitional representations and records correctness, memory,
and performance evidence.

## Copy collector

Copy begins from one or more root `Val`s and uses Meta masks:

- inline scalars and NativeType copy directly;
- Main text, Heap, and uplink references remain unchanged when the MainWorld
  is shared;
- Local text resolves and interns by content in the target;
- Local Heap objects install an object forwarding entry before child traversal;
- Local uplinks use a separate forwarding entry and preserve recursive graph
  closure; and
- Main witnesses remain unchanged and Local witnesses relocate through a
  canonical type forwarding/interner map.

The collector scans distinct reachable Local nodes, not an unfolded tree. A
later copy-GC policy may choose when to invoke this mechanism; this RFC defines
the semantics, not a collection frequency.

## Heap integrity

The Val sub-kind and Heap object header must agree. Initial implementation
retains the object header as the authoritative GC scanner description and
checks the redundant Val sub-kind in debug and tests. Removing object headers
or splitting objects into typed arenas requires separate evidence and is not
part of this RFC.

## Legacy boundary

Public native callbacks continue to use borrowed `ValueRef`. Explicit Host
exports may project to owned `Value` with active-cycle detection and completed
memoization. No internal module, tool, or run-stage transfer may project a Heap
graph merely to cross a World boundary.

## Evidence

The final implementation records:

- `size_of` and alignment assertions for Val and Meta;
- allocation-count tests for short text, NativeType, and narrowing;
- forwarding and sharing tests for Work/Work and Work/Main graphs;
- Fail/Never and provenance regressions;
- the RFC 0242 recursive and QueryBuilder performance protocol; and
- comparison against the accepted RFC 0241 measurements.

## Acceptance criteria

1. RuntimeValue, RichValue, and runtime declared wrappers are removed;
2. all VM and Heap edges use Val as their sole representation;
3. copying retains sharing, cycles, provenance, and witnesses;
4. no short text, NativeType, or narrow operation allocates;
5. legacy cycle and completed-DAG projection semantics remain unchanged;
6. performance does not materially regress from RFC 0241 accepted results;
7. workspace format, tests, and clippy pass; and
8. RFCs 0245 through 0249 record their implemented outcome.
