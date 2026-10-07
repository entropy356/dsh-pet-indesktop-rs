# SPEC —— 开发规范正文（单一事实源）

本文件是 dsh-pet-indesktop-rs 及其协作体系（planner / 执行端）的**规范唯一权威
版本**。各端动工前重读本文件确认最新版；**变更历史以本文件的 git 提交记录
为准**。

> 2026-10-07 起生效（自 planning-center README 迁入并公开）。本文件由
> planner 维护——planner 可直接提交规范文档，不受「不直接写主仓库代码」
> 限制。

## 1. SOFT SPEC（架构与协作红线）

1. **CI 红线**：GUI 代码与纯逻辑隔离——`physics` / `config` / 解码链不得
   依赖窗口层；`animation` 不得反向依赖 `window`；`PetWindow` 私有面冻结。
   由 `scripts/check-arch.sh` 三项检查 + CI 强制。
2. **接口先行**：跨轨依赖（如解码器 ← Frame 契约）以 issue 内契约为准；
   契约变更须在契约所在 issue 内更新并评论提示，相关轨开工前复查契约。
3. **issue 分工**：规划端建任务 issue（含背景与验收标准）；执行端领取时
   评论认领并简述计划，同时开执行 issue（标题 `exec: 简述 (#N)`），完成后
   在 commit / PR 里 `closes #N` 指回父任务；规划端不动执行端已领取的
   issue（以看板认领评论为准）。
4. **素材策略（2026-10-07 定稿）**：素材不入库、PNG 序列帧优先、MIT 音效
   入库；**内存占用 > 磁盘占用 / 包体**是资源类设计的基准约束。
5. **执行端反馈**：契约 / 验收标准的疑问走相关 issue 评论并 @ 用户；
   执行端 token 无 Discussions 写权限——公共广播面收敛：主仓库
   Discussions 各分类对执行端关闭，一切反馈走 issue 评论。

## 2. 身份与禁区（双端协作）

- planner 与执行端在 GitHub 上共用 entropy356 账号（单一 admin），分离靠
  token 分级 + 约定：
  - **planner**：全量 PAT（四仓库读写 + Discussions 写）；
  - **执行端**：受限 fine-grained PAT，仅本仓库 Contents / Issues / PR
    三项 RW + Metadata 只读（规格与 age 交接流程见 agent-bootstrap
    `agent/README.md`）。
- **planner 禁区**：不动执行端已领取的 issue；不直接写主仓库代码
  （本文件 SPEC.md 的维护除外）。
- **执行端禁区**：不写 planning-center（含 PROGRESS.md）——token 层面
  强制（该私有仓库不在执行端令牌范围内）。
- **审计线**：planning-center 的 `PROGRESS.md`（只记决策、事故、规范
  变更、下一步；issue / PR 状态以本仓库看板为事实源）。
- **应急通道（2026-10-07 定稿）**：需要立即触达的事项走两条既有通道——
  用户直接在相关会话中喊话（各执行端均为用户面前的会话）；planner 在
  受影响的 issue 下评论冻结。**不设全局广播位**：Discussions 仅保留
  用户与 planner 的交流分类（Q&A / Ideas 等），Announcements 分类
  不承载任何流程。
