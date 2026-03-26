---
name: wsl-operations
description: This skill should be used when the user asks to "run WSL commands", "execute Linux on Windows", "manage WSL processes", "check WSL status", "transfer files between Windows and WSL", or mentions WSL-related operations like "wsl", "ubuntu", "debian in WSL", "compile in WSL", "wsl bash", "wsl script".
version: 1.0.0
---

# WSL Operations Skill

## Overview

This skill provides comprehensive guidance for performing Linux operations through Windows Subsystem for Linux (WSL). It covers command execution, process management, file operations, system monitoring, and WSL instance management.

## When This Skill Applies

Use this skill when users need to:
- Execute Linux commands from Windows
- Run shell scripts in WSL
- Monitor WSL system resources (CPU, memory, disk)
- Manage processes and services in WSL
- Transfer files between Windows and WSL
- Manage WSL instances (start, stop, restart)
- Perform development tasks in a Linux environment
- Compile or build projects in WSL

---

## Quick Start

### 1. Execute Commands in WSL

Execute a single command:
```bash
wsl bash -lic "ls -la /home"
```

Run a shell script:
```bash
wsl bash -lic "./build.sh"
```

Execute with environment variables:
```bash
wsl bash -lic "NODE_ENV=production node server.js"
```

### 2. Check WSL Status

List WSL distributions:
```bash
wsl --list --verbose
```

Check system resources:
```bash
wsl bash -lic "top -b -n 1"
wsl bash -lic "df -h"
wsl bash -lic "free -h"
```

### 3. Manage Processes

Find processes:
```bash
wsl bash -lic "ps aux | grep nginx"
wsl bash -lic "lsof -i :8080"
```

Terminate processes:
```bash
wsl bash -lic "pkill -f nginx"
wsl bash -lic "kill -9 1234"
```

Manage services:
```bash
wsl bash -lic "systemctl status nginx"
wsl bash -lic "systemctl restart nginx"
```

### 4. Path Conversion

Windows to WSL:
```bash
wsl bash -lic "wslpath -u 'C:\\Users\\username'"
# Output: /mnt/c/Users/username
```

WSL to Windows:
```bash
wsl bash -lic "wslpath -w '/mnt/c/Users/username'"
# Output: C:\Users\username
```

### 5. WSL Management

Terminate specific distribution:
```bash
wsl --terminate Ubuntu-22.04
```

Shutdown all WSL instances:
```bash
wsl --shutdown
```

---

## Command Execution

### Basic Command Format

Use `wsl bash -lic` for most operations:
- `bash` - Use bash shell
- `-l` - Login shell (loads ~/.bash_profile, ~/.bashrc)
- `-i` - Interactive mode (optional)
- `-c "command"` - Command to execute

### Examples

**File Operations:**
```bash
wsl bash -lic "pwd"                    # Print working directory
wsl bash -lic "cd /home && ls -la"     # Change and list
wsl bash -lic "mkdir -p /tmp/test"      # Create directory
wsl bash -lic "cat file.txt"            # View file
wsl bash -lic "tail -f logfile.log"      # Follow log file
```

**Package Management:**
```bash
wsl bash -lic "apt update"              # Update package list
wsl bash -lic "apt install -y git"       # Install package
wsl bash -lic "apt upgrade -y"           # Upgrade packages
wsl bash -lic "dpkg -l | grep python"    # Check installed package
```

**Development Commands:**
```bash
# Git
wsl bash -lic "git clone https://github.com/user/repo.git"
wsl bash -lic "git pull"
wsl bash -lic "git log --oneline -10"

# Python
wsl bash -lic "pip3 install package"
wsl bash -lic "python3 -m venv venv"

# Node.js
wsl bash -lic "npm install"
wsl bash -lic "npm run build"
```

---

## System Monitoring

### Process Monitoring

View all processes:
```bash
wsl bash -lic "ps aux"
```

View top processes:
```bash
wsl bash -lic "top -b -n 1"
wsl bash -lic "htop"
```

Find processes by name:
```bash
wsl bash -lic "pgrep -af nginx"
wsl bash -lic "pstree -p"
```

