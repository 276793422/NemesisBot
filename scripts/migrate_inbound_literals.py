#!/usr/bin/env python3
"""msg_class 批迁移：InboundMessage 字面量 → user_ingress 构造器。

- 词边界匹配（不碰 BusInboundMessage）
- 字符串/char 感知的括号平衡解析
- copy-shape 字面量（逐字段 x.f.clone()）跳过 → 手工处理
- 缺省值字段（Vec::new()/String::new()/HashMap::new()/None）不产 builder
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# 手工迁移清单（生产者/直塞门/copy-shape/已处理）
MANUAL_FILES = {
    "crates/nemesis-types/src/channel.rs",
    "crates/nemesis-types/src/channel/tests.rs",
    "crates/nemesis-agent/src/loop/bus.rs",
    "crates/nemesis-agent/src/loop/admission.rs",
    "crates/nemesis-agent/src/loop/run_loop.rs",
    "crates/nemesis-agent/src/loop/client_spawn.rs",
    "crates/nemesis-agent/src/loop/admission/cov_tests.rs",
    "crates/nemesis-agent/src/loop/l22_snooze_gate_tests.rs",
    "crates/nemesis-agent/src/loop/bus_cov_tests.rs",
    "nemesisbot/src/commands/gateway/post_agent.rs",
    "nemesisbot/src/goals_engine.rs",
    "nemesisbot/src/commands/gateway/ctx.rs",
    "nemesisbot/src/local_dispatch.rs",
    "nemesisbot/src/commands/gateway/cluster_init.rs",
    "nemesisbot/src/adapters.rs",
    "nemesisbot/src/agent_factory/shared.rs",
    "nemesisbot/src/agent_factory/project.rs",
    "nemesisbot/src/commands/gateway/bridges.rs",
    "nemesisbot/src/adapters/tests.rs",  # adapters 手工（continuation 相邻面）
    "crates/nemesis-cluster/src/cluster/tests.rs",
    "crates/nemesis-cluster/src/cluster/tasks.rs",
    "crates/nemesis-cluster/src/cluster/mod.rs",
}

DEFAULTS = {
    "media": ("Vec::new()", "vec![]"),
    "session_key": ("String::new()",),
    "correlation_id": ("String::new()",),
    "metadata": (
        "std::collections::HashMap::new()",
        "HashMap::new()",
        "Default::default()",
    ),
    "voice_playback": ("None",),
}

LIT = re.compile(r"(?<![\w])([A-Za-z_:][\w:]*)?InboundMessage \{")


def skip_string(s, i):
    """i at quote; return index after closing quote."""
    q = s[i]
    i += 1
    while i < len(s):
        c = s[i]
        if c == "\\":
            i += 2
            continue
        if c == q:
            # raw string r#"..."# handling not needed for these files
            return i + 1
        i += 1
    return i


def find_close(s, start):
    """start at '{'; return index of matching '}' (string-aware)."""
    depth = 0
    i = start
    while i < len(s):
        c = s[i]
        if c in "\"'":
            i = skip_string(s, i)
            continue
        if c == "/" and i + 1 < len(s) and s[i + 1] == "/":
            j = s.find("\n", i)
            i = j if j != -1 else len(s)
            continue
        if c == "/" and i + 1 < len(s) and s[i + 1] == "*":
            j = s.find("*/", i + 2)
            i = j + 2 if j != -1 else len(s)
            continue
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    raise ValueError("unbalanced")


def split_top(body):
    """Split struct-literal body on top-level commas → list of 'name: expr'."""
    parts = []
    depth = 0
    last = 0
    i = 0
    while i < len(body):
        c = body[i]
        if c in "\"'":
            i = skip_string(body, i)
            continue
        if c == "/" and i + 1 < len(body) and body[i + 1] == "/":
            j = body.find("\n", i)
            i = j if j != -1 else len(body)
            continue
        if c == "/" and i + 1 < len(body) and body[i + 1] == "*":
            j = body.find("*/", i + 2)
            i = j + 2 if j != -1 else len(body)
            continue
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
        elif c == "," and depth == 0:
            parts.append(body[last:i])
            last = i + 1
        i += 1
    tail = body[last:].strip()
    if tail:
        parts.append(tail)
    return parts


FIELD_RE = re.compile(r"^\s*([a-z_][a-z0-9_]*)\s*:\s*(.*)$", re.S)
SHORTHAND_RE = re.compile(r"^\s*([a-z_][a-z0-9_]*)\s*$")

COPY_RE = re.compile(r"^\s*[A-Za-z_][\w_]*(\.[a-z_][a-z0-9_]*)+(\.clone\(\))?\s*$")


def parse_literal(body):
    fields = {}
    for part in split_top(body):
        text = part.strip()
        m = FIELD_RE.match(text)
        if m:
            fields[m.group(1)] = m.group(2).strip()
            continue
        sh = SHORTHAND_RE.match(text)
        if sh:
            fields[sh.group(1)] = sh.group(1)
            continue
        return None  # unexpected shape
    expected = {
        "channel", "sender_id", "chat_id", "content", "media",
        "session_key", "correlation_id", "metadata", "voice_playback",
    }
    if set(fields) != expected:
        return None
    return fields


def is_copy_shape(fields):
    return all(COPY_RE.match(v) for v in fields.values())


def render(fields, qual, indent):
    q = f"{qual}InboundMessage" if qual else "InboundMessage"
    lines = [f"{q}::user_ingress({fields['channel']}, {fields['sender_id']}, {fields['chat_id']}, {fields['content']})"]
    b = indent + "    "
    if fields["media"].strip() not in DEFAULTS["media"]:
        lines.append(f"{b}.with_media({fields['media']})")
    if fields["session_key"].strip() not in DEFAULTS["session_key"]:
        lines.append(f"{b}.with_session_key({fields['session_key']})")
    if fields["correlation_id"].strip() not in DEFAULTS["correlation_id"]:
        lines.append(f"{b}.with_correlation_id({fields['correlation_id']})")
    if fields["metadata"].strip() not in DEFAULTS["metadata"]:
        lines.append(f"{b}.with_metadata_map({fields['metadata']})")
    if fields["voice_playback"].strip() not in DEFAULTS["voice_playback"]:
        lines.append(f"{b}.with_voice_playback({fields['voice_playback']})")
    return "\n".join(lines)


def process(path):
    src = path.read_text(encoding="utf-8")
    out = []
    pos = 0
    n = 0
    skipped_copy = 0
    while True:
        m = LIT.search(src, pos)
        if not m:
            out.append(src[pos:])
            break
        # 跳过 fn 定义（fn f() -> InboundMessage {）/impl 块（impl InboundMessage {）
        before = src[: m.start()].rstrip()
        if before.endswith("->") or before.endswith("impl"):
            out.append(src[pos : m.end()])
            pos = m.end()
            continue
        qual = m.group(1) or ""
        # group(1) is path prefix like "nemesis_types::channel::" or "" —
        # regex consumed it; but qual includes trailing "::"? The regex
        # ([A-Za-z_:][\w:]*)? matches greedy path incl "::".
        if qual.endswith("::"):
            qual_path = qual
        else:
            qual_path = ""
        brace = m.end() - 1
        close = find_close(src, brace)
        body = src[brace + 1 : close]
        fields = parse_literal(body)
        if fields is None:
            # non-canonical shape → leave, report
            out.append(src[pos : close + 1])
            pos = close + 1
            continue
        # line indent of literal start
        line_start = src.rfind("\n", 0, m.start()) + 1
        indent = src[line_start : m.start()]
        if is_copy_shape(fields):
            skipped_copy += 1
            out.append(src[pos : close + 1])
            pos = close + 1
            continue
        replacement = render(fields, qual_path.rstrip(":"), indent)
        # qual_path like "nemesis_types::channel::" → render expects bare qual
        out.append(src[pos : m.start()])
        out.append(replacement)
        pos = close + 1
        n += 1
    new = "".join(out)
    if new != src:
        path.write_text(new, encoding="utf-8", newline="")
    return n, skipped_copy


def main():
    targets = sys.argv[1:]
    total = 0
    for rel in targets:
        p = ROOT / rel
        if not p.exists():
            print(f"MISSING {rel}")
            continue
        if rel.replace("\\", "/") in MANUAL_FILES:
            print(f"SKIP(manual) {rel}")
            continue
        n, c = process(p)
        if n or c:
            print(f"{rel}: {n} transformed, {c} copy-shape skipped")
        total += n
    print(f"TOTAL transformed: {total}")


if __name__ == "__main__":
    main()
