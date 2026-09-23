#!/usr/bin/env python3
"""慢速 mock LLM（OpenAI /v1/chat/completions 兼容，测试专用）。

用途：项目会话历史修复回归（scripts/verify-history-fix.mjs 场景 B）需要
一个「占用项目 loop 确定性时长」的上游——真模型不可控，本 mock 固定延迟
后才回非流式 JSON（gateway http_provider 对 application/json 完整体有
内置兼容路径，合成单 delta 流，无需实现 SSE）。

用法：python scripts/mock_slow_llm.py [port] [delay_secs]
默认 127.0.0.1:18099，延迟 45s。响应内容固定，无工具调用。
"""
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, HTTPServer

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 18099
DELAY = float(sys.argv[2]) if len(sys.argv) > 2 else 45.0


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):  # 静默默认访问日志
        pass

    def do_POST(self):
        if not self.path.startswith("/v1/chat/completions"):
            self._json(404, {"error": {"message": "not found"}})
            return
        length = int(self.headers.get("Content-Length") or 0)
        if length:
            self.rfile.read(length)
        time.sleep(DELAY)
        self._json(
            200,
            {
                "id": "chatcmpl-mockslow",
                "object": "chat.completion",
                "model": "mock-slow",
                "choices": [
                    {
                        "index": 0,
                        "message": {"role": "assistant", "content": "收到（mock 慢回复）"},
                        "finish_reason": "stop",
                    }
                ],
                "usage": {
                    "prompt_tokens": 10,
                    "completion_tokens": 5,
                    "total_tokens": 15,
                },
            },
        )

    def do_GET(self):
        self._json(200, {"object": "list", "data": [{"id": "mock-slow"}]})

    def _json(self, code, obj):
        body = json.dumps(obj).encode("utf-8")
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


if __name__ == "__main__":
    srv = HTTPServer(("127.0.0.1", PORT), Handler)
    print(f"mock-slow-llm listening on 127.0.0.1:{PORT} delay={DELAY}s", flush=True)
    srv.serve_forever()
