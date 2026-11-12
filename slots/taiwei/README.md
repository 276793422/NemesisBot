# taiwei（装配槽）

商业子系统槽位，住槽区 `slots/` 下（未来新增定制子系统一律 `slots/<name>/`，
不进仓库根目录）。开源形态本目录只有 stub（`Cargo.toml` + `src/lib.rs`），用于
满足 `nemesisbot` 对 optional path 依赖 `nemesis-taiwei` 的解析（cargo 对
path/git 依赖恒解析——无论 feature 开关都会读 manifest/拉取，所以公开
manifest 永远只指向 stub）：feature `taiwei` 关 = 永不编译；误开 = stub
`compile_error!` 诚实拦下。

## 太微实现 = 本项目的声明式子模块（docs/skins 同款语义）

私有实现仓（太微大帝系统）以 **git submodule** 声明在本目录 `impl/`——写在
项目里（`.gitmodules`），不是路径暗道：

```bash
git submodule add --branch main \
  https://github.com/276793422/TaiWei.git slots/taiwei/impl
```

- **有权限**：`git submodule update --init slots/taiwei/impl` 拉下实现；
  后续太微开发就在挂载点里照常 commit/push（独立 git 仓）。
- **没权限**：拉不了是正常依赖语义；公开构建完全不受影响（cargo 图里没有
  它），stub 槽位照常解析。
- **商业构建**（xtask `assemble`）：`cargo xtask assemble [--endpoint URL]
  [--pubkey HEX] [--token TOKEN] [--profile release|iotsmall] [--clean]`——
  仓库复制到装配工作区（`assembly/`，git 忽略，mtime 保持 + 共享 target，
  重复装配走增量）→ 复制体内把 nemesisbot 的 path 改指
  `slots/taiwei/impl/crates/taiwei-impl` 挂载点 → `--features taiwei` 构建 →
  产物验证（实现指纹 + 注入值烤入面）→ 交付物落 `assembly/dist/`。构建期
  `NEMESIS_TELEMETRY_ENDPOINT/_PUBKEY/_TOKEN` 缺省 = 实现内建默认；显式传参
  或同名环境变量可覆盖。公开工作区零写入，公开仓永远只保留 stub 形态，槽位
  不丢、`git status` 恒干净。

当前状态：接线完成且私有仓已推送远端——`impl/` 挂 c92e1ea（origin/main），
依赖解析走云端 canonical 地址；其他机器 `git submodule update --init
slots/taiwei/impl` 即可拉下（有权限时）。
