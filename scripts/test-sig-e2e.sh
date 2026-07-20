#!/usr/bin/env bash
# 签名验证体系 v3 端到端测试（全闭环回归）
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

step "1. gen-keys + build DLL 固化 root（R7 A1）"
ROOT_PUB=$($VL gen-keys "$KEYS" | grep "^root pubkey" | awk '{print $3}')
echo "root pub: $ROOT_PUB"
NEMESIS_BUILD_ROOT_PUBKEY=$ROOT_PUB cargo build -p nemesis-verify 2>&1 | tail -1

step "2. sign + verify Raw（期望 Valid）"
echo "e2e raw payload v3" > /tmp/sig_target.bin
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

step "6. view 证书链（期望 total=1, cert_count=2）"
$VL view "$DLL" /tmp/sig_http.bin | grep -E "total|cert_count" | head -2

step "7. 吊销 + CRL Revoked（期望 Revoked）"
KEY_FP=$($VL view "$DLL" /tmp/sig_http.bin | grep "key_fp=" | head -1 | sed -E 's/.*key_fp=([0-9a-f]+).*/\1/')
$CURL -s -X POST http://$BIND/v1/admin/revoke -H "Authorization: Bearer $ADMIN" -H "Content-Type: application/json" -d "{\"dim\":\"key_fp\",\"value\":\"$KEY_FP\",\"reason\":\"e2e\"}" >/dev/null
NEMESIS_REVOCATION_URL=http://$BIND $VL verify "$DLL" /tmp/sig_http.bin | head -1

step "8. 多签名叠加（期望 total=2）"
$VL sign "$KEYS" ./target/debug/verify-loader.exe /tmp/sig_pe1.exe >/dev/null
sleep 1
$VL sign "$KEYS" /tmp/sig_pe1.exe /tmp/sig_pe2.exe >/dev/null
$VL view "$DLL" /tmp/sig_pe2.exe | grep total | head -1

step "9. verify-dll 自验（R7 A2，期望 Valid）"
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

step "11. 固化验证：不传 --keys（期望 Valid，纯编译期固化 root）"
unset NEMESIS_ROOT_PUBKEY
$VL verify "$DLL" /tmp/sig_signed.bin | head -1

step "12. exe-sign-tool v3 签 + verify（期望 Valid）"
./target/debug/exe-sign-tool sign --keys "$KEYS" /tmp/sig_target.bin --out /tmp/sig_est.bin >/dev/null
./target/debug/exe-sign-tool verify --keys "$KEYS" /tmp/sig_est.bin | head -1

echo -e "\n### 完成 ###"
