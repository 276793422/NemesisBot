# 职能化拆解方法论（软件项目流水线模式）

本节是拆解方法论补充：把软件项目按**专业职能**拆成一条可并行、可验收的工件流水线。普通小任务不需要职能化——判断标准见「何时职能化」。

## 何时职能化
- **走全流水线**：新模块/新系统/需求模糊的大特性——需求本身需要先澄清（PRD），技术方案需要先定型（架构）。
- **不职能化（required_profession 填空串）**：bug 修复、小改动、边界清晰的单一特性——直接拆 dev 子单，套全流水线是仪式主义。
- **半流水线**：需求清晰但跨多技术面的中等任务——可只拆 架构 → 开发 两段，跳过 PRD。

## 职能目录（required_profession 的值域）
`required_profession` 填以下 slug 之一，无职能需求填空串：
- `product` —— 产品经理（需求澄清/PRD）
- `ui-design` —— UI 设计（设计规格/HTML 原型）
- `architecture` —— 架构师（技术选型/模块划分/接口契约）
- `dev` —— 开发工程师（通用职能）
- `dev:cpp` —— 开发工程师（C/C++ 专项）
- `test-whitebox` —— 白盒测试开发（测试代码/覆盖分析）
- `test-blackbox` —— 黑盒测试（验收对照/缺陷报告）
节点可能宣告本目录之外的自定义职能；确有理由时可以填写，但优先使用本目录（未宣告自定义职能会导致该子单派不出去）。
匹配语义是**精确匹配（无继承）**：宣告 `dev` 的节点接不了 `dev:cpp` 的子单，反向亦然——需要专项时写完整 slug（如 `dev:cpp`），不要期望 `dev` 能覆盖。

## 流水线骨架（依赖链模板）
新项目全链拆解的标准形态：

```
① product（PRD）                ← 需求模糊时必拆；description 必须给足背景/用户/场景线索
② architecture（架构）           ← depends_on: ①
③ ui-design（UI 设计）           ← depends_on: ①，与 ② 并行（只依赖 PRD，不依赖架构）
④ dev[:spec]（实现）×N           ← depends_on: ②（有 UI 的另依赖 ③）
⑤ test-whitebox（白盒）          ← depends_on: 对应 dev 子单（接口契约齐后可与开发并行收尾）
⑥ test-blackbox（黑盒验收）      ← depends_on: ④ 全部完成
```

规则：
1. **PRD 子单的 description 是流水线质量的根**：写清背景/用户/场景三要素与已知的约束线索——产品经理节点拿到才能产出有效 PRD；线索不足就写「需求待澄清」点明要澄清什么。
2. **可并行才并行**：依赖链里没有前后关系的子单（如 UI 与架构）不要人为串行；会写同一工件的子单必须串行或合并（共享文件纪律照常生效）。
3. **每个职能子单的 acceptance_criteria 写工件契约**：PRD 单验 `docs/prd.md`（含「## 验收标准」节）、架构单验 `docs/architecture.md`、UI 单验 `design/ui-spec.md` 与 `design/prototype.html`、白盒单验 `docs/test-report-whitebox.md`、黑盒单验 `docs/test-report-blackbox.md`。验收锚点优先用 `[CHECK] re:` 形态对交付汇报实核（远端任务禁 `file:` 锚点——拓扑纪律照常）。
4. **实现类子单（dev）照既有拆解纪律**：单层、可独立验收、共享文件串行化；`dev:cpp` 只在确为 C/C++ 工作面时使用。
5. **工件路径约定全项目统一**：`docs/prd.md`、`docs/architecture.md`、`design/ui-spec.md`、`design/prototype.html`、`docs/test-report-whitebox.md`、`docs/test-report-blackbox.md`——后续子单按约定路径读取上游工件，不要发明新路径。

## 拆解自检
- 需求模糊度高的项目是否有 PRD 子单打头？（没有则整个链条建在流沙上）
- 每个职能子单的验收标准是否对应工件契约的路径与关键章节？
- dev 子单是否拿到了足够线索（依赖架构/PRD 工件路径，执行者知道去哪读上游产出）？
- 黑盒子单是否依赖全部实现完成？（验收对照需要对完整行为面）
