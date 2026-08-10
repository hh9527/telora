# Four-AI ontology experiment run 20260810-161853

- Status: `completed-with-protocol-deviations`
- Source revision: `3c27d373d6319c09d1a0432a96694b89f333f5ff`
- Source worktree at start: dirty; the versioned experiment protocol was uncommitted
- Codex: `0.147.0`, inherited configuration
- Execution profile: `soft-reproducible-v1`
- Isolation grade: `instruction-isolated`
- Runner: built-in persistent sub-agents, Main-star topology
- Feedback relay: Main only
- Implicit interruption on human chat end or Escape: forbidden
- Registry: `agent-registry.yaml`
- Telora: `/home/h00629578/ws/xl/target/debug/telora`
- Final report: `final-report.md`
- Stage 4 evidence: `stage-4.md`

## Accepted artifacts

- Stage 2 round 1: `/tmp/telora-builtin-star-20260810/accepted/stage-2-r1/ontology-edsl`
  - deterministic file-list hash: `87fa182fb4ff173e287dbf0a16033e4ae022827d80474a40e33585e4406be07c`
- Stage 3 round 0: `/tmp/telora-builtin-star-20260810/accepted/stage-3-r0/enterprise-model`
  - deterministic file-list hash: `6c92f226675eaf579f2afae62c650fea1ba6c02747bf32357aec6c166336e800`
- Stage 4: `/tmp/telora-builtin-star-20260810/accepted/stage-4`
  - deterministic file-list hash: `f48ed710a115920fb7a901e0f08c303548ec3c6d685fad15b163450b201f0acc`
- Stage 4 public declarations: `PUBLIC_API.md`

The deterministic hashes above are SHA-256 over the sorted `sha256sum` file list for each accepted
artifact tree. Absolute paths are retained because the run artifacts are intentionally temporary.

## Workspace baselines

| Role/trial | Baseline |
|---|---|
| A2 | `506e515706030afb1865c7fde52aac46343e74b2` |
| A3 | `283781ebd428870eaf586091cda64072dd342fd7` |
| direct | `722d7fac2d0817c05e198c0e7b931746b3fb95a5` |
| novel | `06962a091832644506d9772e4aa2515a447bc689` |
| unapproved | `1e9c860d6ced4578a3a5c4e242cda59c1a15272d` |
| mixed | `72051340b06dfaef475ea10366cf29cced58f9ac` |
| fanout | `5c2bff02006ba994051d6fa384791b696c4564dd` |
| impossible | `a72dc2ed6a0e1e1460422fca05faa2513ce5193c` |

## Stage state

- Preflight: passed
- Stage 2: accepted after bounded correction round 1
- Stage 3: accepted at round 0
- Stage 4 direct: `lowered`, accepted
- Stage 4 novel: `lowered`, accepted
- Stage 4 unapproved: `model-rejected`, accepted
- Stage 4 mixed: `model-rejected`, accepted
- Stage 4 fanout: `model-rejected`, accepted after one bounded semantic correction
- Stage 4 impossible: `agent-refused`, accepted

## Deviations

- The preflight registry named `/root/stable_a4`. Built-in identities were not addressable after a
  Main-session transition. Human authorization allowed A4 to be rebuilt as
  `/root/stable_a4_stage4` for Stage 4 and rebuilt again when later Main sessions resumed. This
  violates the strict no-replacement registry rule and is recorded rather than hidden.
- Main repeatedly routed intended collaboration calls as no-effect `exec true` calls. Each affected
  trial workspace was checked unchanged before the real assignment or correction was delivered.
  These are controller orchestration mistakes, not runner, API, authentication, workspace, or
  model failures, and they consume no model correction round.
- During the Stage 4 pause, the versioned RFC was tightened to forbid every external, CLI,
  service-based, or substitute runner. The final Runbook also removes a stale sentence that
  contradicted the persistent sequential A4 protocol. These post-start method text changes mean
  the run supports a functional result with disclosed protocol deviations, not strict protocol
  conformance.
