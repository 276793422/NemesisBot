@echo off
REM 盒内多级子进程活动：主 cmd 起子 cmd，子 cmd 再起孙 cmd + ping
set P=C:\AI\NemesisBot\NemesisBot_Rust\test-tools\eval-inject-test\test_dll
echo spawn_feed_start > "%P%\sf_start.txt"
REM 子 cmd（活 ~6 秒，让枚举能观察到）
start /min cmd.exe /c "echo child > %P%\sf_child.txt & ping -n 6 127.0.0.1 > NUL"
REM 孙进程（ping）
ping -n 3 127.0.0.1 > NUL
echo spawn_feed_done > "%P%\sf_done.txt"
