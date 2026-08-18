# RFC 0234: Evaluation evidence and CLI finalization

- Status: Implemented
- Depends on: RFC 0044, RFC 0103, RFC 0104, RFC 0233
- Tracking issues: #55, #61
- Supersedes: RFC 0104's prohibition on partial internal containers
- Amended by: #93 (ordinary Module pipeline and Host-boundary finalization)

## Summary

Telora uses one recoverable evaluation/evidence graph for diagnostic tools. A
node in that graph is either known, failed with stable diagnostic identities,
unavailable, unknown, conflicted, or skipped because a dependency failed.
Failed nodes are evaluator-private: they are not Telora values or types. They
may cross the internal WorkWorld/MainWorld Module boundary under a stable
failure identity, but cannot cross codec, final return, SystemEffect, or other
Host-visible boundaries.

The public commands consume this machinery with different finalization rules:

```text
show  = query(ordinary Module evidence graph)
check = evaluate(Module, best-effort policy) |> diagnostic/trust decision
run   = strict Main loading |> selected Entry lifecycle
```

`show` succeeding means that the query ran. `check` succeeding means the
ordinary Module pipeline produced no error diagnostic; it does not use a
second Module type or rerun strict finalization. `run` remains fail-fast and
additionally performs Entry scheduling.

## Recoverable evidence graph

Recovery starts from the lossless CST. Incomplete syntax may still contribute
unaffected definitions, references, types, dependencies, and diagnostics when
CST recovery retains enough structure. The graph therefore distinguishes:

```text
Known(value or fact)
Failed(diagnostic ids)
Unavailable(reason)
Unknown(reason)
Conflicted(evidence)
Skipped(failed dependencies)
```

`Failed` records an operation that was actually evaluated and failed.
`Unavailable` records that no executable semantic node could be formed, for
example because syntax or a dependency is missing. Tool output may expose these
states and diagnostic references as Host data, but source code cannot inspect
or construct them.

## Type-transparent failed children

Internally, an evaluated node of static type `T` has the equivalent shape:

```text
Eval(T) = Value(T) | Failed(DiagnosticIds)
```

Struct, Tuple, Array, tagged payload, and Dict nodes may retain failed children
while best-effort evaluation continues. Their surface type does not change.
Internal Module publication may retain a reachable failed child. Publication
to Host-visible data is rejected.

Continuation depends on data dependencies:

- `array.map` preserves shape, skips failed input slots, and continues healthy
  slots in index order;
- `array.length` uses known shape and does not require child values;
- `array.get` propagates failure only when selecting a failed slot;
- `filter` continues independent predicates but cannot publish an output whose
  membership is unknown;
- `fold` stops after its accumulator fails;
- sorting and other globally dependent transforms publish no partial answer
  after a comparison failure.

Propagation reuses root diagnostic identities. It does not create repeated
user-facing diagnostics merely because several later nodes depend on one root.
Resource exhaustion, cancellation, invalid bytecode, and evaluator invariant
failures terminate the complete session rather than becoming child failures.

This supersedes RFC 0104 where it says that Never is never retained inside an
internal container. RFC 0104's atomic publication rule remains in force.

## `check`

`telora check <module>` evaluates the selected ordinary Module with the
best-effort scheduling policy:

- independent work after a recoverable failure may add diagnostics;
- warnings alone do not fail finalization;
- any syntax, type, resolution, or runtime error produces a nonzero exit and no
  Host-visible result, even if the selected root does not depend on that failure;
- stdout is a `telora.check/v1` JSONL stream containing zero or more diagnostic
  records followed by exactly one summary record; only a graph without error
  diagnostics produces `status: "ok"` and a zero exit;
- exported closures are already-computed values; invoking one later is a
  separate computation outside this check.

