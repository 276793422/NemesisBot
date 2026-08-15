@echo off
set P=C:\AI\NemesisBot\NemesisBot_Rust\test-tools\eval-inject-test\test_dll
echo a1 > "%P%\f1.txt"
echo a2 > "%P%\f2.txt"
echo a3 > "%P%\f3.txt"
type "C:\Windows\System32\drivers\etc\hosts" > "%P%\hosts.txt" 2>nul
echo FEED_DONE > "%P%\feed_done.txt"
