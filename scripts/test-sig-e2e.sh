#!/usr/bin/env bash
# 签名验证体系端到端测试（v4 Authenticode 全链路：verify 管线 S4-1 + 签发
# 单一入口 sign_content_v4 S5-1/S5-3——v3 NMBSIG envelope 已整体退役，
# S5-3 起各步「期望」恢复成立）
# 用法：bash scripts/test-sig-e2e.sh
# 每步打印结果，对照「期望」注释核对。
set -uo pipefail
cd "$(dirname "$0")/.."

CURL=curl.exe
BIND=127.0.0.1:7878
ADMIN=admintok
KEYS=/tmp/sig_e2e_keys.json
DB=/tmp/sig_e2e.db
DLL=./target/debug/nemesis_verify.dll
VL=./target/debug/verify-loader

step() { echo -e "\n### $1 ###"; }
cleanup() { [ -n "${SRV:-}" ] && kill "$SRV" 2>/dev/null; }
trap cleanup EXIT

step "0. build 全部"
cargo build -p nemesis-verify -p revoke-server -p verify-loader -p exe-sign-tool 2>&1 | tail -1

step "1. gen-keys + build DLL 固化根锚（S4-2：值 = 根证书 SHA-256 指纹）"
ROOT_ANCHOR=$($VL gen-keys "$KEYS" | grep "^root anchor" | awk '{print $NF}')
echo "root anchor: $ROOT_ANCHOR"
NEMESIS_BUILD_ROOT_ANCHOR=$ROOT_ANCHOR cargo build -p nemesis-verify 2>&1 | tail -1

step "2. sign + verify Raw（期望 Valid）"
echo "e2e raw payload v4" > /tmp/sig_target.bin
$VL sign "$KEYS" /tmp/sig_target.bin /tmp/sig_signed.bin >/dev/null
$VL verify "$DLL" /tmp/sig_signed.bin | head -1

step "3. 篡改 content（期望 Tampered）"
cp /tmp/sig_signed.bin /tmp/sig_tampered.bin
printf '\xFF' | dd of=/tmp/sig_tampered.bin bs=1 seek=5 count=1 conv=notrunc 2>/dev/null
$VL verify "$DLL" /tmp/sig_tampered.bin | head -1

step "4. 无签名文件（期望 NoSignature）"
$VL verify "$DLL" /tmp/sig_target.bin | head -1

step "5. 服务端 HTTP 签发（user→sign）+ DLL verify（期望 Valid）"
rm -f "$DB"
./target/debug/revoke-server --keys-file "$KEYS" --bind "$BIND" --admin-token "$ADMIN" --db-url "$DB" > /tmp/sig_server.log 2>&1 &
SRV=$!
sleep 2
TOK=$($CURL -s -X POST http://$BIND/v1/admin/user -H "Authorization: Bearer $ADMIN" -H "Content-Type: application/json" -d '{"name":"alice"}' | sed -E 's/.*"token":"([^"]*)".*/\1/')
$CURL -s -X POST http://$BIND/v1/sign -H "Authorization: Bearer $TOK" -F "file=@/tmp/sig_target.bin" -o /tmp/sig_http.bin
$VL verify "$DLL" /tmp/sig_http.bin | head -1

step "6. view 证书链（期望 签名数: 1、信任链 3 行 = leaf→issuing→root 含根）"
$VL view "$DLL" /tmp/sig_http.bin | grep -E "签名数|^\s+\[[0-9]+\]"

step "7. 吊销 + CRL Revoked（期望 Revoked）"
KEY_FP=$($VL verify "$DLL" /tmp/sig_http.bin | grep -oE "key_fp=[0-9a-f]+" | head -1 | cut -d= -f2)
echo "key_fp: $KEY_FP"
$CURL -s -X POST http://$BIND/v1/admin/revoke -H "Authorization: Bearer $ADMIN" -H "Content-Type: application/json" -d "{\"dim\":\"key_fp\",\"value\":\"$KEY_FP\",\"reason\":\"e2e\"}" >/dev/null
NEMESIS_REVOCATION_URL=http://$BIND $VL verify "$DLL" /tmp/sig_http.bin | head -1

step "8. 已签名 PE 二次追表（v4 诚实拒绝：期望 sign 失败 + 原文件 verify 仍 Valid；多签名走 CMS 嵌套 SPC_NESTED_SIGNATURE，非二次追表）"
$VL sign "$KEYS" ./target/debug/verify-loader.exe /tmp/sig_pe1.exe >/dev/null
if $VL sign "$KEYS" /tmp/sig_pe1.exe /tmp/sig_pe2.exe >/dev/null 2>&1; then
  echo "FAIL: 已签名 PE 竟被二次追表（应诚实拒绝）"
else
  echo "✓ 二次追表被诚实拒绝（append_certificate_table: 已存在证书表）"
fi
$VL verify "$DLL" /tmp/sig_pe1.exe | head -1

step "9. verify-dll 自验（R7 A2，期望 Valid——S4-5 承诺：v4 签出的 DLL 过 nv_self_verify）"
$VL sign "$KEYS" "$DLL" /tmp/sig_dll.dll >/dev/null
$VL verify-dll /tmp/sig_dll.dll

step "10. OCSP fallback：CRL 故障 + strict（期望 Revoked）"
kill "$SRV" 2>/dev/null; sleep 1
rm -f "$DB"
NEMESIS_DEBUG_CRL_500=1 ./target/debug/revoke-server --keys-file "$KEYS" --bind "$BIND" --admin-token "$ADMIN" --db-url "$DB" > /tmp/sig_server.log 2>&1 &
SRV=$!
sleep 2
$CURL -s -X POST http://$BIND/v1/admin/revoke -H "Authorization: Bearer $ADMIN" -H "Content-Type: application/json" -d "{\"dim\":\"key_fp\",\"value\":\"$KEY_FP\",\"reason\":\"ocsp\"}" >/dev/null
NEMESIS_REVOCATION_URL=http://$BIND NEMESIS_STRICT_OFFLINE=1 $VL verify "$DLL" /tmp/sig_http.bin | head -1

step "11. 固化验证：不传 --keys（期望 Valid，纯编译期固化根锚）"
unset NEMESIS_ROOT_ANCHOR
$VL verify "$DLL" /tmp/sig_signed.bin | head -1

step "12. exe-sign-tool v4 签 + verify（期望 Valid）"
./target/debug/exe-sign-tool sign --keys "$KEYS" /tmp/sig_target.bin --out /tmp/sig_est.bin >/dev/null
./target/debug/exe-sign-tool verify --keys "$KEYS" /tmp/sig_est.bin | head -1

echo -e "\n### 完成 ###"
