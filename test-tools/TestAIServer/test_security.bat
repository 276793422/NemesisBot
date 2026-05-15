@echo off
REM TestAIServer 测试脚本 - testai-5.0 安全文件操作模型
REM 用于测试文件操作和安全审批功能

echo ========================================
echo TestAIServer - testai-5.0 测试
echo ========================================
echo.

REM 启动服务器（新窗口）
start "TestAIServer" testaiserver.exe

REM 等待服务器启动
echo 等待服务器启动...
timeout /t 3 /nobreak > nul

echo.
echo ========================================
echo 测试 1: 列出所有模型
echo ========================================
echo.
curl -s http://localhost:8080/v1/models
echo.
echo.

timeout /t 2 /nobreak > nul

echo ========================================
echo 测试 2: file_delete 操作（CRITICAL 风险）
echo ========================================
echo.
curl -s -X POST http://localhost:8080/v1/chat/completions ^
  -H "Content-Type: application/json" ^
  -d "{\"model\":\"testai-5.0\",\"messages\":[{\"role\":\"user\",\"content\":\"<FILE_OP>{\\\"operation\\\":\\\"file_delete\\\",\\\"path\\\":\\\"/etc/passwd\\\",\\\"risk_level\\\":\\\"CRITICAL\\\"}</FILE_OP>\"}]}"
echo.
echo.

timeout /t 2 /nobreak > nul

echo ========================================
echo 测试 3: file_read 操作（HIGH 风险）
echo ========================================
echo.
curl -s -X POST http://localhost:8080/v1/chat/completions ^
  -H "Content-Type: application/json" ^
  -d "{\"model\":\"testai-5.0\",\"messages\":[{\"role\":\"user\",\"content\":\"<FILE_OP>{\\\"operation\\\":\\\"file_read\\\",\\\"path\\\":\\\"/etc/passwd\\\",\\\"risk_level\\\":\\\"HIGH\\\"}</FILE_OP>\"}]}"
echo.
echo.

timeout /t 2 /nobreak > nul

echo ========================================
echo 测试 4: file_write 操作（MEDIUM 风险）
echo ========================================
echo.
curl -s -X POST http://localhost:8080/v1/chat/completions ^
  -H "Content-Type: application/json" ^
  -d "{\"model\":\"testai-5.0\",\"messages\":[{\"role\":\"user\",\"content\":\"<FILE_OP>{\\\"operation\\\":\\\"file_write\\\",\\\"path\\\":\\\"/tmp/test.txt\\\",\\\"content\\\":\\\"Hello World\\\",\\\"risk_level\\\":\\\"MEDIUM\\\"}</FILE_OP>\"}]}"
echo.
echo.

timeout /t 2 /nobreak > nul

echo ========================================
echo 测试 5: dir_create 操作（LOW 风险）
echo ========================================
echo.
curl -s -X POST http://localhost:8080/v1/chat/completions ^
  -H "Content-Type: application/json" ^
  -d "{\"model\":\"testai-5.0\",\"messages\":[{\"role\":\"user\",\"content\":\"<FILE_OP>{\\\"operation\\\":\\\"dir_create\\\",\\\"path\\\":\\\"/tmp/testdir\\\",\\\"risk_level\\\":\\\"LOW\\\"}</FILE_OP>\"}]}"
echo.
echo.

timeout /t 2 /nobreak > nul

echo ========================================
echo 测试完成！
echo ========================================
echo.
echo 说明：
echo - 所有测试都返回工具调用（tool_calls）
echo - 可以用来测试 NemesisBot 的安全审批功能
echo - 不同风险级别会触发不同的安全策略
echo.
echo 在 NemesisBot 中使用：
echo nemesisbot model add --model test/testai-5.0 --base http://127.0.0.1:8080/v1 --key test-key --default
echo.
pause
