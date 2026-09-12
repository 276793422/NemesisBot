#!/usr/bin/env bash
# S6-1 默认态行为矩阵自动化（goal §五 M2/M3/M4/M5 + D7 失败面质量线）
#
# 对 exe-sign-tool v4 自产样本断言（全过 = EXIT=0）：
#   M2  signtool verify /pa        → 失败且唯一错误 = CERT_E_UNTRUSTEDROOT（0109 官方文本）
#   M3  Get-AuthenticodeSignature  → Status=UnknownError + 0109 官方文本 + SignerCertificate 非空
#                                    （P0 实测修订：本机 PS 5.1 把 0109 映射为 UnknownError 非 NotTrusted）
#   M4  原内容区篡改 1 字节        → 微软双端 HashMismatch（signtool + PS）+ 自方 Tampered
#   M5  自方 verify（未篡改样本）  → Valid
#
# 产物/输出存 spike 工作目录 s61/（仓库外）；私钥纪律：keys.json 生成于 spike
# 目录，脚本收尾即删；root 公钥证书（root.der）保留供 S6-2 装根复跑。
#
# 前置：signtool（Windows SDK 10.0.26100.0）；本机 signtool/PS 消息为英文
# （S0-4/S0-5 同机实证）；不装任何根 = 默认态（跑前若装过锚先执行
# spike/s05-restore-default.sh）。
set -uo pipefail

SIGNTOOL="/c/Program Files (x86)/Windows Kits/10/bin/10.0.26100.0/x64/signtool.exe"
SPIKE="/c/AI/NemesisBot/Logs/2026-09-12_authenticode-spike/s61"
REPO="$(cd "$(dirname "$0")/.." && pwd)"
UNTRUSTEDROOT_TEXT="A certificate chain processed, but terminated in a root certificate which is not trusted by the trust provider"
HASHMISMATCH_TEXT="The digital signature of the object did not verify"

mkdir -p "$SPIKE"
cd "$REPO"
EST=./target/debug/exe-sign-tool.exe

fail() { echo "!! FAIL: $1"; exit 1; }

echo "### [0] build + keygen + v4 签发样本（PE = verify-loader.exe 副本）###"
cargo build -p exe-sign-tool -p verify-loader 2>&1 | tail -1
[ -x "$EST" ] || fail "exe-sign-tool 未构建"
KEYS="$SPIKE/keys.json"
rm -f "$KEYS" "$SPIKE"/sample*.exe
"$EST" keygen --out "$KEYS" > "$SPIKE/keygen.log" 2>&1 || fail "keygen"
# 显式 --out：Sign 缺省写 {target}.signed 新文件，不原地覆盖
"$EST" sign --keys "$KEYS" ./target/debug/verify-loader.exe --out "$SPIKE/sample.exe" >> "$SPIKE/keygen.log" 2>&1 || fail "sign sample"
# 根证书（公钥部分）提取——S6-2 装根用
node -e "const fs=require('fs');const j=JSON.parse(fs.readFileSync(process.argv[1],'utf8'));fs.writeFileSync(process.argv[2],Buffer.from(j.root_cert,'hex'))" \
    "$(cygpath -w "$KEYS")" "$(cygpath -w "$SPIKE/root.der")" || fail "root.der 提取"
[ -s "$SPIKE/root.der" ] || fail "root.der 为空"
echo "keygen + sign + root.der 提取 OK（详见 $SPIKE/keygen.log）"

echo ""
echo "### [M5] 自方 verify（未篡改样本，期望 Valid）###"
M5_OUT=$("$EST" verify --keys "$KEYS" "$SPIKE/sample.exe" 2>&1)
M5_RC=$?
echo "$M5_OUT" | head -1
[ $M5_RC -eq 0 ] || fail "M5: exe-sign-tool verify 退出码 $M5_RC（期望 0）"
echo "$M5_OUT" | grep -q "Valid" || fail "M5: 输出无 Valid"

