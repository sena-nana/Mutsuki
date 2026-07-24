# 方案：`@lilia/theme` 为唯一视觉标准，`@lilia/ui = theme + Vue`

> 状态：待执行方案（跨仓库）。本文件是 BotTemplate 作为「能力边界核查入口」记录的跨仓验收方案，
> 不在此改任何业务代码。实现动作发生在 owner 仓库（LiliaUI → MutsukiWebHost → MutsukiBotPlugins），
> Template 只负责最终 pin 与验收。

## 0. 现状与问题（调研结论）

- **LiliaUI（权威）**：yarn monorepo，remote `github:sena-nana/LiliaUI`，当前 HEAD `c28209b`，未发 npm，版本 `0.1.0`。
  包：`@lilia/ui-contract` → `@lilia/ui-foundation` → `@lilia/ui`，外加 `@lilia/config` / `@lilia/tools` / `@lilia/build` / `tauri-plugin-lilia`。
  远端消费按 `github:sena-nana/LiliaUI#workspace=@lilia/xxx&commit=<sha>` 固定同一 commit。
- **视觉事实源现在在 `@lilia/ui` 内**：`packages/ui/src/styles.css`（字体 + reset + 表单控件 + 工具类，`@import` tokens 与 state-layer）
  以及 `styles/{tokens,state-layer,workspace,sidebar,page,app-shell,global-scrollbar}.css`。
  `@lilia/ui` 的 `exports` 已经把这些 CSS 以 `./styles.css`、`./styles/tokens.css` … 以及别名 `./theme/base.css` 暴露出去。
- **Mutsuki 现在是「复制」而非「依赖」**：
  - `MutsukiWebHost/packages/ui`（`@mutsuki/ui`）**不依赖 `@lilia/ui`**。`scripts/sync-lilia-styles.mjs` 从兄弟 checkout
    `../LiliaUI` 把 `tokens/state-layer/workspace/sidebar/page.css` **物理拷贝**进 `@mutsuki/ui/src/styles`，
    再叠加自有 `console.css`（产品 chrome），`bundle-css.mjs` 拼成 `dist/mutsuki-ui.css`。
  - `MutsukiBotPlugins/scripts/sync-mutsuki-ui-css.sh` 把上面 `dist/mutsuki-ui.css` 再拷进 3 个 crate 的 `assets/mutsuki-ui.css`，
    由 Rust `include_str!` 编进产物。
- **核心问题**：tokens 存在三份物理副本（LiliaUI 源 → `@mutsuki/ui` src → BotPlugins assets），且 `@mutsuki/ui` 依赖兄弟目录而非 **pinned rev**，
  违反「唯一视觉事实源」和「跨仓 Git 依赖固定 rev、禁止仓库外 path」。字体（`/fonts/*.woff2`，由 `styles.css` 的 `@font-face` 引用）当前在 `@mutsuki/ui` 侧被 `console.css` 重新声明 `html/body/font`，字体资源无人统一提供。

目标：把视觉事实源收敛为**唯一**的 `@lilia/theme`（纯 CSS + 极薄框架无关 runtime，Vue-free），`@lilia/ui` 变为 `theme + Vue`，
Mutsuki 侧从「拷贝 CSS」改为「依赖 pinned `@lilia/theme`，构建期打进 dist」。

---

## 1. 包边界与命名

**新增包：`@lilia/theme`（在 LiliaUI monorepo 内，`packages/theme`）。**

- 定位：LiliaUI **唯一**视觉事实源。所有 design token、state layer、reset、字体契约、基础布局 CSS（workspace/sidebar/page/app-shell/scrollbar）与一个框架无关的主题应用 helper 都在这里。
- 为什么叫 `theme` 而不是 `tokens` / `ui-theme` / 第二视觉层：
  - 它不仅是 token 变量，还包含 reset、表单控件基线、state-layer 语义、布局骨架 CSS，是「界面如何呈现」的完整视觉基座（对齐 `architecture.md` 中 Theme = CSS token 表达的定义）。
  - 它**不是**第二套视觉：`@lilia/ui` 不再拥有任何 tokens/样式源，只 `@import` / re-export `@lilia/theme`。全仓仍只有一份视觉真源，`@lilia/ui` = 这份真源 + Vue 组件/Shell/Provider。
  - 命名与既有 `./theme/base.css` 别名、`@lilia/ui-foundation` 的 `./theme`（useThemeSync）语义一致，迁移心智负担最小。
