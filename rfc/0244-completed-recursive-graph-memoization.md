# RFC 0244: Heap-Native Tool Evaluation and Boundary Memoization

- Status: Implemented
- Tracking issue: #87
- Depends on: RFC 0241, RFC 0242, RFC 0243

## Summary

Telora's tool stage will retain values in one Main Heap. Each expression runs
in a fresh Work World using external links to persistent Main roots, then
publishes its result into Main. It will not use legacy owned `Value` as an
internal interchange format. At unavoidable Host boundaries, graph consumers
distinguish an active visit from a completed result.

## Heap-native tool context

The context owns a `Vm`, a Main `Heap`, and a map from binding name to
`PersistentValue`. Prelude and Host-provided owned values are published once.
Expression compilation emits external constant links for binding names rather
than cloning owned constants. Evaluation calls `execute_in_work`, and its root
is published before it becomes visible to later expressions.

Composite values never leave Heap storage during this process. Immediate
values copy directly. Other values use cheap `(storage, slot)` handles and the
existing forwarding maps during publication. Only `String` and `Atom` intern
references have cross-World value comparison semantics; the text relocation
map preserves those semantics.

## Legacy `Value` projection

The heap-native representation supports shared and cyclic graphs. The legacy
owned `Value` representation does not represent arbitrary cycles and is not an
internal storage strategy.

Projection therefore uses:

- `visiting: Set<Handle>` to reject a back-edge into the active path; and
- `completed: Map<Handle, Value>` where an unavoidable projection can safely
  reuse an already projected object.

An object enters `completed` only after every child succeeds. A failed or
cyclic projection is never cached as success. This optimization is secondary
to retaining Heap roots internally.

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

1. tool bindings are persistent Main Heap roots rather than owned `Value` trees;
2. each tool expression executes in Work and publishes exactly once to Main;
3. a shared acyclic heap node is projected once per unavoidable projection;
4. a true cycle still reports the existing legacy-boundary error;
5. Work/Work and Work/Main copies preserve shared destination handles;
6. `String`/`Atom` relocation preserves value semantics across Worlds;
7. closed structural type pairs may reuse completed results without stale
   inference substitutions;
8. the growing recursive-value fixture is approximately linear in distinct
   nodes rather than exponential in unfolded leaves; and
9. correctness and performance measurements from RFC 0242 pass.

## Outcome

Tool evaluation now holds `PersistentValue` roots in one Main Heap and loads
them through bytecode external links. A fresh WorkWorld is published directly
to Main after each expression. TypeMetadata decoding reads the persistent Heap
view directly.

Legacy projection carries both an active set and a completed map. A regression
constructs a Tuple with two edges to one Array and verifies that both exported
children share one `Arc<[Value]>`; a true self-cycle remains rejected. The
growing recursive-value fixture's median user time fell from 16.166534 seconds
to 0.038608 seconds in the final accepted command path.
