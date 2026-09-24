# Signserver 运维包（owner 用）

本包是 **owner 运维包**：配合你离线保管的全量 `keys.json` 运行 v4 签发 + 吊销
服务（revoke-server）。**包内零私钥**——revoke-server 启动必须加载全量
keys.json（三级私钥俱全，`validate_full_consistency` 诚实校验），缺它起不来。

终端用户的产物验证**不走本包**：用 `sign-tools` 包里的 `exe-sign-tool`
本地验签即可（`VERIFY.md` 有逐步说明）。本包面向的操作是：

- **签发记账**：`POST /v1/sign`（签发即记账，`v4_content_digest` / `key_fp` /
  `latest_sig_hash` 三值同源）
- **吊销**：`POST /v1/admin/revoke`（四维度：key_fp / sig_hash / file_hash /
  publisher；吊销某次 nightly 构建时从该 release 的 `signatures.json` 拿
  `key_fp`）
- **CRL 分发**：`GET /v1/crl`（或把导出的 CRL 作为 release 附件分发——
  `certs/` 目录只放根证书，保持「碰 certs/ = 换根」纪律纯粹）

## 快速启动

```bash
# 1. 把离线保管的全量 keys.json 放到安全位置（本目录外的私密路径）
# 2. 启动（首次 --init-keys 只用于全新密钥体系，已有体系不要带）
./revoke-server --keys-file /secure/path/keys.json --bind 127.0.0.1:7878

# 3. 健康检查
curl http://127.0.0.1:7878/v1/health
```

- 管理面：`web/admin.html`（登录 + CRL / 吊销 / trusted-keys / 审计）
- 验证端挂吊销查询：`exe-sign-tool verify --root-cert certs/root_cert.der
  --revocation-url http://<你的服务器> --target <文件>`
- 详细 API 与参数：仓库内 `docs/PLAN/2026-07-20_cloud-signing-service.md`

## 安全边界（诚实声明）

- 本服务持全量私钥运行 = 最高权限面：只应部署在 owner 控制的机器上，
  不暴露公网（`--bind 127.0.0.1` + 反代/防火墙按需收窄）。
- `certs/root_cert.der` 是信任根公开证书，SHA-256 指纹 = 全部 nightly 产物
  的编译期锚（`NEMESIS_BUILD_ROOT_ANCHOR`）；换它 = 换根（最贵操作）。