- 依赖方向（在既有链上插入 theme，位于最底层视觉基座）：

```text
@lilia/ui-contract
        ↑
@lilia/ui-foundation      @lilia/theme   (无 Vue、无 Tauri，纯 CSS + DOM helper)
        ↑                      ↑
              @lilia/ui  ──────┘
                  ↑
             application
```

  - `@lilia/theme` 不依赖 Vue、Tauri、contract、foundation；可被任意（含无 Vue）消费方单独依赖。
  - `@lilia/ui` 依赖 `@lilia/theme`（生产依赖）并 re-export 其 CSS 与 helper。

---

## 2. 从 `@lilia/ui` 迁出到 `@lilia/theme` 的文件清单

**移动（物理迁移，`@lilia/ui` 不留副本）：**

| 源（`packages/ui/src/…`） | 目标（`packages/theme/src/…`） | 说明 |
| --- | --- | --- |
| `styles.css` | `base.css` | 主题入口：`@font-face` + `:root` 字体设置 + reset + 表单控件基线 + 工具类（`.muted/.ok/.warn/.err/.sr-only/.is-spinning` 等）+ `lilia-spin` keyframes。内部 `@import "./tokens.css"; @import "./state-layer.css";` 保留。 |
| `styles/tokens.css` | `styles/tokens.css` | 唯一 token 源（light/dark、oklch、radius、shadow、surface、backdrop）。 |
| `styles/state-layer.css` | `styles/state-layer.css` | 交互 state layer 与 surface mode。 |
| `styles/workspace.css` | `styles/workspace.css` | Workspace + Region 布局与折叠动画。 |
| `styles/sidebar.css` | `styles/sidebar.css` | `.secondary-panel` 侧栏 frame 基础。 |
| `styles/page.css` | `styles/page.css` | `.page-header`/`.card`/`.kv` 页面基础。 |
| `styles/app-shell.css` | `styles/app-shell.css` | AppShell chrome 基础样式。 |
| `styles/global-scrollbar.css` | `styles/global-scrollbar.css` | 全局滚动条。 |

**留在 `@lilia/ui`（组件私有样式，不属视觉基座）：**
`components/action-menu.css`、`components/search-dropdown.css`、`components/popup-titlebar-frame.css`、`layouts/popup-shell.css`
以及所有 SFC scoped style。它们由组件自身导入，随 `@lilia/ui` 分发；如需公开消费仍走 `@lilia/ui` 的细粒度 CSS export。

**字体处理（明确写死）：**
- `@font-face`（Noto Sans SC 400/500/600/700）定义随 `base.css` 进入 `@lilia/theme`，仍以 **`/fonts/<name>.woff2` 的 host-served 路径契约**引用（不 inline、不改路径）。
- **woff2 二进制资源不进 `@lilia/theme`**，仍由 `@lilia/tools` 作为默认资源提供，消费仓库通过工具复制到自己的 `public/fonts`（对齐 DESIGN.md「默认资源由 `@lilia/tools` 提供，消费仓库不维护第二份源」）。
- `@lilia/theme` 额外提供 `./styles/fonts.css` 子入口（仅 `@font-face`），方便无 Vue 宿主单独控制字体加载时机；`base.css` 默认已含字体声明。
- 消费方契约：使用 `base.css` 的宿主**必须在 `/fonts` 提供 woff2**，否则回退到 `--font-sans` 后续系统字体（不报错、不阻断）。

---

## 3. `@lilia/ui` 如何变成 `theme + Vue`

- `packages/ui/package.json`：
  - 新增 `"dependencies": { "@lilia/theme": "workspace:*" }`（远端消费时为 pinned git workspace，见 §4）。
  - 保留现有 peers（`vue`、`vue-router`、`@lucide/vue`、contract、foundation）与 `@tauri-apps/api` dep。
