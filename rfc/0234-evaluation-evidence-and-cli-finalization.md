# RFC 0234: Evaluation evidence and CLI finalization

- Status: Partially implemented
- Depends on: RFC 0044, RFC 0103, RFC 0104, RFC 0233
- Tracking issues: #55, #61
- Supersedes: RFC 0104's prohibition on partial internal containers

## Summary

Telora uses one recoverable evaluation/evidence graph for diagnostic tools. A
node in that graph is either known, failed with stable diagnostic identities,
unavailable, unknown, conflicted, or skipped because a dependency failed.
Failed nodes are evaluator-private: they are not Telora values or types and
cannot cross an ordinary module, codec, or Host value boundary.

The public commands consume this machinery with different finalization rules:

```text
show  = query(recoverable evidence graph)
check = best-effort evaluate(graph) |> strict Module finalization
run   = strict Main loading |> selected Entry lifecycle
```

`show` succeeding means that the query ran. `check` succeeding proves that the
selected module finalized to a complete ordinary Module value. `run` remains
fail-fast and additionally performs Entry scheduling.

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
Any ordinary publication of a reachable failed child is rejected.

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

`telora check <module>` evaluates the complete selected module with best-effort
recovery, then performs strict finalization:

- independent work after a recoverable failure may add diagnostics;
- warnings alone do not fail finalization;
- syntax, type, resolution, runtime, or reachable failed-node errors produce a
  nonzero exit and no Module value;
- `ok` is printed only when every required module is known and the selected
  module has a complete ordinary value;
- exported closures are already-computed values; invoking one later is a
  separate computation outside this check.

For identical source, dependencies, Host inputs, native implementations, and
resource conditions, successful `check` finalization and strict module loading
produce semantically equivalent Module values. Best-effort changes diagnostic
coverage, never the success judgment.

## `show`

`telora show` queries the graph directly and does not finalize it. It can return
unaffected facts adjacent to damaged syntax or failed evaluation. A successful
empty result is still a successful query. Every record states its authority or
recovery status; it must not turn an unavailable or failed value into `Any` and
present that approximation as authoritative.

## Acceptance criteria

1. `check` is nonzero for `fail!`, division by zero, Array out-of-range, type
   errors, syntax errors, and unavailable dependencies.
2. `check` succeeds for a complete module with warnings and prints `ok` only
   after strict finalization.
3. `show` retains unaffected facts for recoverable syntax and evaluation
   damage.
4. Multiple independent failures have deterministic, deduplicated diagnostics.
5. `array.map` continues healthy slots after recoverable callback failures.
6. shape-only operations remain usable internally when children failed;
   selecting a failed child propagates its diagnostic identity.
7. no partial value crosses module export, codec, debug-value, or ordinary Host
   boundaries.
8. strict `run` behavior and successful values remain unchanged.

## Implementation plan

1. make `check` consume the recoverable workspace and require strict
   finalization before reporting success;
2. retain private failed children at native continuation boundaries;
3. implement dependency rules for common Array operations and general
   structural publication blocking;
4. project node states and diagnostic references into `show` records;
5. update the language SSOT and CLI tutorial and cover all acceptance cases.

## Implementation status

The `check` finalization portion is implemented for #55. The CLI runs a silent
strict evaluation to obtain the authoritative success judgment, then consumes
the recoverable workspace for diagnostics and observations. This keeps `dbg!`
single-shot while making `fail!`, division by zero, out-of-range indexing, and
all other strict runtime failures nonzero. Error diagnostics or any incomplete
workspace module also prevent `ok`; warnings alone do not.

Private failed container children and their operation-specific propagation
rules remain the implementation work tracked by #61.
