# Stage 4 Clean Handoff

继续 run `20260810-161853`，完成 Stage 4 和最终实验记录。

## 第一动作

第一条工具调用必须是 `spawn_agent`：

- `task_name`: `stable_a4_stage4`
- `fork_turns`: `none`
- message 使用下方文本，逐字发送。

```text
Formal run 20260810-161853, Stage 4 trial direct. Work exclusively in
/tmp/telora-builtin-star-20260810/a4/run-20260810-161853-direct and remain inside that workspace.
Begin the assigned experiment role. Read requirement/ROLE.md completely and follow it exactly.
Read every staged requirement file. Modify only crates/intent/intent.telora and
crates/intent/NOTES.md. Do not run programs. Work until the required delivery is complete or a
genuine blocker is reached. Report completion or the blocker concisely; do not paste source.
```

用户已明确授权重建 A4。新建身份是 Stage 4 的固定 A4；六个 trial 顺序复用该身份。

## 已冻结状态

- Stage 2 已接受，不重跑。
- Stage 3 已接受，不重跑。
- Enterprise candidate:
  `/tmp/telora-builtin-star-20260810/accepted/stage-3-r0/enterprise-model`
- Public API:
  `/home/h00629578/ws/xl/target/exp-recs/20260810-161853/PUBLIC_API.md`
- `direct` workspace:
  `/tmp/telora-builtin-star-20260810/a4/run-20260810-161853-direct`
- `direct` baseline:
  `722d7fac2d0817c05e198c0e7b931746b3fb95a5`
- Issue: `hh9527/telora#8`

## Stage 4 顺序

1. `direct`
2. `novel`
3. `unapproved`
4. `mixed`
5. `fanout`
6. `impossible`

预期 Host 分类仅供 Main 验证，不发送给 A4：

1. `direct`: `lowered`
2. `novel`: `lowered`
3. `unapproved`: `model-rejected`
4. `mixed`: `model-rejected`
5. `fanout`: `model-rejected`
6. `impossible`: `agent-refused`

请求文件位于：
`/home/h00629578/ws/xl/target/exp-recs/20260810-161853/stage4-requests/`

## 每个 trial

1. 等待 A4 完成交付。
2. 审计改动仅限 `crates/intent/intent.telora` 和 `crates/intent/NOTES.md`。
3. Expressible trial 在独立 Host 验证目录中结合冻结 enterprise model 执行 check/run。
4. `impossible` 验证明确拒绝，且没有发明标识符、物理计划或 SQL。
5. 用固定 preparer 创建下一个全新 workspace。
6. 对同一 A4 使用 `followup_task` 发送下一 trial assignment。
7. 每个启动、交付、验证、修正或 blocker 都用中文更新 Issue #8。

后续 workspace 命令：

```text
experiments/four-ai-ontology/prepare-workspace.sh ai4 \
  /tmp/telora-builtin-star-20260810/a4/run-20260810-161853-TRIAL \
  /tmp/telora-builtin-star-20260810/accepted/stage-3-r0/enterprise-model/PUBLIC_INTENT.md \
  /home/h00629578/ws/xl/target/exp-recs/20260810-161853/PUBLIC_API.md \
  /home/h00629578/ws/xl/target/exp-recs/20260810-161853/stage4-requests/TRIAL.md
```

完整背景仅在启动成功后按需查阅原 `HANDOFF.md`、`manifest.md` 和版本化 runbook。
