# authn-demo — 身份与访问框架独立验证 demo

> **性质**：独立验证任务（非产品集成）。仿 `nemesis-verify` 先例：crate 先在
> test-tools 里独立跑通、由用户亲手验收，最后才谈移植进 NemesisBot。
> **本目录不依赖任何 `nemesis-*` crate，产品代码零改动，plan 文件零改动。**

## 验证范围（与计划 F2 的 A+B 块对应）

| 验证项 | 内容 | demo 子命令 | 外部依赖 |
|--------|------|-------------|----------|
| ① OIDC 全流程 | discovery → PKCE → 授权码 → 令牌验签 → claims → 角色提取 | `oidc-login`（浏览器）/ `oidc-ropc`（脚本化） | Keycloak 容器 |
| ② LDAP/AD | DN 转义 → bind → 组提取（memberOf / filter 反查）→ LDAPS 选项 | `ldap-login` | OpenLDAP 容器 |
| ③ 本地账号 | argon2id 哈希/校验 + JSON 账号文件 + hash-password CLI | `hash-password` / `local-login` | 无 |
| ④ 会话 | 签发 / TTL 过期 / 吊销 / 单用户并发上限 | `selftest` / `local-login --session-ttl-secs 1` | 无 |
| ⑤ 静态 token 兼容 | 空=开放模式 / 非空=单用户模式（常量时间比较） | `compat` / `selftest` | 无 |

## 目录布局（移植准备）

```
authn-demo/
├── Cargo.toml            # 独立 cargo workspace（不属于 NemesisBot 根 workspace）
├── authn-core/           # ★ 可复用 crate（移植时整个拷走/改为 path 依赖）
│   └── src/
│       ├── identity.rs   #   通用出口形状：Identity{subject, display_name, roles, source}
│       ├── local.rs      #   本地账号 + argon2（验证项③）
│       ├── session.rs    #   会话存储 + SessionStorage trait 轮廓（验证项④）
│       ├── compat.rs     #   旧静态 token 共存语义（验证项⑤）
│       ├── oidc.rs       #   OIDC 协议引擎（验证项①）
│       ├── ldap.rs       #   LDAP/AD 协议引擎（验证项②）
│       └── tests*.rs     #   独立测试文件（40 用例：36 纯逻辑 + 4 容器门控）
├── demo/                 # demo CLI（authn-demo.exe，每个验证项一个子命令）
├── docker/               # Keycloak + OpenLDAP 容器编排 + 种子数据
└── examples/users.sample.json
```

### 复用三边界（移植前的硬约束，代码内已贯彻）

1. **零 `nemesis-*` 依赖**——只依赖外部协议 crate（openidconnect / ldap3 / argon2 / …）。
2. **builder 式配置**——所有配置由调用方构造传入，crate 绝不读任何产品的 config.json。
3. **通用出口形状**——所有认证路径收敛到一个 `Identity{subject, display_name, roles, source}`；
   `source`（Oidc/Ldap/Local/StaticToken）供产品侧翻译成自己的审计与授权语义。

---

## 快速开始

### 第 0 步：纯逻辑自测（无任何外部依赖，1 分钟）

```bash
cd test-tools/authn-demo
cargo build
./target/debug/authn-demo.exe selftest
```

预期：15 项检查全部 `[✅]`，末行 `全部通过 ✅`。
（argon2 哈希/校验、会话签发/过期/并发、静态 token 三态——对应验证项③④⑤。）

单元测试同理不依赖容器：

```bash
cargo test          # 36 passed, 4 ignored（ignored 的需要下面的容器）
```

### 第 1 步：起容器（验证项①②需要）

```bash
cd docker
docker compose up -d
```

首次会拉 `quay.io/keycloak/keycloak:26.0`（约 1GB）和 `bitnami/openldap:2.6`。
Keycloak 健康检查就绪约需 40-90 秒：`docker compose ps` 看 keycloak 变 healthy。

种子数据（compose 首次启动自动导入，重导需 `docker compose down -v` 后再 up）：

| 系统 | 账号 | 密码 | 角色 |
|------|------|------|------|
| Keycloak（realm `demo`） | alice | alice123 | operator, viewer |
| Keycloak | bob | bob123 | admin, operator, viewer |
| OpenLDAP | carl | carl123 | operators（组） |
| OpenLDAP | dana | dana123 | operators, admins（组） |

端口：Keycloak **8088**（控制台 http://localhost:8088，admin/admin）、
OpenLDAP **1389**（ldap）/ **1636**（ldaps）。demo 回调端口 **18081**。
（刻意避开 NemesisBot 常用端口 8080/49000/49001/18790/21949。）

