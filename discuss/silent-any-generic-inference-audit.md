# Silent `Any` paths in generic inference

- Status: completed implementation audit
- Scope: GitHub issue #9 after commit `a54f423`
- Audited implementation: `crates/telora-core/src/types.rs` and module import
  projection in `crates/telora-core/src/module.rs`

## Question

This audit asks whether generic inference can lose evidence by silently replacing an
inferred type with `Any`. It does not treat every occurrence of `Any` as a defect.
`Any` is also the declared representation of an erased Host boundary, an explicit
dynamic value, and several conservative recovery projections.

The relevant pipeline has three distinct layers:

```text
Host values and module interfaces
  -> shallow tool-stage projection and interpolation checks
  -> strict HIR resolution and generic constraint solving
  -> resolved facts, schemes, and an erased runtime TypeGraph
```

A fallback in one layer is a defect only if it silently replaces stronger evidence
owned by that layer or is published as if it were an authoritative strict fact.

## Classification

| Path | Reachability | Classification | Reason |
| --- | --- | --- | --- |
| `infer_value(Func) -> Fn(Any...) -> Any` | External value without an interface | Intentional boundary | A closure value exposes arity but carries no recoverable static parameter or result scheme. |
| Heterogeneous external Array item inference | External value without an interface | Intentional boundary | Structural value inspection retains the common shape and erases an item type when no common descriptor exists. |
| External value with `ModuleInterface` | Ordinary semantic import or declared Host interface | No fallback | The exported `TypeScheme` is authoritative and is instantiated by strict inference. Runtime descriptors are separately erased. |
| Root Host input marked dynamic and carrying no interface | Root invocation boundary | Intentional boundary | Its static descriptor is deliberately `Any`, and the compiler records the binding as dynamic. |
| Opaque runtime import with an interface | Recursive/up-link runtime boundary | No static fallback | The compiler treats its storage dynamically, while strict analysis can still retain and instantiate the separate semantic interface. Runtime dynamism does not discard that scheme. |
| Strict `ExprKind::Variable` missing from both scheme and descriptor environments | Not reachable from accepted source | Unreachable invariant | HIR resolution reports `unknown binding` before `GenericInference::infer` runs. The fallback is defensive for internal callers, not source-error recovery. |
| Shallow `infer_expr_with` missing variable, call, closure parameter, pattern input, or field | Tool-stage precheck | Recovery/conservative projection | These descriptors support interpolation checks and provisional expression coverage. On strict success, `GenericInference::records` overwrites the same expression locations. On unresolved source, HIR resolution fails instead of publishing a strict result. |
| Generic call receives an actual `Any` | Strict, when the input is already erased | Intentional propagation | Unsolved variables reachable from that parameter are explicitly erased because the call has crossed a boundary that supplied no static evidence. |
| Generic call receives a partial Tagged value | Strict | Intentional compatibility widening | A single Tagged constructor is partial evidence for an Enum. Remaining unsolved portions are erased after ordinary payload and argument constraints have run. Concrete conflicting evidence still fails before widening. |
| Calling a callee whose strict type is `Any` | Strict, only after an explicit or Host erasure | Intentional propagation | Arguments are still checked internally, but no callable contract exists from which a result can be derived. |
| Unresolved generic result without erased evidence | Strict | Strict failure | The solver reports `cannot infer generic result type` or `cannot infer monomorphic binding`; it does not default the result to `Any`. |
| Empty match result | Parser/recovered AST only | Unreachable or recovery invariant | Accepted source requires match arms. The descriptor prevents an internal empty fold from escaping. |
| `erase_type_variables` / `intern_erased_descriptor` | Runtime publication | Intentional boundary | Solver and bound variables cannot enter `TypeGraph` directly. Schemes are published separately; erasure is explicit and tested. |

Two other strict language operations can produce `Any` without participating in
generic anchoring: heterogeneous source collections widen their item type, and field
projection without a statically available Struct field is deferred to runtime. They
are existing gradual/runtime-check behavior, not evidence loss inside scheme
instantiation. Their consistency with the language-level statement that `Any` is
explicit is tracked separately by issue #13 rather than changed under issue #9.

## Evidence

The audit adds regressions that establish the boundary distinctions directly:

- the same external native closure is `Fn(Any) -> Any` without an interface;
- a generic identity interface for that closure instantiates to `Int` at `host(1)`;
- a Host-marked dynamic value is published as known `Any`;
- an unknown source name produces `unknown binding` before generic inference.

Existing regressions establish the remaining solver and import properties:

- imported generic schemes survive namespace, selective, aliased-selective, and open
  imports with the same five-parameter higher-order contract;
- imported private bound identities no longer collide with enclosing contracts after
  `a54f423`;
- underconstrained generic results fail, while explicit `Any` calls remain callable;
- solver descriptors panic if interned without explicit runtime erasure;
- exported definitions retain schemes separately from monomorphic call instances.

The strict solver records are merged after shallow facts, so successful strict records
win for every expression the solver visits. Published schemes are also validated to
reject inference variables and unbound parameter identities.

## Result

No independent post-`a54f423` generic anchoring failure was reproduced. The concrete
namespace/selective discrepancy was bound-identity leakage and is covered by the
four-form import regression. The remaining silent `Any` paths in the generic call
pipeline are either downstream of an already erased boundary, recovery-only, or
unreachable from accepted source.

Issue #9 therefore does not justify a new inference rule or RFC. It can be closed once
this audit and its regressions are accepted. A future report should reopen the topic
only with a strict, post-fix program where stronger scheme or argument evidence exists
before the result becomes `Any`.
