# SPEC —— 开发规范正文（单一事实源）

本文件是 dsh-pet-indesktop-rs 及其协作体系（planner / 执行端）的**规范唯一权威
版本**。变更流程见「规范公告」一节；变更历史见本仓库 Discussions 的
Announcements 分类。

> 2026-10-07 起生效（自 planning-center README 迁入并公开）。本文件由
> planner 维护——planner 可直接提交规范文档，不受「不直接写主仓库代码」
> 限制。

## 1. SOFT SPEC（架构与协作红线）

1. **CI 红线**：GUI 代码与纯逻辑隔离——`physics` / `config` / 解码链不得
   依赖窗口层；`animation` 不得反向依赖 `window`；`PetWindow` 私有面冻结。
   由 `scripts/check-arch.sh` 三项检查 + CI 强制。
2. **接口先行**：跨轨依赖（如解码器 ← Frame 契约）以 issue 内契约为准；
   契约变更须发公告。
3. **issue 分工**：规划端建任务 issue（含背景与验收标准）；执行端领取时
   评论认领并简述计划，同时开执行 issue（标题 `exec: 简述 (#N)`），完成后
   在 commit / PR 里 `closes #N` 指回父任务；规划端不动执行端已领取的
   issue（以看板认领评论为准）。
4. **素材策略（2026-10-07 定稿）**：素材不入库、PNG 序列帧优先、MIT 音效
   入库；**内存占用 > 磁盘占用 / 包体**是资源类设计的基准约束。
5. **执行端反馈**：契约 / 验收标准的疑问走相关 issue 评论并 @ 用户；
   执行端 token 无 Discussions 写权限，Announcements 只读（Q&A / Ideas
   分类对执行端关闭，公告位隔离优先）。

## 2. 规范公告（Announcements）

- 规范的发布与变更**只从本仓库 Discussions 的 Announcements 分类广播**；
  各端动工前应确认已读最新公告。
- **变更流程**：先改本文件，再发一条编号公告（标题格式
  `公告 NNN · 标题（YYYY-MM-DD）`），说明改了什么、影响谁、何时生效。
- **幂等规则（2026-10-07 定稿）**：发公告前先列出 Announcements 现存
  讨论，同标题 / 同主题一律不重发；编号取现存最大 + 1（不靠记忆递增）；
  API 报错但服务端可能已建成功时，先查列表再决定是否重试。公告正文
  **不嵌入规范快照**，只写变更说明 + 指向本文件的链接，避免快照漂移。
- **公告位保护（2026-10-07 定稿）**：全体系共用 entropy356 单一账号，
  「仅维护者可发」对任何 token 都不构成技术隔离；真正的隔离靠执行端
  token 不含 Discussions 写权限（PAT 规格见 agent-bootstrap
  `agent/README.md`）。Announcements 发帖由 planner 独占。

## 3. 身份与禁区（双端协作）

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