`docker/ldap/tls/` 里的证书/私钥是**demo 专用一次性自签件**（CN=localhost，
有效期 10 年，仅用于本机 LDAPS 演示，不保护任何真实资产）；生产环境替换为
受信 CA 签发并去掉 `--tls-no-verify`。

### 第 2 步：验证项① — OIDC 全流程

**脚本化路径（ROPC，无浏览器，先跑这个确认链路）：**

```bash
./target/debug/authn-demo.exe oidc-ropc \
    --issuer http://localhost:8088/realms/demo \
    --username alice --password alice123
```

预期：token 响应摘要 + 身份 JSON（subject=`alice`，roles 含 `operator`/`viewer`）。

**浏览器全流程（PKCE + CSRF + nonce 验签，真实用户路径）：**

```bash
./target/debug/authn-demo.exe oidc-login --issuer http://localhost:8088/realms/demo
```

按提示把授权 URL 粘进浏览器 → alice/alice123 登录 → 同意后回调本机 18081 →
终端打印已验签的 id_token 载荷与 Identity。
（浏览器流程验了 ROPC 不覆盖的部分：state 校验、PKCE 交换、JWKS 验签。）

### 第 3 步：验证项② — LDAP

```bash
# carl: bind + 组反查（filter 模式，OpenLDAP 无 memberof overlay 的通用姿势）
./target/debug/authn-demo.exe ldap-login --url ldap://localhost:1389 \
    --username carl --password carl123

# dana: 应拿到 operators + admins 两个组
./target/debug/authn-demo.exe ldap-login --url ldap://localhost:1389 \
    --username dana --password dana123

# 负路径：错密码 / 不存在用户 → 统一 InvalidCredentials
./target/debug/authn-demo.exe ldap-login --url ldap://localhost:1389 \
    --username carl --password wrong

# LDAPS（容器自签证书，--tls-no-verify 仅 demo 用；生产配受信 CA）
./target/debug/authn-demo.exe ldap-login --url ldaps://localhost:1636 \
    --username carl --password carl123 --tls-no-verify

# memberof 模式（AD 风格——本 OpenLDAP 未加载 overlay，预期 roles 为空，仅演示开关）
./target/debug/authn-demo.exe ldap-login --url ldap://localhost:1389 \
    --username carl --password carl123 --group-mode memberof
```

容器门控的 e2e 测试（4 个）在容器就绪后可跑：

```bash
cargo test -p authn-core -- --ignored
```

### 第 4 步：验证项③④ — 本地账号 + 会话

```bash
# 生成 argon2id 哈希（PHC 格式），填进 users.json 的 password_hash 字段
./target/debug/authn-demo.exe hash-password 我的密码

# 用样例账号登录（账号文件见 examples/users.sample.json，内置 zoo/guest 演示账号）
./target/debug/authn-demo.exe local-login \
    --users-file examples/users.sample.json --username zoo --password zoo-demo-2026

# TTL=1s 演示过期：第二次校验返回 "error":"会话已过期"
./target/debug/authn-demo.exe local-login \
    --users-file examples/users.sample.json --username zoo --password zoo-demo-2026 \
    --session-ttl-secs 1
```

### 第 5 步：验证项⑤ — 静态 token 兼容

```bash
# 非空 token + 正确出示 → allow（静态 token 匹配）
./target/debug/authn-demo.exe compat --static-token legacy-tok --presented legacy-tok
# 非空 token + 不出示 → deny
./target/debug/authn-demo.exe compat --static-token legacy-tok
# 空 token → 开放模式（任何请求放行）——NemesisBot 现行默认语义
./target/debug/authn-demo.exe compat
```

### 收尾

```bash
cd docker && docker compose down          # 停容器（数据保留）
cd docker && docker compose down -v       # 连数据卷一起清（下次 up 重导种子）
```

---

## 对接真实公司 LDAP/AD（无需改代码，只换参数）

`ldap-login` 全部参数都是 CLI 开关，指向公司服务器 = 换参数。先问 IT/AD 管理员要 5 件事：

| 要问的 | 对应参数 |
|--------|----------|
| 服务器地址 + 端口（AD 一般 `ldaps://dc01.公司域:636`） | `--url` |
| 域名后缀（登录名是不是 `工号@公司域.com` 形态） | `--user-dn-template '{}@公司域.com'`（AD UPN 直 bind） |
| 目录根（如 `dc=corp,dc=example,dc=com`） | `--base-dn`（仅 filter 模式用） |
| LDAPS 证书是否内网 CA 自签 | 是 → 加 `--tls-no-verify`（域内机器常已信任则不用） |
| 是否只开了 389 明文端口强制 STARTTLS | 是 → `--url ldap://…:389 --starttls` |

