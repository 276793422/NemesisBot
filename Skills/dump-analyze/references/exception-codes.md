# Common Windows Exception Codes

## Frequently Seen in Crash Dumps

| Code | Name | Description |
|------|------|-------------|
| `0x80000003` | `STATUS_BREAKPOINT` | Software breakpoint (`int 3` / `__debugbreak()`) |
| `0x40010006` | `DBG_PRINTEXCEPTION_C` | `OutputDebugString` / CRT debug output. Non-fatal, captured by VEH. |
| `0x40010005` | `DBG_PRINTEXCEPTION_WIDE_C` | Wide-char version of debug output exception |
| `0xC0000005` | `STATUS_ACCESS_VIOLATION` | Null pointer dereference, use-after-free, buffer overflow. **Most common real crash.** |
| `0xC0000008` | `STATUS_INVALID_HANDLE` | Invalid handle used in API call |
| `0xC000001D` | `STATUS_ILLEGAL_INSTRUCTION` | Executed invalid CPU instruction |
| `0xC0000094` | `STATUS_INTEGER_DIVIDE_BY_ZERO` | Integer division by zero |
| `0xC0000096` | `STATUS_PRIVILEGED_INSTRUCTION` | Privileged instruction in user mode |
| `0xC00000FD` | `STATUS_STACK_OVERFLOW` | Stack overflow (infinite recursion) |
| `0xC0000135` | `STATUS_DLL_NOT_FOUND` | Required DLL not found |
| `0xC0000139` | `STATUS_ENTRYPOINT_NOT_FOUND` | DLL entry point not found |
| `0xC0000142` | `STATUS_DLL_INIT_FAILED` | DLL initialization failed |
| `0xC0000374` | `STATUS_HEAP_CORRUPTION` | Heap corruption detected |
| `0xC0000409` | `STATUS_STACK_BUFFER_OVERRUN` | Stack buffer overrun (security cookie) |
| `0xC000041D` `STATUS_INVALID_CRUNTIME_PARAMETER` | Invalid CRT parameter passed |
| `0xE06D7363` | `CPP_EXCEPTION` | C++ exception thrown (`0xE06D7363` = "msc") |
| `0x80000004` | `STATUS_SINGLE_STEP` | Single step (debugging) |

## SmartConnect-Specific Analysis

### Exception Code 0x40010006 (DBG_PRINTEXCEPTION_C)

This is the most common code seen in SmartConnect crash dumps. Key characteristics:

- **Not a real crash** — It's triggered by `OutputDebugStringA()` calls
- **Captured by VEH** — The `DiagnosticManager` installs a Vectored Exception Handler via `AddVectoredExceptionHandler(1, ExceptionFilter)` which generates a dump for EVERY exception
- **High volume** — Each `LogToDebug()` call in WinAPI.dll uses `OutputDebugStringA()`, which triggers this exception code
- **During DLL unload** — Most commonly seen during `DLL_PROCESS_DETACH` when `SDK_Stop()` → `TriggerBroadcastCallback()` → `LogToDebug()` is called

### Exception Code 0xC0000005 (Access Violation)

If this code appears, it's a **real crash**:
- Check if accessing already-freed memory (use-after-free)
- Common in `StaticOnBroadcast()` when `g_pServer` or `m_pipeEndpoint` is null/freed
- Look for null pointer dereferences in callback chains

### How to Interpret the Exception Address

The exception address tells you where in memory the crash occurred:

```
Exception Address: 0x00007FFB355D73FA
```

1. Use `lmv` to find module base addresses
2. Calculate offset: `address - module_base = RVA`
3. Match RVA against the module's PDB symbols

Example:
```
WinAPI.dll base: 0x00007FFB355C0000
Exception addr:  0x00007FFB355D73FA
RVA:             0x00000000000173FA
→ Look up 0x173FA in WinAPI.pdb
```

## Reading CDB !analyze -v Output

Key fields to look for:

```
FAULTING_IP:             ← The instruction that caused the crash
Probably caused by:      ← CDB's best guess at the faulting module
DEFAULT_BUCKET_ID:       ← Crash classification
PROCESS_NAME:            ← Which process crashed
EXCEPTION_CODE:          ← The exception code (see table above)
EXCEPTION_PARAMETER1:    ← For AV: 0=read, 1=write
EXCEPTION_PARAMETER2:    ← For AV: the address that was accessed
FAULTING_THREAD:         ← Thread that crashed
STACK_TEXT:              ← Full call stack with parameters
```
