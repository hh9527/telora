# Remaining graph/descriptor boundaries

Investigation after the single-module arena unification, using the ordinary
ontology `@test/query` check. No compiler behavior or benchmark binary changed
during this investigation.

## Allocation evidence

Source profile: `/tmp/unified-module-type-graph-after.zst`, 3,701,547 allocation
calls, 117.69 MB peak heap. `heaptrack_print` exported allocation-weighted folded
stacks; `c++filt` demangled Rust symbols. Counts sum each matching stack once,
including all allocation functions beneath the named frame. The full stack total
was verified against the profile summary. Categories overlap and must not be
summed; allocation counts are not CPU time or expected speedup percentages.

| Frame or intersection | Allocation calls | Share of total |
| --- | ---: | ---: |
| `solve_program_types` | 1,533,810 | 41.44% |
| `TypeGraph::descriptor` | 866,039 | 23.40% |
| `collect_program_annotations` | 326,773 | 8.83% |
| Annotations intersecting descriptor conversion | 326,369 | 8.82% |
| `Compiler` methods | 236,574 | 6.39% |
| `ModuleInterface::qualified` | 101,376 | 2.74% |
| `WorkspaceSnapshot::build` | 18,329 | 0.50% |
| `solve_tool_expression_types` | 3,981 | 0.11% |
| `default_prelude_exports` | 576 | 0.02% |
| `select_import_interface` | 311 | 0.01% |

The last two rows exclude copies performed by callers before entering those
functions. They do not prove that import copying is cheap. Optimized/inlined
frames also limit source attribution. In particular, co-occurrence of the text
`ModuleInterface` and a Clone frame is not a reliable count of interface clones
and is deliberately omitted.

The descriptor conversion stacks with an explicit immediate
`analyze_program_with_bindings_observed` caller account for another 520,548
allocations. Optimization prevents attributing these to individual source lines
from this profile alone. Inspection identifies its three direct graph conversion
sites: concrete declaration publication, family-body publication and definition
contract publication in `types/dependency.rs`.

## Representation boundaries

The module already owns a flat `TypeGraph`, but `StaticAnnotationContext::elaborate`
immediately calls `graph.descriptor(root)`. Local annotations are then stored as
`HashMap<Location, TypeDescriptor>`, consumed by GenericInference, and converted
back into inference structure slots. Nearly all allocation traffic under local
annotation collection in this profile is beneath descriptor conversion.

Definition contracts similarly reconstruct a descriptor and distribute clones
to binding schemes, the static environment, binding types and definition
contracts. These descriptor inputs remain inside the main solver, so simply
moving graph ownership into a session cannot remove their allocation traffic.

`ModuleInterface` also still owns descriptor-bearing exports and concrete types,
plus recursively owned namespace interfaces. `qualified` rebuilds named type
trees for each alias. `load_resolved_value` clones a Ready artifact, and native
loading clones a stored module. Prelude selection clones a whole interface for
each exported binding before filtering it. Replacing only the final selection
function with a borrowed input would leave upstream clones; indiscriminately
borrowing it would also introduce new clones on currently owned/movable paths.

## Migration order

1. Keep declaration contracts and annotation roots as graph IDs through solver
   ingress. Add a graph-to-inference-slot handoff that preserves sharing and
   recursive nominal identities without reconstructing descriptor trees.
   Resolve scope-specific bound parameters explicitly; do not treat equally
   spelled local type parameters as one global parameter.
2. Move the corresponding solver consumers to slots/IDs, including declared-body
   lookup and contextual construction. Remove descriptor ingress for these paths
   rather than keeping it as a fallback. Existing explicit type errors, nominal
   recursion, generic constraints and diagnostic locations are acceptance gates.
3. Put module interfaces in a session table indexed by ModuleId; imports refer to
   the target and selected export. Keep local alias resolution separate from
   canonical type identity, so aliases do not require rebuilding type trees.
   Change artifact ownership and consumers together; an isolated borrowed
   `select_import_interface` wrapper is insufficient.
4. Continue replacing generated tool expression inference with consumption of
   the final static artifact. This remains an architectural requirement, although
   its visible allocation share is currently small on this workload.

This refines implementation priority; it does not relax the session-wide static
phase, VM exclusion or final typed-code requirements. Unknown/Conflicted graph
results and complete diagnostic collection remain required. No runtime changes,
new performance gains, or completed session boundary are claimed here.

Retained investigation artifacts: `/tmp/session-interface-allocators.log`,
`/tmp/session-interface-costs.awk`, `/tmp/session-interface-costs.log` and the
original compressed profile. The multi-gigabyte intermediate folded stack files
can be regenerated from that profile and are not required repository assets.
