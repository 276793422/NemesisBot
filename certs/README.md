# certs/ — 签名信任根（公开材料）

本目录存放 v4 签名体系的**信任根公开证书**，随产物分发，供所有使用者验签。

| 文件 | 性质 | 来源 |
|---|---|---|
| `root_cert.der` | 根证书**公开**部分（X.509 DER）。公钥+身份+有效期，不含任何私钥 | 初始化时 `exe-sign-tool keygen` 生成、`split-keys` 拆出 |

信任锚 = 本文件的 **SHA-256 指纹**（初始化时由 `init-signing.sh keygen` stage 打印并记录）。
验证端（exe-sign-tool / verify-loader / 未来 nemesisbot 自验）都锚定该指纹。

## 变更纪律

**修改本目录下的证书 = 更换信任根**，属于最敏感操作：

- 会导致所有既有产物在新根下报 `Untrusted`；
- 需要重编全部持有锚的验证端；
- 因此任何触碰本目录的 PR 必须在标题/正文中显式声明「换根」，走完整 review。

`root_cert.der` 本身是公开材料，入库不构成泄密；**任何私钥（root_sk / issuing_sk /
leaf_sk）绝对不入库**——红线见 `docs/PLAN/2026-09-23_ci-v4-signing-integration.md` §10。
