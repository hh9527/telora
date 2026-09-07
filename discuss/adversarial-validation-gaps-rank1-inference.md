# Adversarial validation gaps in the rank-1 inference phase

- Stage: Discussion
- Audience: implementation agent (sous) and subsequent reviewers
- Scope: RFC 0070 through RFC 0083 (the rank-1 type-inference phase)

## Purpose

This document consolidates the adversarial-validation gaps identified during
panel review of the inference phase. It is not a bug report: nothing here is
known to be wrong. It is a checklist of *constructable programs and scenarios*
that the current test suite does not clearly exercise, grouped by the
mechanism each one targets. The common risk is not "tests fail" but "tests
pass while the semantics are silently wrong" — a failure mode the current
suite has only caught by accident (see below).

## Why this matters (the two weak spots in the current suite)

1. **Tests are co-authored with the semantics they verify.** The
   implementation agent writes both the checker and its tests, so the tests
   encode the same assumptions as the code. The strongest evidence: the RFC
   0072 regression (27 existing cases that the proposed immediate-error rule
   would have broken) was found by a *manual workspace audit*, not by any
   test. A suite that misses a 27-case regression in the very feature it was
   testing is not yet an adversarial suite.
2. **Count is thin relative to the surface.** The inference arc added roughly
   twenty net tests while reshaping checker core semantics five times. The
   acceptance criteria are covered; the *boundary conditions* of those same
   criteria are largely not.

A targeted constructed-case suite (this document) is the priority; general
fuzz/mutation/differential infrastructure is desirable later but is not
required to start closing the gaps below.

## Cross-cutting invariants (verify once, everywhere)

The following must hold in every published position. Each one deserves a
negative probe (a program that *would* leak if the invariant were broken):

- No `InferenceVariableId` reaches: `TypeGraph`, `WorkspaceTypeGraph`, module
  interfaces, CLI type output, LSP hover, binding facts, or expression facts.
- No numeric-domain marker reaches any published type (`numeric (Int or
  Float)` is a solver obligation, never a descriptor).
- No unresolved placeholder reaches any published fact or interface.
- Cancellation and stale revisions publish no partial substitution or
  provisional scheme.
- The same program yields identical descriptors and primary diagnostics
  regardless of hash-map iteration, cache state, cancellation timing, or
  query scheduling.
- Definitions display schemes (`for(A) Fn(A) -> A`); references and calls
  display monomorphic instances. The erased runtime type graph never replaces
  a separate exported scheme.

## Mechanism-specific adversarial cases

### 1. Directional checking / `Never` (RFC 0070)

- `Never` in nested positions: e.g. `choose(Array(stop()), [1])` and
  `choose([1], Array(stop()))` — a `Never` inside a structural shell must not
  solve the shell's element variable, while the reachable shell must.
- `Never` as a callback result: a callback that evaluates to `stop()` where a
  result is expected — no variable solved by it.
- The two directions stay distinct:
  - `check(Never, ?A)` succeeds without substitutions;
  - `check(?A, Never)` (explicitly required `Never`, e.g. a binding annotated
    `let x: Never = ...`) binds the variable.
- Reverse-direction acceptance is rejected: a resolved non-bottom actual is
  never accepted in the expected-to-actual direction.

### 2. Inference-variable hygiene (cross-cutting, highest priority)

- A variable surviving through a Struct field, a callback, an empty
  collection, and a delayed alias — the published fact must be concrete.
- `let y = x;` where `x` is a delayed binding: completion must check only
  variables owned by `y`'s initializer (the RFC 0073 ownership rule) and must
  not fail prematurely.
- Alias chains share one identity: `let a = f; let b = a; b(1); a("x")` must
  conflict (one monomorphic instance), never behave as two instances.

### 3. Numeric domains (RFC 0074)

- Domain merge conflict: `fn(a, b) { a + b }` with `a` receiving `Int`
  evidence and `b` receiving `Float` evidence must fail (no promotion, no
  union).
- Domain via a callback: `fn(f) { f(1) + 2.0 }` — the callback result domain.
- Domain leakage into generalization: a numeric-domain variable in an
  eligible closure (e.g. `let negate = fn(v) { -v }`) must NOT generalize as
  an unconstrained parameter. `negate[String]` must be rejected. This is the
  RFC 0079 soundness boundary.
- Domain never published: no `Int | Float` or `numeric` marker in hover,
  facts, or interfaces for an unresolved or resolved numeric expression.

### 4. Delayed monomorphic ownership (RFC 0073)

- Nested-block no-escape: returning an underconstrained closure from a nested
  block is an error at the block boundary unless a parent expected type
  solves it while the block is checked.
- Solving through a Struct field: a closure stored in a field, then called
  through `record.field(...)` — the same inference identity must solve.
