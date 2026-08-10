# Ontology eDSL tutorial

Import the stable surface with:

```telora
import "ontology-edsl/lib.telora" {
    CompilationEvidence, compile_capabilities,
    classify_paths, report_relationship_errors, compile_analytics,
};
```

## 1. Keep enterprise types closed

Define enum types for measure, dimension, and node vocabularies and concrete records for lowering
outputs, edges, mappings, combined measures, and plans. The library never asks you to convert them
to `String`, `Any`, or `Dyn`.

The metadata functions in `definitions.telora` are optional conveniences. For example,
`CapabilityDefinition(Id, Input, Output)` creates a record type with `id` and `lower`. In practice,
an enterprise often adds fields and defines its own record. Both approaches work because shared
rules consume typed selectors rather than requiring a nominal library-owned record.

## 2. Compile independent capabilities

Define a concrete evidence type, normally with `CompilationEvidence(Output)`, and a typed builder:

```telora
type SalesEvidence = CompilationEvidence(SalesOutput);
def build_sales_evidence:
    Fn(Array(Option(SalesOutput)), Array(SalesOutput), Option(Array(SalesOutput)))
        -> SalesEvidence =
    fn(results, values, complete) { {results, values, complete} };
```

Call `compile_capabilities` with requested ids, the catalog, an id selector, a lower adapter, one
shared input, and the builder. `results` stays positionally aligned with requests. `values` contains
all successes. `complete` is `Some` only when every request succeeded. A missing catalog item emits
an error against the authored requested id; lowerers should emit their own domain diagnostic and
return `None`.

Never use `values` as publication proof. It exists so independent later diagnostics can still run.

## 3. Classify relationships

Keep mapping payloads on your edge type; provide only `edge_from` and `edge_to` selectors. Pass
many-to-one or otherwise grain-preserving edges as `safe_edges`, and expanding edges as
`fanout_edges`.

`classify_paths(base, required, safe_edges, fanout_edges, edge_from, edge_to, build)` supplies the
builder with:

- safe edges lying on a safe route from the base toward a required target;
- targets reachable only when fan-out edges are allowed;
- targets unreachable even when both catalogs are allowed.

Each edge belongs in exactly one catalog. The reachability implementation expands eight times, so
it is suitable only when every relevant path has at most eight edges. It is deliberately not
described as an unbounded graph algorithm.

Call `report_relationship_errors` with authored requirements, a required-node selector, and a
diagnostic-subject selector. Dimension requests are typical requirements. It emits specific
fan-out and missing-path messages and returns whether all represented requirements are valid.

## 4. Use the shared analytics compiler

`compile_analytics` owns the stage order. Its arguments are grouped as follows:

1. measure request/catalog/selectors/lowerer/input, evidence builder, and evidence accessors;
2. the equivalent dimension group;
3. measure combination, base-node and required-node selectors, plus extra nodes;
4. relationship catalogs/selectors and authored requirement adapters;
5. path evidence builder/accessors and the final plan builder.

Telora has no traits or associated type families, so these forwarding adapters are intentionally
explicit. Give every adapter a full `Fn` signature; doing so also resolves empty-array and generic
callback inference.

The combination policy receives all successful measure outputs. This allows downstream diagnostics
to run even when another measure failed. The final builder likewise receives successful dimension
outputs and may run before completeness is known. Publication is still atomic: the returned value
is `Some(Plan)` only when both capability evidence values are complete, combination succeeded,
relationships are valid, and the builder returned a candidate.

Required nodes should include combined-measure nodes, completed dimension nodes, and filter or
other extra nodes. Put every authored dimension-like request into the `requirements` array so its
diagnostic subject is retained. See `probe.telora` for small concrete capability and graph adapters.
