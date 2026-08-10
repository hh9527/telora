# 新 Main 会话 Handoff

请从此处恢复并完成四角色 Telora ontology 实验。不要重新分析或重跑已经接受的阶段。

## 目标

持续完成 run `20260810-161853` 的 Stage 4 六个固定 A4 trial，执行 Host 验证，使用中文更新
GitHub Issue #8，并产出最终实验记录。

Issue: <https://github.com/hh9527/telora/issues/8>

## 拓扑与硬规则

- Main 是当前主 agent。
- A2、A3、A4 是内置持久 sub-agent。
- A2、A3、A4 不直接通信；Main 是唯一 Feedback 转发者。
- 人与 Main 的聊天结束或按 Escape 不授权 `interrupt_agent`。
- 不使用任何外部、CLI、服务型或新替代 runner；正式 run 仅复用已注册的内置身份。
- 不要重新创建 A2/A3。
- 不要重跑 Stage 2 或 Stage 3。
- 遇到长任务时用 `list_agents`、工作区只读检查和 agent 消息观察；不要发送探测任务。
- 每个实质进展、验证结果或 blocker 都用中文追加到 Issue #8；同时更新主贴 task list。

## 固定身份

- A2: `/root/stable_a2`，已完成，保持空闲。
- A3: `/root/stable_a3`，已完成，保持空闲。
- A4: `/root/stable_a4`，只有 `READY` bootstrap 历史，尚未收到任何实验任务。

Registry:
`target/exp-recs/20260810-161853/agent-registry.yaml`

## 已接受结果

### Stage 2

- 状态：接受，经过 bounded correction round 1。
- 候选：`/tmp/telora-builtin-star-20260810/accepted/stage-2-r1/ontology-edsl`
- 5 个源码模块和 2 个 probe 全部通过 `bin/telora check`。
- `probe.telora` run 成功，输出 complete capability evidence。
- `analytics_probe.telora` run 成功，输出：
  `'Some({dimensions: 1, relationships: 1, total: 101})`

### Stage 3

- 状态：接受，round 0。
- 候选：`/tmp/telora-builtin-star-20260810/accepted/stage-3-r0/enterprise-model`
- 三个 Telora 文件全部通过 check。
- `valid.telora` run 成功，生成 Order-grain、read-only 且保留安全关系 mapping 的计划。
- `invalid.telora` 被 unavailable capability 正确拒绝。
- 当前 runner 在首个 fatal diagnostic 处结束，因此不能声称同一次 run 展示了后续 fan-out 诊断。
- 公共 API stub：`target/exp-recs/20260810-161853/PUBLIC_API.md`
- `intent-tutorial.md` 使用概念名 `AnalyticsIntent`，实际公共类型名是 `Intent`；PUBLIC_API 已明确实际声明。

Manifest:
`target/exp-recs/20260810-161853/manifest.md`

## Stage 4 固定 corpus

来源：`experiments/four-ai-ontology/stage4-trials.yaml`

顺序必须是：

1. `direct`，预期 Host 分类 `lowered`
2. `novel`，预期 Host 分类 `lowered`
3. `unapproved`，预期 Host 分类 `model-rejected`
4. `mixed`，预期 Host 分类 `model-rejected`
5. `fanout`，预期 Host 分类 `model-rejected`
6. `impossible`，预期 Host 分类 `agent-refused`

不得把预期分类发送给 A4。A4 顺序复用同一个身份历史，每个 trial 使用新的独立 Git workspace。

请求文件已固定在：
`target/exp-recs/20260810-161853/stage4-requests/`

## 当前精确恢复点

`direct` workspace 已由固定 preparer 创建：

`/tmp/telora-builtin-star-20260810/a4/run-20260810-161853-direct`

Git baseline:

`722d7fac2d0817c05e198c0e7b931746b3fb95a5`

该 workspace 未污染。A4 尚未收到 assignment，Stage 4 尚未实际启动。

## 新会话第一动作

不要先运行 `exec`，不要检查 daemon，不要写计划。第一条工具调用必须是对现有 A4 的
`followup_task`，目标 `/root/stable_a4`，消息如下：

```text
Formal run 20260810-161853, Stage 4 trial direct. Work exclusively in
/tmp/telora-builtin-star-20260810/a4/run-20260810-161853-direct and remain inside that workspace.
Begin the assigned experiment role. Read requirement/ROLE.md completely and follow it exactly.
Read every staged requirement file. Modify only crates/intent/intent.telora and
crates/intent/NOTES.md. Do not run programs. Work until the required delivery is complete or a
genuine blocker is reached. Report completion or the blocker concisely; do not paste source.
```

调用成功后，确认 A4 为 running，并向 Issue #8 写中文 comment，说明 `direct` 已真实启动。

## 每个 A4 trial 的处理

1. 等待同一 A4 完成，不调用 `interrupt_agent`。
2. Host 检查仅改动 `crates/intent/intent.telora` 和 `crates/intent/NOTES.md`。
3. 对 expressible trial，Host 在独立验证目录中结合冻结 enterprise model 检查并运行 intent。
4. 对 `impossible`，验证明确 refusal、未发明标识符、没有物理计划或 SQL。
5. 记录 Telora diagnostic 到修正的因果链；若需反馈，由 Main 写唯一 bounded feedback 文件并转发。
6. 使用 `prepare-workspace.sh ai4` 创建下一个全新 trial workspace，然后对同一
   `/root/stable_a4` 调用 `followup_task`。
7. 更新 Issue comment 和主贴 task list。

创建后续 workspace 的固定命令形状：

```text
experiments/four-ai-ontology/prepare-workspace.sh ai4 \
  /tmp/telora-builtin-star-20260810/a4/run-20260810-161853-TRIAL \
  /tmp/telora-builtin-star-20260810/accepted/stage-3-r0/enterprise-model/PUBLIC_INTENT.md \
  /home/h00629578/ws/xl/target/exp-recs/20260810-161853/PUBLIC_API.md \
  /home/h00629578/ws/xl/target/exp-recs/20260810-161853/stage4-requests/TRIAL.md
```

## 先前 Main 的失误

先前 Main 多次应调用 `followup_task` 却错误调用了无意义的 `exec`。这不是 sub-agent、API、
认证或 workspace 故障。Issue #8 已更正归因。新会话不要复制该行为；第一步直接执行上述
`followup_task`。
