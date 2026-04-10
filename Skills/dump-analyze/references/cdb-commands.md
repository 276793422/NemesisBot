# CDB Command Reference for Dump Analysis

## Basic Commands

| Command | Description |
|---------|-------------|
| `!analyze -v` | Verbose crash analysis (most important command) |
| `!analyze -hang` | Analyze hang (non-crash) dumps |
| `k` | Print current thread call stack |
| `kPN <n>` | Print call stack with frame numbers, max n frames |
| `~` | List all threads |
| `~*k` | Print call stacks for all threads |
| `~<n>s` | Switch to thread n |
| `r` | Display registers |
| `q` | Quit debugger |

## Symbol Commands

| Command | Description |
|---------|-------------|
| `.sympath <path>` | Set symbol search path |
| `.sympath+ <path>` | Append to symbol path |
| `.symfix` | Set default MS symbol server path |
| `.symfix+ <cache>` | Set MS symbol server with local cache |
| `.reload` | Reload all symbols |
| `.reload /f <module>` | Force reload specific module symbols |
| `lm` | List loaded modules |
| `lmv` | List loaded modules (verbose) |
| `lmv m <module>` | Detailed info for specific module |

## Memory Commands

| Command | Description |
|---------|-------------|
| `!address <addr>` | Query memory region information |
| `db <addr>` | Display memory as bytes |
| `dd <addr>` | Display memory as DWORDs |
| `du <addr>` | Display memory as Unicode string |
| `da <addr>` | Display memory as ASCII string |
| `!heap` | Heap information |

## Exception Analysis Commands

| Command | Description |
|---------|-------------|
| `.exr -1` | Display most recent exception record |
| `.ecxr` | Switch to exception context |
| `!gle` | Get last error for current thread |
| `!peb` | Display process environment block |
| `!teb` | Display thread environment block |

## CDB Command Line Options

```
cdb -y <sympath> -z <dumppath> -c "<commands>"
```

| Option | Description |
|--------|-------------|
| `-y <path>` | Symbol search path |
| `-z <path>` | Dump file path |
| `-c "<cmds>"` | Commands to execute on startup |
| `-cqr "<cmds>"` | Execute commands and quit (remote) |
| `-p <pid>` | Attach to live process |

## Symbol Path Syntax

```
# Local directory
C:\path\to\pdb

# Microsoft symbol server with cache
SRV*C:\sym_cache*http://msdl.microsoft.com/download/symbols

# Multiple paths (semicolon separated)
SRV*C:\cache*http://msdl.microsoft.com/download/symbols;C:\local\symbols;C:\project\Debug_x64
```

## Pipeline Pattern

For automated analysis, use this command pattern:

```powershell
& cdb.exe -y $symbolPath -z $dumpPath -c "!analyze -v; kPN 50; q"
```

The `q` at the end ensures CDB exits after processing.