- **避免双份 CSS**（关键）：`@lilia/ui` 不再有 `src/styles/` 视觉基座文件。改为**薄转发**，二选一并**全仓统一**：
  - 方案（写死采用）：`exports` 里把视觉 CSS 子路径直接映射到 `@lilia/theme` 的文件，保持向后兼容路径不变：

```jsonc
// packages/ui/package.json exports 片段（改写后）
".": "./src/index.ts",
"./styles.css": "@lilia/theme/base.css",
"./theme/base.css": "@lilia/theme/base.css",
"./styles/tokens.css": "@lilia/theme/styles/tokens.css",
"./styles/state-layer.css": "@lilia/theme/styles/state-layer.css",
"./styles/workspace.css": "@lilia/theme/styles/workspace.css",
"./styles/sidebar.css": "@lilia/theme/styles/sidebar.css",
"./styles/page.css": "@lilia/theme/styles/page.css",
"./styles/global-scrollbar.css": "@lilia/theme/styles/global-scrollbar.css"
// 组件级 CSS（action-menu / search-dropdown / popup-*）仍指向 @lilia/ui 自己的文件
```

  - 若打包器对 `exports` 目标写包名支持不佳，则退化为**一行 `@import`** 的转发文件（如 `src/styles.css` 内容仅 `@import "@lilia/theme/base.css";`），**绝不 `@import` 后再复制内容**。
  - 无论哪种，`@lilia/theme` 的 CSS 在最终 bundle 中**只出现一次**：`@lilia/ui` 的 SFC/组件 CSS 不得再重复声明 token 或基座规则。
- Vue 侧不变：`@lilia/ui` 继续拥有 Contract 基础组件、`shell`/`settings`/`commands`/`overlay`/`layouts`/`provider`/`runtime`（含 Tauri adapter）/`diagnostics`。这些属于「theme + Vue」的 Vue 部分。
- 主题应用 helper：把当前散落的 DOM 主题应用逻辑收敛为 `@lilia/theme` 导出的**框架无关** `applyTheme(theme, opts)`（写 `data-theme` / `data-corners` / `data-lilia-surface-mode` 等，无 Vue 依赖）。`@lilia/ui-foundation` 的 `useThemeSync`（Vue）在其之上封装，不再各写一份 DOM 操作。

---

## 4. 发布与 semver

- **同 monorepo、同消费模型**：`@lilia/theme` 与其它 `@lilia/*` 同仓，远端消费统一用
  `github:sena-nana/LiliaUI#workspace=@lilia/theme&commit=<sha>`，并与本批次其它 `@lilia/*` **锁到同一个 commit SHA**（扩展 `release.md` 的同-commit 清单，把 `@lilia/theme` 加入）。
- **独立包版本，共享兼容队列**（对齐 `release.md`）：
  - theme 内部新增兼容 token/规则 → theme `minor`；消费该能力的 `@lilia/ui` 至少 `minor`。
  - 删除/重命名稳定 token、收紧语义 → theme `breaking`，`@lilia/ui` 同批 `breaking`，走弃用 + 迁移 + Changelog（`Lilia Layer` / 新增 `Theme` 分类）。
  - 纯 theme 视觉能力变化 → theme `minor`，`@lilia/ui` 视是否消费决定是否同批。
- `@lilia/ui` 对 `@lilia/theme` 声明兼容范围（`^0.x`）；改实现必须同步范围与消费 fixture。
- 发布门禁沿用 `release.md`：全 workspace typecheck/test、Contract/exports/依赖方向、视觉/性能证据、同-commit 安装验证、Changelog 覆盖。

---

## 5. Mutsuki 迁移路径

**目标：去掉 `sync-lilia-styles.mjs`，`@mutsuki/ui` 直接依赖 pinned `@lilia/theme`，构建期打进 dist；BotPlugins 继续物化 dist 产物。**

### 5.1 `@mutsuki/ui`（MutsukiWebHost/packages/ui）
- `package.json`：
  - 新增 `"dependencies": { "@lilia/theme": "github:sena-nana/LiliaUI#workspace=@lilia/theme&commit=<sha>" }`（pinned；lockfile 同提交）。
  - 删除 `scripts.sync:lilia`。
  - `peerDependencies.vue` 保留（`@mutsuki/ui` 仍导出 `ConsoleShell` Vue 组件），但 CSS 消费方无需 Vue —— theme 是 Vue-free，纯 CSS 消费方（Web Console / 未来 `@mutsuki/ui/styles.css`）只吃 CSS。
