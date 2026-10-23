# dummy-targets —— 紫微系统假想目标夹具（T0.8）

紫微系统（【万物皆工具】）M1-M5 集成测试的**注入/读取靶子**。独立项目（非
workspace member），零依赖，仅 Windows x86/x64。

| 目标 | 用途 | 预埋要素 |
|------|------|----------|
| `dummy-draw` | 注入/管道/read_mem/锚点扫描/失效演练 | `G_AREA` 数据区（magic+layout+记录数组）、`push_record`（入口 8×0x90 + imm 魔法）、两级链 `feed_render→render_record`、特征字符串、GDI 心跳；`--mutate` 切 V2 布局 |
| `dummy-monitor` | read_mem/utf16 解码/on_change 轮询 | `G_STATE`（magic/status/tick/utf16 消息），每 interval 翻转 OK→WARN→ALERT |

## stdout 契约（测试驱动解析）

两个目标启动即打印（println! = LineWriter，管道下逐行落）：

```
dummy-draw:    <产品串 V1|V2> / 特征串 / 链串 / PID=<pid> / LAYOUT=<1|2> / MAGIC=<hex> / DATA_ADDR=<hex>
dummy-monitor: ZIWEI_DUMMY_MONITOR_V1 / PID=<pid> / STATE_ADDR=<hex> / INTERVAL_MS=<n> / tick=<n> status=<0|1|2>
```

`DATA_ADDR` / `STATE_ADDR` 是 M1 read_mem 直读入口（M3 起可被锚点扫描替代——
byte_pattern 找 `push_record` 再按偏移定位数据区）。

## 构建（M1 测试前置）

```bash
bash test-tools/dummy-targets/build.sh          # x64 + x86 双架构 → bin/
# x86 需要先：rustup target add i686-pc-windows-msvc
```

产物：`test-tools/dummy-targets/bin/{x64,x86}/{dummy-draw,dummy-monitor}.exe`。
M1+ 集成测试按此路径查找，缺席时诚实跳过并注明原因（不现场编译，避免测试
进程里嵌 cargo）。

## mutate 语义（M5 失效演练）

`dummy-draw --mutate` = 模拟「软件已更新」：同一可执行文件，运行期切 V2
布局（字段重排 + label 16→24 + 尾部加字段 + 魔法 +1）。V2 下：

- byte_pattern 锚点（函数入口 nop 串）**仍命中**（代码没变）→ anchors vote 通过；
- 数据区 magic/layout/字段偏移**全变** → 按 V1 建模的 profile 判定应落在
  ActiveDegraded（锚点过但版本/布局失配）；
- 完全 Stale（锚点也丢）的场景由单元测试以「扫错误 pattern」模拟。