### Resource Monitoring

Check memory:
```bash
wsl bash -lic "free -h"
wsl bash -lic "cat /proc/meminfo"
```

Check disk usage:
```bash
wsl bash -lic "df -h"
wsl bash -lic "du -sh /path/to/dir"
```

Check network:
```bash
wsl bash -lic "ip addr"
wsl bash -lic "netstat -tulpn"
wsl bash -lic "ss -tulpn"
```

### System Information

Show system details:
```bash
wsl bash -lic "uname -a"
wsl bash -lic "hostname"
wsl bash -lic "uptime"
wsl bash -lic "whoami"
```

---

## Process Management

### Viewing Processes

**List all processes:**
```bash
wsl bash -lic "ps aux"
wsl bash -lic "ps aux | head -20"      # Top 20 processes
```

**Find specific processes:**
```bash
wsl bash -lic "ps aux | grep nginx"
wsl bash -lic "pgrep -f nginx"
wsl bash -lic "pgrep -af nginx"      # Include process tree
```

**Check port usage:**
```bash
wsl bash -lic "lsof -i :8080"
wsl bash -lic "netstat -tulpn | grep :8080"
```

### Terminating Processes

**Terminate by name:**
```bash
wsl bash -lic "pkill -f nginx"        # Kill process by name
wsl bash -lit "killall nginx"         # Kill all by name
```

**Terminate by PID:**
```bash
wsl bash -lic "kill 1234"             # Graceful termination
wsl bash -lic "kill -9 1234"           # Force kill
```

### Service Management

**Systemctl commands:**
```bash
wsl bash -lic "systemctl status nginx"
wsl bash -lic "systemctl start nginx"
wsl bash -lic "systemctl stop nginx"
wsl bash -lic "systemctl restart nginx"
wsl bash -lic "systemctl enable nginx"
wsl bash -lic "systemctl disable nginx"
```

**View all services:**
```bash
wsl bash -lic "systemctl list-units --type=service"
wsl bash -lic "service --status-all"
```

---

## File Operations

### Path Conversion

**Windows to WSL:**
```bash
# In WSL - Windows paths appear as /mnt/c/
ls /mnt/c/Users/username/Documents

# Convert Windows path to WSL path
wsl bash -lic "wslpath -u 'C:\\Users\\username\\Documents'"
```

**WSL to Windows:**
```bash
# Access WSL files from Windows
# In Explorer: \\wsl$\Ubuntu\home\username

# Convert WSL path to Windows path
wsl bash -lic "wslpath -w '/home/username/file.txt'"
```

### File Transfer

**From Windows to WSL:**
```bash
# Copy file to WSL home
wsl bash -lic "cp /mnt/c/Users/username/file.txt ~/"
```

**From WSL to Windows:**
```bash
# Copy file to Windows
wsl bash -lic "cp file.txt /mnt/c/Users/username/"
```

### VS Code Integration

**Open WSL files in VS Code:**
```bash
code /mnt/c/path/to/file            # Open specific file
code /mnt/c/path/to/directory      # Open directory
```

---

## WSL Instance Management

### View WSL Status

**List distributions:**
```bash
wsl --list                        # Simple list
wsl --list --verbose             # Detailed list
wsl --status                     # WSL status
```

### Control WSL Instances

**Terminate distribution:**
```bash
wsl --terminate Ubuntu-22.04
```

**Shutdown all WSL:**
```bash
wsl --shutdown
```

**Set default distribution:**
```bash
wsl --set-default Ubuntu-22.04
```

**Update WSL:**
```bash
wsl --update
```

---

## Additional Resources

### Reference Files
- `references/commands.md` - Complete WSL command reference
- `references/path-conversion.md` - Detailed path conversion guide
- `references/troubleshooting.md` - Common issues and solutions

### Example Files
- `examples/basic-commands.md` - Basic command examples
- `examples/process-management.md` - Process management scenarios
- `examples/file-transfer.md` - File transfer operations

### Scripts
- `scripts/wsl-run.sh` - Command execution helper
- `scripts/wsl-ps.sh` - Process viewing helper