- 删除拷贝来的 `src/styles/{tokens,state-layer,workspace,sidebar,page}.css`（不再本地留副本）。
- 保留 `src/styles/console.css`（产品 chrome，属 Mutsuki 自有，不进 LiliaUI）与 `src/styles/index.css`；`index.css` 改为从包说明符导入：

```css
@import "@lilia/theme/base.css";        /* 含 tokens + state-layer + reset + 字体契约 */
@import "@lilia/theme/styles/workspace.css";
@import "@lilia/theme/styles/sidebar.css";
@import "@lilia/theme/styles/page.css";
@import "./console.css";                 /* Mutsuki 产品 chrome */
```

- `scripts/bundle-css.mjs`：改为**从 node_modules 解析 `@lilia/theme`** 的 CSS（`require.resolve`/`import.meta.resolve`）并按顺序拼接，再接 `console.css`，输出 `dist/mutsuki-ui.css`。这是**构建期一次性打入**，不是持续 sync；源仍是 pinned 包。
- 字体：Web Console 宿主需在 `/fonts` 提供 woff2（由 `@lilia/tools` 复制或 WebHost 静态资源提供）；`console.css` 不再需要重复 `font-family`（继承 `base.css` `:root`）。

### 5.2 BotPlugins（MutsukiBotPlugins）
- **物化策略不变**（这是把已构建 dist 编进 Rust 的合法产物固化，不是源分叉）：`scripts/sync-mutsuki-ui-css.sh` 继续把 `MutsukiWebHost/packages/ui/dist/mutsuki-ui.css` 拷进 3 个 crate 的 `assets/mutsuki-ui.css`，`include_str!` 照旧。
- 语义升级：该 `mutsuki-ui.css` 现在**可追溯到 pinned `@lilia/theme` commit**（单一事实源经 `@mutsuki/ui` 构建产出），不再是「拷 LiliaUI 源 CSS」的二次分叉。legacy `lilia-tokens.css` 分叉已被脚本删除，保持删除。
- 可选加固：脚本在拷贝前校验 dist 存在且 `@mutsuki/ui` 已按 pinned 依赖构建（现有 `missing $SRC` 检查已覆盖）。

### 5.3 与现有 sync 的关系（一句话）
- **删除**：`MutsukiWebHost/scripts/sync-lilia-styles.mjs`（源码级 CSS 拷贝，被 pinned 依赖 + 构建期打包取代）。
- **保留**：`MutsukiBotPlugins/scripts/sync-mutsuki-ui-css.sh`（dist 产物 → Rust 嵌入的物化步骤，属正常构建物固化）。

---

## 6. 分阶段落地与验收标准

> 顺序遵守 Hard Rule：能力先在 owner 仓库补齐、验证、推送，再更新下游 pin。

**Phase A — LiliaUI 抽取 `@lilia/theme`（owner，权威）**
- 动作：建 `packages/theme`，迁 §2 文件；`@lilia/ui` 改为依赖 + re-export（§3）；补 `applyTheme` helper 与 `check:ui-boundaries` 中 theme 的依赖方向规则；`release.md` 同-commit 清单加入 theme。
- 验收：`yarn typecheck`、`yarn test`、`yarn check:ui-boundaries`、`yarn check:contract-fixtures`、`yarn test:ui:visual`（token/布局视觉零回归）、`yarn perf:components:light` 无回归；`public-api.md` 的 CSS bundle 预算（42,755 / 42,890 bytes、4 async chunk）不放宽；`@lilia/ui` 内不再存在第二份 tokens/基座 CSS（grep 验证）。

**Phase B — 发布 / pin（owner）**
- 动作：提交并 `push` LiliaUI；记录 commit `<sha>`；同批次所有 `@lilia/*`（含 theme）指向同一 `<sha>`；Changelog 记新增 `@lilia/theme` 与 `@lilia/ui` 消费。
- 验收：在**没有兄弟仓库的独立 checkout** 用 `github:...#workspace=@lilia/theme&commit=<sha>` 能被独立解析安装（`@lilia/theme` 不拉 Vue/Tauri）。

