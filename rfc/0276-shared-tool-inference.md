# RFC 0276: Shared Tool Inference and Static Property Evidence

- Status: Implemented on local experimental branch `perf/shared-tool-inference`.
- Baseline: 58f0b8c, after RFC 0274.
- Related: RFC 0260, RFC 0274, RFC 0275.

## Motivation

RFC 0274 introduced constructor inference before each tool expression. Each
invocation copies module environments, decodes every tool binding and rebuilds
declaration evidence. A debug check of 400 independent struct declarations
increased from 3.08 s before RFC 0274 to 11.46 s. Instrumentation attributed
7.58 s to tool inference, including 362,085 type-decoding attempts.

The module already has an authoritative inference pass. Ordinary value bodies
should be checked there; only dependencies of metadata computations need early
tool evaluation. Type identities and declaration contracts are shared module
facts, not inputs that must be reconstructed for each expression.

## Design

Maintain declaration contracts and bodies as module-level inference inputs.
Publish newly established bindings incrementally. Tool expressions use only
their referenced value inputs, including lexical generic witnesses, while
sharing module contracts and declaration identity. Failed or incomplete local
inference must not contaminate later expressions or publish unresolved evidence.

Keep the full-module constraint pass authoritative. Retain complete constructor
evidence when compiling an already checked tool expression. Preserve checks
for unknown members, invalid generic arguments and nominal payload contracts.

Property providers cannot modify the type skeleton. Their return contracts
determine the property type. Register type-property evidence and its future
runtime binding from those contracts before module inference. Trait and
Property constraints can consume this static evidence without evaluating the
property value. Lexical evidence for generic parameters follows the same rule.

After static checking succeeds, evaluate property providers and materialize
their records. A failed provider, invalid capability or missing runtime record
prevents module publication; static evidence alone never authorizes publication.
Retain member-before-type evaluation, previous-property values and provenance.
Static type errors may now precede property-provider execution errors.

## Non-Goals

- Changing constructor, trait-selection or property applicability rules.
- Caching results solely by source location across different lexical scopes.
- Removing checks or treating incomplete metadata as successful evidence.
- Implementing RFC 0275 construction checks.
- Publishing, pushing or merging this local experiment.

## Verification

Use language cases for local and imported constructor providers, generic
shadowing, property-constrained traits, provider failures and diagnostics.
Run the workspace suite with a debug build. Compare identical declaration,
function and shared-type workloads against the preserved baseline executable.
Keep benchmark inputs and a reproducible runner with the change. Record actual
results and remaining architectural limitations before marking implemented.

The workspace debug test suite passes, including constructor tool evaluation,
generic lexical shadows and explicit type arguments, local/imported property
trait selection, static-error precedence and failed-provider publication.

Debug measurements on 2026-09-08, same toolchain and lockfile, one warmup and
three sequential samples per case (median wall seconds):

| Workload | 58f0b8c | Experiment | Speedup |
| --- | ---: | ---: | ---: |
| One Int constant | 1.203 | 0.802 | 1.50x |
| 400 Int functions | 4.550 | 2.201 | 2.07x |
| 400 independent structs | 11.231 | 2.863 | 3.92x |
| 400-element array | 1.296 | 0.874 | 1.48x |

Reproduce with `python3 scripts/measure-tool-inference.py BASELINE_BINARY
EXPERIMENT_BINARY --sizes 400 --samples 3`. The runner generates identical
temporary workspaces and performs no builds. These results measure the combined
optimization, not the isolated contribution of static property evidence.

## Remaining Limits

Tool expressions still have isolated inference state; this is shared input
preparation, not a single inference invocation for every phase. Partial and full
module analysis remain separate. Metadata-computing helpers still require tool
evaluation and local inference. Property planning retains the existing explicit
provider return-contract requirement. Shared type-DAG traversal is not globally
memoized. These changes do not establish a speedup for every workload.

## Follow-up: Scoped Type Environments

Strict inference and provisional type projection now borrow their parent
environment when entering closures, blocks and pattern branches. Local bindings
use a small Vec searched from the end; missing local type information explicitly
hides the outer binding instead of falling back to it. Rebinding changes only
the current scope, and dropping a scope leaves its parent unchanged.

