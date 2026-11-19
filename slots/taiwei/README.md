# taiwei（静态直链槽）

商业子系统槽位，住槽区 `slots/` 下（未来新增定制子系统一律 `slots/<name>/`，
不进仓库根目录）。

**形态（2026-11-13 rlib 直连，取代归档手术直链）**：宿主 `nemesisbot` 的
`build.rs` 在每次构建时探测本目录 `impl/target/release/libnemesis_taiwei.rlib`
（crate 名 `nemesis_taiwei`，`.cargo/config.toml` 以静态 `--extern` 挂进
extern 搜索面）——

- **rlib 缺席（开源常态）**：`--extern` 路径悬空，rustc 惰性——crate 无引用
  时零影响，零 cfg、零链接指令，exe 内零太微符号；
- **rlib 在场**：build.rs 四防线版本对质（见下）通过后 emit
  `rustc-cfg=nb_taiwei` 点亮引用面（`nemesisbot/src/taiwei_bridge.rs`），
  宿主 `lto="thin"` 的 ThinLTO 管线直接吸收 taiwei rlib 内嵌的 bitcode
  ——太微代码与宿主一起做最终 codegen，单一 rlib 服务宿主全部 profile。

同一条 `cargo build --release`，目录状态自己说话——宿主 manifest/依赖图零
taiwei 痕迹。

> 为什么是 rlib 不是 staticlib：宿主开 `lto="thin"` 时 rustc 在编译期就跑
> ThinLTO，std/core 符号已被内化，opaque archive 直链在链接期无定义可拉
> （/INCLUDE 强引无效，实证 20 undefined）；全量拉入又必撞 duplicate
> symbol。rlib 携带的 bitcode 是 IR，经 ThinLTO 融合是唯一结构性出路
>（2026-11-13 十项实验闭环，含阴性/阳性双向验证）。

## 太微实现 = 本项目的声明式子模块

私有实现仓（太微大帝系统）以 **git submodule** 声明在本目录 `impl/`——写在
项目里（`.gitmodules`），不是路径暗道：

```bash
git submodule add --branch main \
  https://github.com/276793422/TaiWei.git slots/taiwei/impl
```

- **有权限**：`git submodule update --init slots/taiwei/impl` 拉下实现；
  后续太微开发就在挂载点里照常 commit/push（独立 git 仓）。
- **没权限**：拉不了是正常依赖语义；公开构建完全不受影响（探测按「目录
  缺席 = 开源形态」静默短路）。

## 商业构建（两条命令，编译习惯与输出位置不变）

```bash
# ① 太微侧：进 submodule 正常构建（rlib 落 target/release/，自带双版本戳
#    <lib>.stamp = 短 commit [+ -dirty] + rustc commit-hash 两行）
cd slots/taiwei/impl && cargo build --release

# ② 宿主侧：回根目录正常构建——build.rs 自动探测 + ThinLTO 融合
cargo build --release
```

- 环境烤入（`NEMESIS_TELEMETRY_ENDPOINT/_PUBKEY/_TOKEN`，`option_env!`）在
  太微侧构建时发生——命令①前设置同名环境变量即可，烤入值随 rlib 进 exe。
- **stale 四防线**（宿主 build.rs 对质，错配即硬错并给出修复命令）：版本戳
  ≠ submodule HEAD（源更新忘重编）/ 版本戳 rustc 行 ≠ 宿主 rustc（rlib
  元数据跨编译器版本无消费保证）/ 版本戳晚于库文件 ≥2s（构建半途而废）/
  rlib 在场但戳缺席（老构建）。`-dirty` 与 `unknown` = WARN 放行（诚实边界）。
- **交叉编译不探测**：rlib 按宿主三元组编译，元数据跨三元组不可消费
  （android/musl 交叉构建走各自脚本，天然不受影响）。
- profile 无关：rlib 内嵌 bitcode 是 IR，宿主 release/debug/iotsmall 各自
  最终 codegen——固定探测 `target/release/`，单一产物服务全 profile。
- 仓库内嵌套构建安全：宿主 `.cargo/config.toml` 的 `--extern` 会被太微侧
  构建继承（cargo 配置向上爬目录），但太微仓内**无人引用** extern 名
  `nemesis_taiwei`（内部依赖已改名 `tw-impl`）——悬空 flag 惰性化，
  rustc 对未引用的 extern 零动作（实证 EXIT=0）。
- 恢复开源形态：删掉 `impl/target/`（或仅删 rlib）再正常构建即可。

## 历史形态（已退役）

- v1 装配槽（stub crate + optional path 依赖 + `taiwei` feature）：公开
  manifest 指向 stub、商业构建在装配复制体里换指。
- v2 xtask 装配流水线（`cargo xtask assemble`：复制体 + 换指 + 环境指纹 +
  交付 `bin/bin_assembly/`）。
- v3 opaque staticlib 直链 + 归档手术（剥 sysroot 成员 + 符号索引重排）：
  在宿主 `lto="thin"` 下结构性不可行（见上），整体退役。
- 三者已全部退役（2026-11-13 用户拍板「不要 assembly，不要 bin_assembly，
  这就是最终形态」），交付物即正常 `target/release/nemesisbot.exe`——
  不再有独立商业产物目录。

当前状态：接线完成且私有仓已推送远端——`impl/` 挂 9ac552f（origin/main），
其他机器 `git submodule update --init slots/taiwei/impl` 即可拉下（有权限时）。