**Phase C — WebHost（`@mutsuki/ui`）**
- 动作：加 pinned `@lilia/theme` 依赖，删 `sync-lilia-styles.mjs` 与拷贝的 src CSS，`index.css`/`bundle-css.mjs` 改从包解析（§5.1）；lockfile 同提交。
- 验收：`pnpm --filter @mutsuki/ui typecheck && build` 通过；`dist/mutsuki-ui.css` 内容 = `@lilia/theme`（pinned）+ `console.css`，token 段与 LiliaUI 源逐字节等价（diff 校验）；Web Console 页面在 dark/light 下视觉与迁移前一致；`/fonts` 缺失时不报错（系统字体回退）。

**Phase D — BotPlugins**
- 动作：按 pinned `@mutsuki/ui` 重建 dist；重跑 `scripts/sync-mutsuki-ui-css.sh` 刷新 3 个 crate 的 `assets/mutsuki-ui.css`；`cargo fmt --check`、`cargo check`、`cargo test`。
- 验收：3 份 `assets/mutsuki-ui.css` 与新 dist 一致；`include_str!` 构建通过；无 `lilia-tokens.css` 残留；嵌入式 Web Console 渲染正常。

**Phase E — Template pin 与跨仓验收（本仓库）**
- 动作：更新本仓库对相关仓库的 pin（若 Template 直接/间接引用 `@mutsuki/ui` 或消费其 CSS 产物）；更新 `docs/` 验收记录。
- 验收：`cargo metadata --locked` 通过；独立 checkout（无兄弟仓库）可解析全部 pinned 依赖；跨仓 smoke（启动嵌入式 Web Console，样式正确）通过；`git status --short` 干净，pin 与 lockfile 一并提交。

---

## 7. 风险与非目标

**非目标（写死，不做）：**
- **不**把不完整的 Vue 组件集抽进 `@lilia/theme`：theme 只含 CSS + 框架无关 `applyTheme` helper，零 Vue。所有 Vue 组件/Shell/Provider/Settings/Commands 留在 `@lilia/ui`。
- **不**恢复 `@lilia/nana-ui` 分叉或任何「第二视觉层」；`ui.preset` 仍只接受 `lilia`。
- **不**在 BotTemplate / 业务仓复制 CSS 实现或加生产 fallback/shim；跨仓依赖一律 pinned rev，禁止仓库外 `path`/`[patch]`。
- **不**把 `console.css`（Mutsuki 产品 chrome）上收进 LiliaUI —— 它是消费方业务样式。

**归属保持：**
- Tauri / Shell / `runtime/tauri` / `tauri-plugin-lilia` **仍属 `@lilia/ui` 与 LiliaUI 工程包**；`@lilia/theme` 不引入 Tauri 依赖。

**风险与缓解：**
- **字体路径耦合**（`/fonts/*.woff2`）：以「host-served 路径契约 + `@lilia/tools` 提供 woff2」明确责任；缺失时回退系统字体，不阻断。
- **CSS 顺序**：`tokens → state-layer → 布局 → console` 顺序必须保持（token/别名先定义）；`base.css` 内部 `@import` 与 `bundle-css.mjs` 拼接顺序都固定。
- **bundle 体积预算**：`public-api.md` 的 42,755 bytes / 4 async chunk 预算不放宽；抽取后重跑构建报告核对。
- **跨包管理器 Git 依赖**：LiliaUI 用 yarn workspace 语法，`@mutsuki/ui` 用 pnpm 消费 `github:...#workspace=...&commit=...`；Phase B 必须在 pnpm 独立 checkout 验证该语法可解析 `@lilia/theme` 子包。
- **视觉回归**：Phase A/C 用 `test:ui:visual` 与逐字节 diff 双重校验 token/布局无漂移。
- **依赖方向回归**：`check:ui-boundaries` 增加规则，禁止 `@lilia/theme` 反向依赖 contract/foundation/ui、禁止 Vue/Tauri 进入 theme。

---

## 附：包/文件动作速查

