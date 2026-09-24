# NemesisBot 产物验签说明（v4 Authenticode 格式）

本包内的 `nemesisbot` 主程序与 `plugins/` 下全部插件均带 v4 签名
（ECDSA P-256 + X.509 三级证书链 + CMS，与 Windows Authenticode 格式对齐）。
`certs/root_cert.der` 是信任根的**公开**证书；`signatures.json` 是本包的签名清单。

## 验证单个文件

```
sign-tools/exe-sign-tool verify --root-cert certs/root_cert.der --target nemesisbot
```

Windows 包:

```
sign-tools\exe-sign-tool.exe verify --root-cert certs\root_cert.der --target nemesisbot.exe
```

## 验证包内全部文件

```bash
for f in nemesisbot plugins/*; do
  exe-sign-tool verify --root-cert certs/root_cert.der --target "$f" || echo "FAILED: $f"
done
```

## 结果状态含义

| 输出 | 含义 |
|---|---|
| `Valid` | 签名完整、证书链可达信任根、未过期 |
| `Tampered(detail)` | **文件被篡改**（任一字节变化都会到这）——不要使用 |
| `Untrusted` | 证书链无法锚定到所给根证书（换根了或根不匹配） |
| `NoSignature` | 文件没有签名 |
| `Revoked` | 签名/密钥已被吊销（需 `--revocation-url` 联网查询时） |
| `Expired` | 证书链过期 |

## 查吊销（可选，部署了吊销服务时）

```
exe-sign-tool verify --root-cert certs/root_cert.der --target nemesisbot --revocation-url https://<吊销服务地址>
```

## Windows 原生方式（装根一次）

双击 `certs/root_cert.der` →「安装证书」→ 存储位置选「当前用户」→
「受信任的根证书颁发机构」。之后右键任意产物 → 属性 →「数字签名」选项卡，
或用 `signtool verify /pa nemesisbot.exe` / `Get-AuthenticodeSignature`，
全部显示有效。

不装根时 Windows 工具唯一报错是 `CERT_E_UNTRUSTEDROOT`（自签根的固有语义）。