echo ""
echo "### [M2] signtool verify /pa（期望失败且唯一错误 = CERT_E_UNTRUSTEDROOT）###"
M2_OUT=$(MSYS_NO_PATHCONV=1 "$SIGNTOOL" verify /pa "$(cygpath -w "$SPIKE/sample.exe")" 2>&1)
M2_RC=$?
# signtool 长错误行会折行（\n\t 缩进续行）——匹配前规范化空白
M2_NORM=$(echo "$M2_OUT" | tr '\r\n\t' '   ' | tr -s ' ')
echo "$M2_OUT" | tail -3
[ $M2_RC -ne 0 ] || fail "M2: signtool verify 竟然成功（默认态应失败）"
echo "$M2_NORM" | grep -q "$UNTRUSTEDROOT_TEXT" || fail "M2: 输出无 UNTRUSTEDROOT 官方文本（D7：默认态唯一失败原因）"
# 唯一错误：不得混入其他缺陷信号（D7 质量线）
echo "$M2_NORM" | grep -qi "HashMismatch\|did not verify\|0x80096010" && fail "M2: 混入 HashMismatch（未篡改样本不应摘要差）"
echo "$M2_NORM" | grep -qi "0x800B010A\|could not be built\|CHAINING" && fail "M2: 混入 CERT_E_CHAINING（证书集缺根信号，S0-4 根因回归）"
echo "$M2_NORM" | grep -qi "0x800B0100\|Invalid" && fail "M2: 混入 NO_SIGNATURE/Invalid 形态缺陷"
ERR_LINES=$(echo "$M2_OUT" | grep -c "SignTool Error" || true)
[ "$ERR_LINES" -le 1 ] || fail "M2: SignTool Error 行数 $ERR_LINES > 1（非唯一错误）"

echo ""
echo "### [M3] Get-AuthenticodeSignature（期望 UnknownError + 0109 官方文本 + 签名者非空）###"
M3_OUT=$(powershell -NoProfile -Command "\$s = Get-AuthenticodeSignature -FilePath '$(cygpath -w "$SPIKE/sample.exe")'; Write-Host ('STATUS=' + \$s.Status); Write-Host ('MSG=' + \$s.StatusMessage); if (\$s.SignerCertificate) { Write-Host ('SIGNER=' + \$s.SignerCertificate.Subject) } else { Write-Host 'SIGNER=' }")
echo "$M3_OUT"
echo "$M3_OUT" | grep -q "STATUS=UnknownError" || fail "M3: Status 非 UnknownError（P0 实测口径）"
echo "$M3_OUT" | grep -qi "MSG=$UNTRUSTEDROOT_TEXT" || fail "M3: StatusMessage 非 0109 官方文本"
echo "$M3_OUT" | grep -Eq "^SIGNER=.+" || fail "M3: SignerCertificate 为空"

echo ""
echo "### [M4] 原内容区篡改 1 字节（期望 微软 HashMismatch 双端 + 自方 Tampered）###"
cp "$SPIKE/sample.exe" "$SPIKE/sample_tampered.exe"
printf '\xFF' | dd of="$SPIKE/sample_tampered.exe" bs=1 seek=32768 count=1 conv=notrunc 2>/dev/null
M4_MS_OUT=$(MSYS_NO_PATHCONV=1 "$SIGNTOOL" verify /pa "$(cygpath -w "$SPIKE/sample_tampered.exe")" 2>&1)
M4_MS_NORM=$(echo "$M4_MS_OUT" | tr '\r\n\t' '   ' | tr -s ' ')
echo "$M4_MS_OUT" | tail -2
echo "$M4_MS_NORM" | grep -qi "$HASHMISMATCH_TEXT\|0x80096010" || fail "M4: signtool 未报 HashMismatch/BAD_DIGEST"
M4_PS_OUT=$(powershell -NoProfile -Command "\$s = Get-AuthenticodeSignature -FilePath '$(cygpath -w "$SPIKE/sample_tampered.exe")'; Write-Host ('STATUS=' + \$s.Status)")
echo "$M4_PS_OUT"
echo "$M4_PS_OUT" | grep -q "STATUS=HashMismatch" || fail "M4: PS Status 非 HashMismatch"
M4_SELF_OUT=$("$EST" verify --keys "$KEYS" "$SPIKE/sample_tampered.exe" 2>&1)
M4_SELF_RC=$?
echo "$M4_SELF_OUT" | head -1
[ $M4_SELF_RC -ne 0 ] || fail "M4: 自方 verify 竟然退出码 0"
echo "$M4_SELF_OUT" | grep -q "Tampered" || fail "M4: 自方输出无 Tampered"

echo ""
echo "### [收尾] 私钥纪律 ###"
rm -f "$SPIKE/keys.json" && echo "keys.json 已删（root.der/sample*.exe 保留：S6-2 装根复跑用）"

echo ""
echo "### S6-1 默认态矩阵：M2/M3/M4/M5 全过（EXIT=0）###"
