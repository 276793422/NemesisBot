工作流单工具（action 判别四动作）：run=按名运行已注册工作流（返回执行 id 与节点输出）/ create=把流程定义存为草稿（不生效，先 capabilities 拿字段规格）/ apply=把草稿登记进引擎（覆盖同名自动备份 .history/ 并删草稿，登记后即可 run）/ capabilities=查询节点类型、配置字段、触发器与结构规则。

**用法要点**
- run：执行名必须来自已注册工作流，不确定有哪些可跑时先 action=capabilities 或查工作流页，不要凭记忆猜名字。
- 每次执行有 id 可追溯；拿到各节点输出后做核验与汇总，不要只看最终状态就下结论。
- create：写定义前必先 action=capabilities 拿精确字段规格；validation_errors 非空时按提示修正后同名重存。
- apply：create 存好后直接 action=apply 登记，接着 run，全程无需用户介入；响应带 replaced_existing——覆盖了用户已有的同名工作流时必须向用户说明（旧定义自动备份 .history/）。
- apply 只做登记不执行节点；执行仍走 run，节点内工具调用照常过安全管线。

**决策路由**
- 一次性任务没有现成工作流 → 直接用基础工具做；重复出现的固定流程 → create 沉淀成工作流，apply 登记，再 run。
- 需要推理与中间决策的任务 → `spawn` 子代理（工作流是预定义 DAG，不现场推理）。
- 多节点协作的看板任务 → `board_issue`。

**约束与边界**
- create 落草稿文件过安全管线（file_write 档）；apply/run/capabilities 为登记/只读/执行面。
- 工作流节点内的工具调用照常过审批与拦截，工作流不是旁路。
- 执行失败读失败节点的输出定位原因，不要原样反复重跑。