| 仓库 | 动作 |
| --- | --- |
| LiliaUI | 新增 `packages/theme`；迁 8 个基座 CSS + `styles.css`→`base.css`；`@lilia/ui` 依赖并 re-export theme；加 `applyTheme` helper；更新 boundaries/release 文档。 |
| MutsukiWebHost | `@mutsuki/ui` 依赖 pinned `@lilia/theme`；删 `sync-lilia-styles.mjs` 与拷贝的 src CSS；`index.css`/`bundle-css.mjs` 改从包解析；保留 `console.css`；lockfile 同提交。 |
| MutsukiBotPlugins | 保留 `sync-mutsuki-ui-css.sh`（dist→Rust 物化）；按 pinned `@mutsuki/ui` 重建并刷新 3 份 `assets/mutsuki-ui.css`。 |
| MutsukiBotTemplate | 更新 pin 与 `docs/` 跨仓验收；`cargo metadata --locked` + 独立 checkout smoke。 |

---

## 附：实施验收记录（2026-07-24）

按 Phase A→E 实施并推送，实际结果：

| 仓库 | commit（已 push main） | 关键动作 |
| --- | --- | --- |
| LiliaUI | `4dab5476073468c4da24a39cf4b85cad6464e83a` | 新增 `packages/theme`（迁 8 基座 CSS + `styles.css`→`base.css` + `fonts.css` 子入口 + 框架无关 `applyTheme`）；`@lilia/ui` peer 依赖并 `@import` 转发 theme；boundaries 纳入 theme；token/基座测试改读 `packages/theme/src`。`typecheck`/`test`(213)/`check:ui-boundaries`/`check:contract-fixtures`/browser+surface 均过。 |
| MutsukiWebHost | `bf0e57cf26d0ededc4dbb7e6e0a9c634fb7cbb76` | `@mutsuki/ui` 删 `sync-lilia-styles.mjs` 与拷贝 CSS，依赖 pinned `@lilia/theme`；`bundle-css.mjs` 从 node_modules 解析并递归内联 `@import`。`typecheck`+`build` 过，dist token 段与 theme 源逐字节等价。 |
| MutsukiBotPlugins | `b2da81c0fe8e5568bdd8581e939355fe38bdc73d` | 保留 `sync-mutsuki-ui-css.sh`，从 WebHost dist 重新物化 3 份 `assets/mutsuki-ui.css`（追溯到 pinned theme），删 legacy `lilia-tokens.css`。`fmt`+web-console/overview/config 测试过。 |
| MutsukiBotTemplate | 本次提交 | pin BotPlugins → `b2da81c`；移除临时 `[patch]`；`cargo metadata --locked`、`fmt --check`、`cargo test -p mutsuki-bot`（含两个 web_console smoke）全过。 |

### 与方案的必要偏差（已验证）

1. **pnpm 不支持 yarn 的 `#workspace=...&commit=...` 语法**（Phase B 风险点已证实）。WebHost 用 pnpm，改用 pnpm 官方 git 子目录语法
   `github:sena-nana/LiliaUI#<sha>&path:/packages/theme`，可正常解析 `@lilia/theme`（无 Vue/Tauri）。yarn 消费方仍用 `#workspace=...&commit=<sha>`。
2. **`@lilia/ui` 对 theme 采用 peer(`^0.1.0`) + workspace devDep**（与既有 ui-contract/ui-foundation 一致），而非方案字面的 `dependencies: workspace:*`——后者在远端 git pin 消费时无法解析。
3. **`@lilia/ui` 的 `styles.css`/`styles/*.css` 用一行 `@import` 转发**（方案允许的退化路径），因 Node/bundler 不支持 `exports` 目标写裸包说明符。
4. **Template 的 WebHost Rust pin 保持 `f77cb0b`**（与 BotPlugins 内部 pin 一致，避免 `mutsuki_web_host` 双版本冲突）。主题迁移只影响 `@mutsuki/ui`（JS/CSS，已物化进 BotPlugins），不涉及 WebHost 的 Rust crate。
5. `useThemeSync`（Vue）暂未改为封装 `applyTheme`，以免给 `ui-foundation` 引入 `→theme` 依赖边；`applyTheme` 已作为 theme 的框架无关导出提供。