- Completion timing of `empty[_]()` (RFC 0081): the documented relaxation is
  "program-analysis boundary, not block boundary." Verify a nested-block
  placeholder is caught, and that code *after* the failing block still
  receives diagnostics.

### 5. Generalization (RFC 0079)

- Free-variable escape: a closure capturing an outer delayed binding must not
  generalize that outer variable (it stays one monomorphic identity).
- Inner captures outer generic parameter: when a generalized closure contains
  a nested closure that uses the outer scheme's parameter, the inner bound
  identity must be distinct (no conflation of `TypeParameterId`s).
- Numeric-domain rejection path: the exact `negate` case above.
- Shadowing restore: `let f = fn(v) { v }; { let f = fn(v) { v + 1 }; }` —
  after the inner block, the outer `f` must still generalize.
- Generalize-then-apply-then-conflict: `let f = fn(v) { v }; f[Int](1);
  f[String]("x")` — both valid (independent instances); `f[Int]("x")` is an
  error.

### 6. Branch joins (RFC 0075)

- True arm-order independence for: distinct metadata witnesses (join to
  `Type`), `Never` mixes, contextual enums, and duplicates.
- Join purity: a join must not leave substitutions behind that a later
  conflicting branch observes.
- Incompatible contributors require an explicit common contract in either order.
- Pathological shapes: deeply nested enum payloads and collections retain
  deterministic diagnostics and terminating display.

### 7. Recursion / least fixed point (RFC 0078)

- Evidence-free recursion requires an explicit result contract:
  `def loop = fn(v) { loop(v) };`.
- Indirect self-reference that *looks* acyclic: recursion via an alias, via a
  capture, or via a `let` hop (`def a = fn(v) { b(v) }; let tmp = a; def b =
  fn(v) { tmp(v) };`). This is also the RFC 0083 misclassification risk.
- LFP termination: a recursive function whose skeleton is constrained by
  mixed base/recursive branches (`if v < 1 { 0 } else { f(v - 1) }`) must
  iterate to a stable `Int` solution without diverging or stalling.

### 8. Placeholder type application (RFC 0081)

- Two `_` arguments are independent: `pair[_, _](1, "x")`.
- A repeated bound occurrence shares one placeholder: `pair[Int, _]` where the
  parameter appears twice in the body.
- Placeholder on an inferred scheme uses semantic identity, not presentation
  name: `let pair = fn(l, r) { (l, r) }; pair[Int, _](1, "x")`.
- Placeholder hover/facts show a resolved type or an explicit incomplete/error state.
- Placeholder in a nested block cannot escape unresolved.

### 9. Context-complete generic calls (RFC 0082)

- The source-order fix: `choose(empty(), 1)` and `choose(1, empty())` produce
  identical results.
- Inner-call pending: a genuinely underconstrained outer call still fails (it
  is not deferred forever), while an inner result connected to an enclosing
  descriptor completes with it.
- Result-to-callback: `let value: String = recover(fn(item) { item });` — the
  callback parameter is `String` before the body is checked.
- A callback checked as an argument is never generalized; a generalized
  binding passed as a callback instantiates once.
- Empty structural values preserve context but provide no evidence.

### 10. Acyclic `def` components (RFC 0083 — in progress)

- Misclassification is the danger direction: a recursive component treated as
  acyclic would open a polymorphic-recursion soundness hole. Construct
  indirect recursion through a `let` hop, a capture, or a field access that a
  naive containment walk might misread as acyclic.
- Shadowing creates no false edge: same-spelled definitions in different
  scopes, and a shadowing parameter, must not connect components.
- Mixed edges: an acyclic node referencing a recursive component sees the
  completed monomorphic descriptor; a recursive node referencing an acyclic
  component sees an instantiated scheme.
- Forward-reference chains generalize regardless of source order
  (`apply` before `identity`).

## Migration-honesty check (the RFC 0073 lesson)

When a semantic change forces test fixtures to be rewritten, distinguish a
*legitimate migration* from a *masked regression*:

- For each fixture given an explicit contract, verify whether its original
  type relationships can be inferred. Preserve generic relationships when
  testing decorator contexts, callbacks and metadata/runtime helpers.
- Keep explicit Dyn packaging in fixtures whose purpose is dynamic inspection.
  Fixtures for ordinary generic functions should exercise their checked schemes.

## Minimum viable adversarial suite

A single test module per mechanism above, holding five to ten constructed
cases each, would close most of the identified gaps. The two highest-value
modules are:

1. inference-variable hygiene (no leak in any published position);
2. recursion misclassification (RFC 0078/0083 soundness boundary).

Priority order for the remainder: generalization free-variable boundary,
numeric-domain leakage, `Never` nested positions, placeholder completion,
branch-join purity, migration honesty.
