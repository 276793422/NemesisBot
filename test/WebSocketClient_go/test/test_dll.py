"""
Test wsclient.dll via ctypes
Tests: WSC_Init, WSC_Send, WSC_Recv, WSC_Destroy
"""
import ctypes
import os
import sys
import time
import json

DLL_PATH = os.path.join(os.path.dirname(__file__), "..", "wsclient.dll")
DLL_PATH = os.path.abspath(DLL_PATH)

WS_URL = b"ws://127.0.0.1:49999/ws"
TIMEOUT_MS = 5000

passed = 0
failed = 0

def check(name, condition, detail=""):
    global passed, failed
    if condition:
        print(f"  [PASS] {name}")
        passed += 1
    else:
        print(f"  [FAIL] {name} {detail}")
        failed += 1


def main():
    global passed, failed

    print(f"DLL path: {DLL_PATH}")
    print(f"DLL exists: {os.path.exists(DLL_PATH)}")
    print()

    # Load DLL
    print("=== Loading DLL ===")
    dll = ctypes.CDLL(DLL_PATH)
    check("Load DLL", dll is not None)
    print()

    # --- Test 1: WSC_Init ---
    print("=== Test 1: WSC_Init ===")
    ret = dll.WSC_Init(WS_URL, b"")
    check("WSC_Init returns 0", ret == 0, f"got {ret}")
    time.sleep(0.5)  # wait for connection + welcome message
    print()

    # --- Test 2: WSC_Recv welcome message ---
    print("=== Test 2: WSC_Recv (welcome message) ===")
    buf = ctypes.create_string_buffer(4096)
    ret = dll.WSC_Recv(buf, 4096, TIMEOUT_MS)
    check("WSC_Recv returns > 0", ret > 0, f"got {ret}")

    if ret > 0:
        msg_str = buf.value.decode("utf-8")
        check("Recv is valid JSON", True)
        try:
            msg = json.loads(msg_str)
            check("Message type is 'message'", msg.get("type") == "message", f"got {msg.get('type')}")
            check("Message role is 'system'", msg.get("role") == "system", f"got {msg.get('role')}")
            check("Content contains 'Connected'", "Connected" in msg.get("content", ""),
                   f"got '{msg.get('content', '')[:80]}'")
            print(f"    Content: {msg.get('content', '')[:80]}")
        except json.JSONDecodeError as e:
            check("Recv JSON parse", False, str(e))
    print()

    # --- Test 3: WSC_Send ---
    print("=== Test 3: WSC_Send ===")
    ret = dll.WSC_Send(b"Hello Bot")
    check("WSC_Send returns 0", ret == 0, f"got {ret}")
    print()

    # --- Test 4: WSC_Recv echo response ---
    print("=== Test 4: WSC_Recv (echo response) ===")
    buf = ctypes.create_string_buffer(4096)
    ret = dll.WSC_Recv(buf, 4096, TIMEOUT_MS)
    check("WSC_Recv returns > 0", ret > 0, f"got {ret}")

    if ret > 0:
        msg_str = buf.value.decode("utf-8")
        try:
            msg = json.loads(msg_str)
            check("Message role is 'assistant'", msg.get("role") == "assistant",
                   f"got {msg.get('role')}")
            check("Content contains 'Echo: Hello Bot'",
                   "Echo: Hello Bot" in msg.get("content", ""),
                   f"got '{msg.get('content', '')[:80]}'")
            print(f"    Content: {msg.get('content', '')}")
        except json.JSONDecodeError as e:
            check("Recv JSON parse", False, str(e))
    print()

    # --- Test 5: WSC_Recv timeout ---
    print("=== Test 5: WSC_Recv (timeout, no message) ===")
    buf = ctypes.create_string_buffer(4096)
    ret = dll.WSC_Recv(buf, 4096, 1000)  # 1 second timeout
    check("WSC_Recv returns 0 (timeout)", ret == 0, f"got {ret}")
    print()

    # --- Test 6: Multiple send/recv ---
    print("=== Test 6: Multiple send/recv ===")
    for i in range(3):
        content = f"Message {i+1}"
        ret = dll.WSC_Send(content.encode("utf-8"))
        check(f"Send '{content}'", ret == 0, f"got {ret}")
        time.sleep(0.1)

        buf = ctypes.create_string_buffer(4096)
        ret = dll.WSC_Recv(buf, 4096, TIMEOUT_MS)
        if ret > 0:
            msg = json.loads(buf.value.decode("utf-8"))
            check(f"Recv echo for '{content}'",
                   f"Echo: {content}" in msg.get("content", ""),
                   f"got '{msg.get('content', '')}'")
        else:
            check(f"Recv echo for '{content}'", False, f"ret={ret}")
    print()

    # --- Test 7: WSC_Destroy ---
    print("=== Test 7: WSC_Destroy ===")
    dll.WSC_Destroy()
    check("WSC_Destroy completed", True)
    print()

    # --- Test 8: Send after destroy ---
    print("=== Test 8: WSC_Send after destroy ===")
    ret = dll.WSC_Send(b"should fail")
    check("WSC_Send returns -1 (not initialized)", ret == -1, f"got {ret}")
    print()

    # --- Test 9: Recv after destroy ---
    print("=== Test 9: WSC_Recv after destroy ===")
    buf = ctypes.create_string_buffer(4096)
    ret = dll.WSC_Recv(buf, 4096, 500)
    check("WSC_Recv returns -1 (not initialized)", ret == -1, f"got {ret}")
    print()

    # --- Test 10: Re-init after destroy ---
    print("=== Test 10: WSC_Init (reconnect after destroy) ===")
    ret = dll.WSC_Init(WS_URL, b"")
    check("WSC_Init reconnect returns 0", ret == 0, f"got {ret}")
    time.sleep(0.5)

    buf = ctypes.create_string_buffer(4096)
    ret = dll.WSC_Recv(buf, 4096, TIMEOUT_MS)
    check("Recv welcome after reconnect", ret > 0, f"got {ret}")
    if ret > 0:
        msg = json.loads(buf.value.decode("utf-8"))
        check("Welcome content correct", "Connected" in msg.get("content", ""),
               f"got '{msg.get('content', '')[:80]}'")
    print()

    # Final destroy
    dll.WSC_Destroy()
    print("========================================")
    print(f"Results: {passed} passed, {failed} failed, {passed+failed} total")
    print("========================================")

    return 0 if failed == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