The module environment remains a HashMap. Match joins that freshen inference
variables still construct a transformed environment: that operation changes
visible descriptors, unlike ordinary lexical scope entry. Large individual
local scopes may eventually need an index; the common small-scope path does
not allocate a hash table. Local scheme stacks are unchanged.

Unit tests cover borrowed descriptor identity, nested shadowing, removal and
reinsertion, and visiting each visible binding exactly once.

The workspace suite passes with 271 core tests and the language acceptance
fixtures. Release builds were compared with the existing benchmark runner,
`--sizes 400 1600 --samples 3`, after builds and tests finished. Median seconds:

| Workload | Before scoped environments | HashMap local scopes | Vec local scopes |
| --- | ---: | ---: | ---: |
| One Int constant | 0.146 | 0.134 | 0.133 |
| 400 Int functions | 0.399 | 0.226 | 0.230 |
| 1600 Int functions | 3.232 | 0.774 | 0.767 |
| 400 independent structs | 0.524 | 0.511 | 0.517 |
| 1600 independent structs | 3.868 | 3.850 | 3.870 |
| 1600-element array | 0.152 | 0.141 | 0.139 |

Eliminating full environment copies accounts for the substantial function
speedup. These samples do not show a clear additional speedup from Vec versus
HashMap local scopes. Independent type-declaration scaling remains unchanged;
dependency indexing and partial/full analysis reuse are outside this follow-up.

## Follow-up: Type Declaration Scaling

Tool evaluation now borrows module bindings and overlays generic parameters,
recursive family witnesses and provider arguments. Tool compilation returns
the actual external names needed by the lowered expression, including HIR
constructor references and hidden declared-owner links. Only those values enter
the VM external environment. Static inference still sees explicit type inputs.

HIR child indices replace whole-module scans for subtree dependencies, and
source-location lookup uses the existing sorted expression order. A single
iterative SCC decomposition identifies recursive type groups; the full module
schedules groups with dependency counts and a deterministic ready queue.
Partial analysis uses the same component classification and dependency order,
while retaining its existing recovery loop and failure propagation.

Verification includes all 512 three-node directed graphs, a 2048-node forward
chain, subtree-query equivalence including decorators, and the workspace suite
(274 core tests plus language acceptance fixtures). The benchmark runner adds
optional `--workloads forward-types repeated-family` to distinguish scheduling
costs from repeated applications of a memoized type family.

Release measurements against `b01c06e`, one warmup and three sequential samples
per workload, median wall seconds:

| Workload | Before | After |
| --- | ---: | ---: |
| 400 independent structs | 0.517 | 0.309 |
| 1600 independent structs | 3.804 | 0.957 |
| 1600 Int functions | 0.766 | 0.486 |
| 400 forward type aliases | 0.391 | 0.194 |
| 400 applications of Box(Int) | 0.440 | 0.252 |

The binding-only intermediate build measured 1.214 s for 1600 independent
structs, before the dependency/index changes. The main comparison used
`--sizes 400 1600 --samples 3`; the two additional workloads used `--sizes 400
--samples 3 --workloads forward-types repeated-family`. These improvements do
not imply that all inference work is now linear or globally shared.

### Duplicate Inference Audit

Interpreter memoization caches runtime results by function identity and
canonical argument TypeIds within the current heap. It does not cache tool
expression inference, bytecode preparation or environment construction.

Partial and full analysis still create separate ToolEvaluators. Within full
analysis, tool expressions receive constructor inference before execution, and
their ASTs can be checked again by authoritative module inference. Provisional
type projection also visits value bodies before strict inference. These are
remaining repeated computations, not eliminated by this follow-up.

The architectural target is one module constraint context, with tool queries
contributing evidence and recovery consuming the same analysis facts. It is
not sound to substitute a source-location-only cache: lexical witnesses,
expected types, substitutions and declaration completeness can differ between
visits. This change does not implement global constraint solving or claim to
remove those stage boundaries.

The solver itself already uses fresh inference variables, a substitution map,
bidirectional expected types and unification. Unknown callees receive a
function skeleton with fresh parameter and result variables. However, `resolve`
recursively follows substitutions and reconstructs many composite descriptors;
`occurs` invokes resolution again. A shared node-based solver could separate
shallow representative lookup from final descriptor materialization. Such a
change must preserve occurs checks, per-use generic instantiation, lexical
generalization boundaries, trait obligations and rejection of unresolved
inference variables at publication. It is not implemented by this optimization.
