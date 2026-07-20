# NemesisBot 签名 / 验证 / 吊销系统

本包含两个二进制（Windows）：
- **`exe-sign-tool.exe`** — 签名 / 验证客户端（PE/ELF/Raw）
- **`revoke-server.exe`** — 吊销服务端（含 Web UI + SQLite + 审计）

> 密钥（`keys\`）、数据库（`data\`）、配置都是**部署后**生成，不在包里。
> 所有脚本是 **Windows 原生 .bat**，在 cmd / 资源管理器双击运行。

---

## 一、部署服务端

### 方式 1：便利脚本（推荐）

```cmd
REM 默认（keys\ + 0.0.0.0:7878 + 随机 token + data\revoke.db）
start-server.bat

REM 自定义参数：keys目录  bind  admin_token  db路径
start-server.bat keys 0.0.0.0:7878 myToken data\revoke.db
```

脚本自动：① 首次 `keygen` 生成密钥到 `keys\` ② 后台启动 `revoke-server.exe`（隐藏窗口）。
**启动时打印 admin token（请记录，Web UI 登录用）**。

### 方式 2：手动命令

```cmd
exe-sign-tool.exe keygen --out keys
revoke-server.exe --crkey <crkey_hex> --db-url data\revoke.db --admin-token <你的token> --bind 0.0.0.0:7878
REM 注：--crkey 传 exe_sign.crkey 文件里的 hex 内容（64 字符），不是文件路径
```

### 访问
- **Web UI**：浏览器 `http://<host>:7878/` → 输入 admin token 登录 → 管理吊销/受信任公钥/查审计
- **API**：`POST /v1/verify`、`GET /v1/crl`、`GET /v1/trusted-keys`、`POST /v1/admin/revoke`、`GET /v1/audit`、`GET /v1/health`

### 停止
```cmd
taskkill /F /IM revoke-server.exe
```

### 数据迁移 / 证书迁移
- **换机器**：拷贝 `keys\`（含 `exe_sign.crkey` 吊销根私钥）+ `data\revoke.db`（CRL/审计）到新机器，同样命令启动。
- **换密钥**：重新 `keygen`（旧签名全部失效，因公钥变）。

---

## 二、测试客户端

### 方式 1：便利脚本

```cmd
REM 仅本地测
test-client.bat

REM 本地 + 云端测
test-client.bat http://server:7878
```

脚本自动：① 首次 `keygen` ② sign `test-app.exe`（publisher=TestPub）③ verify（本地 + 可选云端）。

### 方式 2：手动命令

```cmd
exe-sign-tool.exe keygen --out keys
exe-sign-tool.exe sign app.exe --key-dir keys --publisher MyOrg
exe-sign-tool.exe verify app.exe --key-dir keys                                  REM 本地
exe-sign-tool.exe verify app.exe --key-dir keys --cloud-url http://server:7878    REM 云端实时
```

### 验证结果（SignatureStatus）
- `code=Valid cloud=Reached source=Cloud` — 云端实时核实通过（高可信）
- `code=Revoked` — 已被吊销（CRL 命中）
- `code=Expired` — 签名过期
- `code=Valid cloud=Unreachable source=Local` — 云不可达，本地放行（**非高可信**，仅 fallback）

---

## 三、完整闭环测试（客户端 ↔ 服务端）

```cmd
REM 1. 部署机：启动服务端（记下 token）
start-server.bat

REM 2. 客户端：测试（同一台或另一台）
test-client.bat http://127.0.0.1:7878

REM 3. Web UI 吊销：浏览器 http://127.0.0.1:7878 → 登录 → 添加吊销（publisher=TestPub）

REM 4. 再验证：云端应返回 Revoked
exe-sign-tool.exe verify test-app.exe --key-dir keys --cloud-url http://127.0.0.1:7878
```

---

## 四、安全注意

- **admin token**：启动时设强 token，不要用默认/弱值。Web UI + admin API 都靠它。
- **`exe_sign.crkey`（吊销根私钥）**：服务端签 CRL/响应用，**机密**，泄露可伪造"未吊销"。妥善保管（keys\ 目录权限）。
- **`exe_sign.key`（签名私钥）**：客户端签可执行文件用，机密。
- **纯软件本地验证不绝对安全**：攻击者控制运行环境可绕本地。**只有 `cloud=Reached`（云端实时）才算高可信**。高危场景只信云端。
- 算法版本化：未知版本/算法返 `UnsupportedVersion`，绝不误判 Valid。
