# RFC 0244: Completed Recursive Graph Memoization

- Status: Proposed
- Tracking issue: #83
- Depends on: RFC 0241, RFC 0242, RFC 0243

## Summary

Telora graph consumers will distinguish an active visit from a completed
result. Active visits retain cycle semantics; completed visits reuse the prior
result. This prevents shared recursive DAGs from being expanded as trees while
preserving errors for unsupported cycles.

## Legacy `Value` projection

The heap-native representation supports shared and cyclic graphs. The legacy
owned `Value` representation supports sharing through cloned `Arc` container
payloads but does not represent arbitrary cycles.

Projection therefore uses:

- `visiting: Set<Handle>` to reject a back-edge into the active path; and
- `completed: Map<Handle, Value>` to reuse an already projected object.

An object enters `completed` only after every child succeeds. A failed or
cyclic projection is never cached as success. Reusing a completed `Value`
preserves its shared `Arc` storage and avoids repeated allocation.

The cache scope is one logical multi-root projection. APIs that project a set
of related roots should share one context rather than starting one cache per
root.

## Structural type pairs

Anonymous structural comparisons may memoize completed stable pairs. Keys use
interned `TypeId` pairs when operating in `TypeGraph`; descriptor-level code
uses explicit stable identity owned by the inference context. Full `Debug`
strings and display names are forbidden as keys.

Results involving unresolved inference variables are not cached across a
substitution mutation. Implementations may use an inference-generation number,
clear the cache when substitutions change, or limit caching to closed pairs.

## Existing heap copy

`PendingCopy` already installs a destination handle before copying children and
uses forwarding tables for objects, text, and shapes. RFC 0244 adds regression
coverage for that behavior but does not replace it. Any later copy optimization
must retain the same pre-order forwarding invariant.

## Acceptance criteria

1. a shared acyclic heap node is projected once per logical projection;
2. repeated results reuse owned `Value` storage where the representation
   supports sharing;
3. a true cycle still reports the existing legacy-boundary error;
4. Work/Work and Work/Main copies preserve shared destination handles;
5. closed structural type pairs may reuse completed results without stale
   inference substitutions;
6. the growing recursive-value fixture is approximately linear in distinct
   nodes rather than exponential in unfolded leaves; and
7. correctness and performance measurements from RFC 0242 pass.
