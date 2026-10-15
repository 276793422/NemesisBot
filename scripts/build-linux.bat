@echo off
REM S8（2026-10-14 审计）：CWD 锚定 + 参数透传——Explorer 双击 / 非 repo 根
REM 启动时 "./scripts/build-wsl.sh" 相对路径解析失败（bash 127）；%* 透传
REM --clean / --skip-plugin 等旗标给 build-wsl.sh。
cd /d "%~dp0\.."
cmd /c wsl bash -lic "./scripts/build-wsl.sh %*"