**AD（最常见）**——两行：

```bash
./target/debug/authn-demo.exe ldap-login \
    --url ldaps://dc01.公司域.com:636 \
    --user-dn-template '{}@公司域.com' \
    --group-mode memberof \
    --username 你的域账号 --password 你的域密码
```

（AD 组提取走 `memberOf` 属性原生支持，`--base-dn` 不需要。登录成功但 roles 为空
→ 检查你是否真的在任何 AD 安全组里。）

**OpenLDAP 风格自建服务器**——DN 模板换 `uid={}` 形态 + filter 反查：

```bash
./target/debug/authn-demo.exe ldap-login \
    --url ldaps://ldap.公司域.com:636 \
    --user-dn-template 'uid={},ou=people,dc=公司域,dc=com' \
    --base-dn 'dc=公司域,dc=com' \
    --group-mode filter \
    --username 你的账号 --password 你的密码
```

诚实提醒：
- **命令行传密码会进 shell history**——demo 级卫生；测完可清 history，别用这方式测管理员账号。
- **嵌套组不展开**：AD 的 `memberOf` 只含直接组（嵌套组需 `LDAP_MATCHING_RULE_IN_CHAIN`，demo 未实现）。
- 先拿**自己的普通账号**测，别动服务账号；公司网络/VPN 得能通 DC 的 636/389 端口。
- 若公司身份源其实是 Entra ID / 内网 SSO（而非裸 AD），OIDC 引擎是对的那条路，见验证项①。



- **两段式 OIDC**：`oidc::begin` 返回可序列化的 `OidcPendingFlow`（授权 URL +
  CSRF state + nonce + PKCE verifier 全是普通字符串），`complete` 内重新 discovery
  重建 client——openidconnect 4.x 的 typestate client 不跨函数存活，换来的副作用是
  complete 时元数据新鲜。产品集成时 pending flow 可放进会话存储而非进程内存。
- **角色提取只吃可信渠道数据**：id_token 先经库内验签（JWKS + issuer + audience +
  nonce + 有效期），角色再从 id_token + access_token **两个载荷合并**——Keycloak
  默认把 realm 角色放 access_token 而 id_token 不带，其他 IdP（Auth0/Azure AD）
  惯例是 id_token 带 groups；两个 token 都是 token endpoint 直连 TLS 响应（可信
  渠道），没有未经可信渠道的输入进入角色映射。
- **LDAP 注入防御**：用户名进 DN 模板前过 RFC 4514 值转义，进 filter 前过 filter
  转义（`*()\` NUL），单测有注入样例回归。
- **本地账号不泄露存在性**：未知用户做等代价假校验；失败统一 `BadCredentials`。
- **会话存储轮廓**：`SessionStorage` trait + `token_fingerprint` 占位（生产应存
  SHA-256(token) 而非明文）——移植时替换存储实现即可，接口形状不变。
- **ROPC 的诚实定位**：`oidc-ropc` 仅测试用（Keycloak directAccessGrants），其
  id_token 未经 PKCE/nonce 链路验签，demo 只解码展示；生产用户路径一律走
  `oidc-login` 浏览器全流程。

## 诚实边界（demo 验证范围之外）

- **OpenLDAP ≠ 真实 AD**：sAMAccountName/UPN 登录名、嵌套组
  （`LDAP_MATCHING_RULE_IN_CHAIN`）、Windows 证书信任链只有接真 AD 才会暴露。
  demo 验证的是协议链路；预期接真 AD 只改配置不改代码（`user_dn_template`
  换 UPN 直 bind、`--group-mode memberof`、`group_object_class=group`）。
- **GFW**：Keycloak 镜像 ~1GB，若 quay.io 拉取超时，需给 Docker 配镜像加速后重试。
- **RUST 生态现实**：openidconnect 4.x 的 typestate API 较重（见 `oidc.rs` 顶部的
  `DiscoveredClient` 别名）；这是当前 crate 生态的现实，移植时如嫌重可评估换成
  手写 discovery + JWT 验签（ JWT 部分建议继续用成熟库）。

## 与计划的对应关系

本 demo 对应《安全治理框架通用化计划》F2 中的 **A（外部协议引擎：计划第 19/20 步）
+ B（身份内核：计划第 18/21 步）** 两块。**验证项①-⑤ 通过 = 这两块技术可行性
与工程形态得到验证**。计划中的其余部分（AuthOverlay 三形态、ToolInvocation.user
接线、审批人透传、SecurityRule.subject、审计链 user 字段）属于产品集成面，
**不在本 demo 范围**——那是移植阶段的事。
