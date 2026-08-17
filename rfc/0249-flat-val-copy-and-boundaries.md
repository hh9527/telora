# RFC 0249: Flat-Val Copy and Boundaries

- Status: Implemented
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

## Outcome

`Val` is now the only stored VM value representation. The former
`RuntimeValue`/`RichValue` names were removed; `DecodedValue` remains only as a
transient private decoding view and is never stored in a register, Heap edge,
or boundary value.

Each collector has fixed source and target Worlds. Its object forwarding table
is therefore exactly `HashMap<u32, u32>`, mapping a source arena slot to a
target arena slot. Main references are recognized by their scoped high bit and
retained before forwarding. Raw Heap references, uplinks, and `ty` witnesses
all use the same object map. A forwarding entry is installed before child
scanning, preserving cycles and shared subgraphs.

The RFC 0242 release protocol produced these three-sample median user times on
the implementation host:

| fixture | RFC 0241 accepted | flat `Val` |
|---|---:|---:|
| flat-functions | 0.076314s | 0.074788s |
| recursive-functions | 0.116674s | 0.117637s |
| nested-functions | 0.217471s | 0.217537s |
| recursive-values-shallow | 0.040793s | 0.041378s |
| recursive-values-growing | 0.038608s | 0.037188s |
| query-builder-check | 0.405207s | 0.398406s |
| query-builder-show | 0.401056s | 0.411054s |

No fixture materially regressed. Core tests and clippy pass except for the
independently reproduced declared-family identity baseline recorded on issue
#88.