Dependency reachability controls which additional computations best-effort may
perform, not whether its result may be delivered. A clean root can sometimes be
computed after an unrelated internal failure, but the complete evaluation has
already lost publication authority. Any error diagnostic makes `check`
nonzero and prevents Host delivery. The internal Module graph remains
available to `show` and downstream diagnostic evaluation. A subsequent strict
`run` is the authoritative application acceptance run.

Across module boundaries, best-effort always retains an ordinary Module. A
Module's availability says only whether its source exists; individual facts
carry known, unknown, conflicted, or incomputable state. Healthy exports remain
usable by independent downstream work, while failed exports propagate the same
stable failure identity. The boundary does not manufacture another diagnostic.

## `show`

`telora show` queries the graph directly and does not finalize it. It can return
unaffected facts adjacent to damaged syntax or failed evaluation. A successful
empty result is still a successful query. Every record states its authority or
recovery status; it must not turn an unavailable or failed value into `Any` and
present that approximation as authoritative.

## Acceptance criteria

1. `check` is nonzero for `fail!`, division by zero, Array out-of-range, type
   errors, syntax errors, and unavailable dependencies.
2. `check` succeeds for a module with warnings and no errors, without executing
   a duplicate strict/recovery Module pipeline.
3. `show` retains unaffected facts for recoverable syntax and evaluation
   damage.
4. Multiple independent failures have deterministic, deduplicated diagnostics.
5. `array.map` continues healthy slots after recoverable callback failures.
6. shape-only operations remain usable internally when children failed;
   selecting a failed child propagates its diagnostic identity.
7. internal Module publication may retain failed nodes, while no failed node
   crosses codec, final return, SystemEffect, or another Host-visible boundary;
   `dbg!` may render a bounded Host-only `<failed>` marker.
8. a clean root computed after an internal failure remains diagnostic evidence
   only: `check` and `run --best-effort` are nonzero and publish no value.
9. strict `run` behavior and successful values remain unchanged.

## Implementation plan

1. make `check` consume the ordinary Module workspace under the best-effort
   scheduling policy and decide success from concrete diagnostics;
2. retain private failed children at ordinary result registers and native
   continuation boundaries;
3. implement dependency rules for common Array operations and general
   structural publication blocking;
4. project node states and diagnostic references into `show` records;
5. update the language SSOT and CLI tutorial and cover all acceptance cases.

## Implementation status

The `check` behavior is implemented by the unified workspace Module pipeline.
Each module is analyzed, compiled, and evaluated once under the best-effort
scheduling policy. `fail!`, division by zero, out-of-range indexing, and other
concrete errors produce diagnostics and `status: "error"`; warnings alone do
not. There is no synthetic incomplete-module diagnostic and no duplicate
strict pass used merely to judge success. The summary contains the stable
module ID, status, and dependency count. Expected language rejection is
structured stdout plus a nonzero exit, not mixed text; ordinary stderr remains
reserved for CLI/Host faults and the separate `dbg!` observation channel.

The #61 implementation adds crate-private failed heap nodes and a best-effort
VM recovery loop at ordinary result-register and native callback boundaries.
Direct structural expressions can therefore retain failed children and keep
evaluating independent siblings. Array and Dict map operations retain failed
slots and continue healthy slots. Filter and flat-map continue independent
callbacks but return failure because output shape is unknown. Fold stops when
its accumulator fails. Array length remains shape-only, and get propagates a
selected failed slot. Root diagnostic identities propagate without creating
new diagnostics. Failed nodes may relocate between WorkWorlds and across
internal Module publication into MainWorld; codec, final return, SystemEffect,
and other Host-visible publication reject them.

`run --best-effort` performs this diagnostic Main pass before Entry startup. It
emits `telora.run/v1` JSONL diagnostics on stderr. Any error diagnostic makes
the command nonzero and discards the recovered Main, even when its selected
return root happened to compute successfully. No Entry starts and no effect is
committed in that case. With no errors it proceeds through a fresh, unchanged
strict Entry and Host-effect lifecycle; default `run` remains fail-fast.
