# -*- coding: utf-8 -*-
"""analyze_v2.py — 可测面覆盖率（v2 豁免台账：逐行/逐文件证据背书）。

v1（analyze.py）= 2026-08-28 crate 级粗粒度台账（6 crate 固定豁免量）。
v2 = 在 v1 基础上引入 wave4/5/6 findings 逐行坐实的结构墙豁免。保守原则：
  1) 只豁免证据背书的行——LINE_EVIDENCE 与官方 miss 行取交集才计入；
  2) FILE_CAPS 逐文件上限（wave5-A/C VERIFIED 表的本地裁决量，官方 miss
     ⊆ 本地 miss，故 cap 只会收窄不会放大）；
  3) crate 级 final = min(miss, max(v1_floor, evidence))——v1 台账是已认证
     下限，evidence 与 v1 重叠部分不重复计（取 max 不取和，防双计）；
  4) 无证据的 miss 保持 miss，不作任何补偿。

墙类编号沿用终版报告 §4 的 11 类：
  W1 stdin  W2 网络硬编码  W3 exit/真子进程  W4 系统突变  W5 硬件/原生库
  W6 常驻/阻塞  W7 平台 cfg  W8 真睡眠  W9 防御死分支  W10 活外部进程
  W11 llvm-cov 伪影

用法: python analyze_v2.py <lcov.info> [--detail]
"""
import sys, io
from collections import defaultdict

# ---------------------------------------------------------------- v1 floor
V1 = {  # 2026-08-28 认证（与 analyze.py EXEMPT 一致；None=不豁免，按 raw 判）
    "nemesisbot": 1161,
    "nemesis-memory": 104,
    "nemesis-desktop": 230,
    "nemesis-web": 1527,
    "nemesis-sandbox": 115,
    "nemesis-voice": 311,
}

# ------------------------------------------- wave5-A 逐文件 cap（bot crate）
# wave5_findings_A.md VERIFIED 表：11 文件剩余本地 miss 全部裁定结构墙。
BOT_FILE_CAPS = {
    "src/commands/cluster.rs": (176, "W4/W1/W2", "W5A§3-5 netsh+stdin Reset+Pair/run_node"),
    "src/commands/eval.rs": (263, "W3", "W5A§6 eval 真 sandbox 子进程链"),
    "src/commands/agent.rs": (109, "W1/W9", "W5A§4 REPL rustyline + §7 Err 臂双闸同源"),
    "src/commands/voice.rs": (133, "W5", "W5A§5 真模型包下载/sherpa 运行时"),
    "src/commands/scanner.rs": (91, "W3/W2", "W5A§6 真下载/freshclam + handler 内嵌 exit(1)"),
    "src/conflict_resolver.rs": (121, "W8", "W5A§8 PROBE_SCHEDULE 真睡眠段"),
    "src/commands/persona.rs": (180, "W2", "W5A§5 api.github.com 硬编码+转换块"),
    "src/commands/test_cmd.rs": (135, "W3", "W5A§6 真子进程+UI 窗口+WS server"),
    "src/commands/skills.rs": (128, "W2", "W5A§5 GitHub 硬编码无注入缝"),
    "src/commands/eval_rules.rs": (124, "W1", "W5A§4 + W5B-F-B9 stdin 向导三层叠加"),
    "src/main.rs": (160, "W3", "W5A§6 进程入口 main()/run_command 分发臂"),
}

# ------------------------------------------- wave5-C §9 逐文件 cap（web crate)
WEB_FILE_CAPS = {
    "src/handlers/voice.rs": (974, "W5", "W5C§9 STT/TTS/speaker 音频设备+模型下载"),
    "src/handlers/cluster.rs": (209, "W10", "W5C§9 活对端 RPC"),
    "src/handlers/scanner.rs": (134, "W2/W3", "W5C§9 ClamAV 下载/安装"),
    "src/handlers/skills.rs": (132, "W2", "W5C§9 网络臂"),
    "src/handlers/sandbox.rs": (128, "W5", "W5C§9 驱动臂"),
    "src/handlers/board.rs": (118, "W6", "W5C§9 常驻/阻塞臂"),
}
WEB_DIR_CAPS = {  # relay 子树合计
    "src/relay/": (60, "W2", "W5C§9 relay 网络臂"),
}

# ------------------------------------------- 整 crate cap（wave6-B voice）
CRATE_CAPS = {
    "nemesis-voice": (383, "W5/W2", "W6B voice 本地 383 miss 全结构性（无设备/无DLL/下载LIVE）"),
}

# ------------------------------------------- 逐行证据表
# crate -> { relpath: [(start, end, 墙类, 依据), ...] }
LINE_EVIDENCE = {
    "nemesis-agent": {
        "src/tool_dispatch.rs": [(239, 242, "W9", "W6B-A deny-None 兜底（execute 恒 Some(DenyInfo)）")],
        "src/cc_hooks.rs": [(446, 448, "W7", "W6B-A sh -c 臂 cfg!(windows) 恒真"),
                            (477, 477, "W9", "W6B-A stdin take 恒 Some")],
        "src/image_attach.rs": [(117, 117, "W9", "W6B-A sniff/mime 早退"),
                                (461, 462, "W9", "W6B-A 毫秒时间戳无法预占位"),
                                (571, 571, "W9", "W6B-A 上游已去重"),
                                (716, 716, "W9", "W6B-A verify/read TOCTOU")],
        "src/chat_log.rs": [(247, 248, "W9", "W6B-A 打开成功后立即写失败毫秒窗"),
                            (359, 361, "W9", "W6B-A 写线程 panic 无注入点"),
                            (1003, 1004, "W9", "W6B-A 非 UTF8 行 Windows 恒不出现"),
                            (1007, 1007, "W9", "W6B-A sessions 目录恒存在"),
                            (1016, 1016, "W9", "W6B-A 非 UTF8 行")],
        "src/loop_continuation.rs": [(445, 445, "W9", "W6B-C save-barrier 竞态微秒窗"),
                                     (622, 627, "W9", "W6B-A Vec<LlmMessage> 序列化无失败面"),
                                     (706, 718, "W9", "W6B-C save-barrier 竞态窗"),
                                     (1307, 1352, "W9", "W6B-A 闭包内 await 同步测试不可达")],
        "src/loop_tools.rs": [(422, 422, "W9", "W6B-A 静态 schema 构造期验证"),
                              (464, 464, "W9", "W6B-A 同上"),
                              (2396, 2396, "W9", "W6B-A parking_lot 无 poison"),
                              (2705, 2705, "W9", "W6B-A 同上"),
                              (2844, 3053, "W2", "W6B-C 真搜索引擎 API 往返"),
                              (3191, 3199, "W2", "W6B-C SSRF pinned-client（SecurityPlugin 不透传 allowed_hosts）"),
                              (3235, 3241, "W2", "W6B-C DOWNLOAD_LIMIT 大响应截断"),
                              (3293, 3293, "W7", "W6B-B I2C/SPI Linux-only"),
                              (4207, 4207, "W9", "W6B-A 关闭的 semaphore 仅进程退出"),
                              (5063, 5064, "W7", "W6B-B 硬件 Linux-only"),
                              (5192, 5194, "W7", "W6B-B 硬件 Linux-only"),
                              (5619, 5619, "W7", "W6B-B 硬件 Linux-only"),
                              (5670, 5670, "W7", "W6B-B 硬件 Linux-only"),
                              (7103, 7103, "W10", "W6B-B claude CLI 在 PATH（本机无）"),
                              (7115, 7123, "W10", "W6B-B codex CLI PATH 探测（本机未装）")],
        "src/loop_tools/lsp_tool.rs": [(228, 233, "W10", "W6B-B 活语言服务器 stdio 往返"),
                                       (268, 296, "W10", "W6B-B 同上"),
                                       (330, 333, "W9", "W6B-B 会话槽 poison 防御")],
    },
    "nemesis-board": {
        "src/store.rs": [(623, 623, "W11", "W6A-A Err(format!) 闭合括号伪影"),
                         (815, 815, "W11", "W6A-A 同"),
                         (3091, 3091, "W11", "W6A-A 同"),
                         (3246, 3246, "W9", "W6A-C extract_mentions 死防御臂（可证不可达）")],
        "src/archive.rs": [(157, 157, "W11", "W6A-A Err 收尾闭合伪影")],
        "src/archive_writer.rs": [(112, 112, "W11", "W6A-A push_str 块闭合伪影")],
        "src/asset_token.rs": [(214, 214, "W11", "W6A-A create_dir_all ? 后闭合伪影")],
        "src/db.rs": [(347, 347, "W11", "W6A-A 同")],
        "src/quota.rs": [(204, 204, "W11", "W6A-A return 块外层闭合伪影")],
        "src/git_repo.rs": [(87, 87, "W11", "W6A-A return true 后闭合伪影"),
                            (246, 246, "W11", "W6A-A create_dir_all ? 后闭合伪影"),
                            (300, 301, "W9", "W6A-B libgit2 默认 opts 无 Renamed/TypeChange"),
                            (460, 462, "W9", "W6A-B 双删路径不产出冲突条目"),
                            (524, 524, "W9", "W6A-B 真删除恒带旧路径"),
                            (529, 529, "W9", "W6A-B validate_rel_path 上游已拒 ..")],
        "src/watcher.rs": [(26, 26, "W11", "W6A-A if-let 闭合伪影")],
        "src/anchor.rs": [(260, 260, "W9", "W6A-B unreachable!（构造期强制）"),
                          (299, 299, "W7", "W6A-B UNC 臂仅非 Windows 可达"),
                          (308, 308, "W7", "W6A-B 根相对臂仅非 Windows 可达")],
    },
    "nemesis-sandbox": {
        "src/extract.rs": [(42, 131, "W2", "W6B-sb resolve/download 下载解压 GitHub LIVE"),
                           (141, 142, "W5", "W6B-sb 本机装 7-Zip none 态机器级不可达"),
                           (156, 156, "W9", "W6B-sb where/exists 消失竞态"),
                           (164, 164, "W5", "W6B-sb 首候选恒命中"),
                           (166, 166, "W5", "W6B-sb 删系统 7-Zip 禁止")],
        "src/install.rs": [(60, 123, "W2", "W6B-sb install 下载+解压+校验 LIVE"),
                           (165, 178, "W4", "W6B-sb 驱动/服务注册禁止"),
                           (210, 210, "W4", "W6B-sb 归属门读真实 SCM"),
                           (213, 213, "W4", "W6B-sb 同"),
                           (238, 238, "W6", "W6B-sb 引擎真实运行"),
                           (242, 243, "W6", "W6B-sb 同")],
        "src/selftest.rs": [(105, 120, "W2", "W6B-sb TCP 1.1.1.1 出站探针 LIVE"),
                            (150, 151, "W9", "W6B-sb 序列化无失败面")],
        "src/kmdutil.rs": [(119, 120, "W4", "W6B-sb HKLM 写=提权系统变更"),
                           (141, 142, "W4", "W6B-sb 同"),
                           (163, 164, "W4", "W6B-sb HKCU 持久变异")],
        "src/status.rs": [(21, 21, "W9", "W6B-sb sc.exe System32 恒可解析"),
                          (62, 62, "W9", "W6B-sb 需畸形注册服务"),
                          (100, 100, "W4", "W6B-sb 需真实注册服务")],
        "src/elevation.rs": [(74, 75, "W4", "W6B-sb runas 触发 UAC 弹窗禁止")],
        "src/exec_world.rs": [(270, 270, "W9", "W6B-sb wait 失败需句柄层异常")],
    },
    "nemesis-data": {
        "src/pricing.rs": [(110, 110, "W9", "W6B-data 内嵌快照 alias 先命中，构造器私有")],
        "src/pricing_lite.rs": [(45, 45, "W9", "W6B-data serde 拒 out-of-range，inf 不可达")],
    },
    "nemesis-lsp": {
        "src/install.rs": [(63, 68, "W7", "W6B-lsp cfg!(windows) 落空臂 Unix-only"),
                           (76, 81, "W7", "W6B-lsp 同"),
                           (97, 102, "W7", "W6B-lsp 同"),
                           (235, 247, "W4", "W6B-lsp auto_install 真 npm -g 系统变更+网络")],
        "src/manager.rs": [(236, 236, "W10", "W6B-lsp 活服务器 stale Response"),
                           (371, 374, "W9", "W6B-lsp spec_for-None 防御"),
                           (404, 414, "W10", "W6B-lsp 活会话请求泵"),
                           (451, 454, "W10", "W6B-lsp 活会话改名"),
                           (473, 473, "W10", "W6B-lsp 活服务器"),
                           (477, 477, "W10", "W6B-lsp 活服务器"),
                           (525, 528, "W9", "W6B-lsp spec_for-None 防御"),
                           (596, 596, "W10", "W6B-lsp 活服务器收尸"),
                           (716, 716, "W9", "W6B-lsp spec_for-None 防御"),
                           (896, 896, "W9", "W6B-lsp method_noun 穷尽死臂"),
                           (899, 900, "W9", "W6B-lsp 同")],
    },
    "nemesis-auth": {
        "src/oauth.rs": [(57, 57, "W10", "W6B-auth OAuth start 主体（真浏览器+回调端口）"),
                         (59, 64, "W10", "W6B-auth 同"),
                         (66, 67, "W10", "W6B-auth 同"),
                         (70, 74, "W10", "W6B-auth 同"),
                         (77, 78, "W10", "W6B-auth 同"),
                         (125, 125, "W8", "W6B-auth device-code 15min deadline 硬编码"),
                         (762, 762, "W10", "W6B-auth open_browser spawn Err 无 seam")],
    },
    "nemesis-utils": {  # 行号对齐 run6 lcov（BUG-3 修复前快照）
        "src/platform.rs": [(20, 21, "W7", "W6B-utils cfg!(target_os) 落空臂"),
                            (23, 23, "W7", "W6B-utils 同"),
                            (33, 34, "W7", "W6B-utils 同"),
                            (36, 36, "W7", "W6B-utils 同")],
        "src/http_retry.rs": [(186, 188, "W9", "W6B-utils loop 末次必 return 尾臂死")],
        "src/media.rs": [(182, 184, "W9", "W6B-utils Client::build 永不失败")],
        "src/message.rs": [(53, 53, "W9", "W6B-utils clamp 恒等严格 < 恒假"),
                           (123, 127, "W9", "W6B-utils 防御死分支")],
    },
    "nemesisbot": {
        "src/board_review.rs": [(2946, 2946, "W9", "W5B-F-B7 run_detached 三出口恒非空")],
    },
}


def crate_of(sf):
    s = sf.replace("\\", "/")
    if "/crates/" in s:
        return s.split("/crates/")[1].split("/")[0]
    if "/nemesisbot/" in s or s.startswith("nemesisbot/"):
        return "nemesisbot"
    return "?"


def relpath_of(sf, crate):
    s = sf.replace("\\", "/")
    if crate == "nemesisbot":
        return s.split("/nemesisbot/")[-1]
    return s.split("/crates/")[-1].split("/", 1)[1] if "/crates/" in s else s


def is_test_file(sf):
    s = sf.replace("\\", "/")
    base = s.rsplit("/", 1)[-1]
    if base == "tests.rs" or base.endswith("_tests.rs"):
        return True
    if "/tests/" in s:
        return True
    return False


def parse_miss_sets(path):
    """SF -> set of missed line numbers (DA count==0)."""
    miss = {}
    sf = None
    cur = set()
    with io.open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            line = line.strip()
            if line.startswith("SF:"):
                sf = line[3:].replace("\\", "/")
                cur = set()
            elif line.startswith("DA:"):
                parts = line[3:].split(",")
                if len(parts) >= 2 and int(parts[1]) == 0:
                    cur.add(int(parts[0]))
            elif line == "end_of_record":
                if sf and not is_test_file(sf):
                    miss[sf] = cur
                sf, cur = None, set()
    return miss


def in_ranges(ln, ranges):
    for a, b, _w, _n in ranges:
        if a <= ln <= b:
            return True
    return False


def main():
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    path = sys.argv[1]
    detail = "--detail" in sys.argv
    miss_sets = parse_miss_sets(path)

    # per-file miss counts（product, non-test）
    file_miss = defaultdict(int)   # (crate, relpath) -> miss line count
    file_miss_lines = {}           # (crate, relpath) -> set of missed line numbers
    for sf, lines in miss_sets.items():
        if "/test-tools/" in sf:
            continue  # 测试工具不在评估范围（user ruling，同 v1）
        c = crate_of(sf)
        if c == "?":
            continue
        rp = relpath_of(sf, c)
        file_miss[(c, rp)] = len(lines)
        file_miss_lines[(c, rp)] = lines

    # per-crate LF/LH：直接读 LF:/LH: 头（与 analyze.py/v1 逐字节同口径；
    # DA 条目按 region 重复，数 DA 会虚增）
    lf_lh = defaultdict(lambda: [0, 0])
    sf = None
    rec_lf = rec_lh = 0
    with io.open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            line = line.strip()
            if line.startswith("SF:"):
                sf = line[3:].replace("\\", "/")
                rec_lf = rec_lh = 0
            elif line.startswith("LF:"):
                rec_lf = int(line[3:])
            elif line.startswith("LH:"):
                rec_lh = int(line[3:])
            elif line == "end_of_record":
                if (sf and not is_test_file(sf) and sf in miss_sets
                        and "/test-tools/" not in sf):
                    d = lf_lh[crate_of(sf)]
                    d[0] += rec_lf
                    d[1] += rec_lh
                sf = None

    # evidence evaluation per crate
    ev_detail = defaultdict(list)
    ev_total = {}
    for c in set(list(lf_lh.keys()) + list(CRATE_CAPS.keys())):
        got = 0
        # (a) 逐行证据 ∩ 官方 miss
        for rp, ranges in LINE_EVIDENCE.get(c, {}).items():
            # find actual sf key
            hits = 0
            for (cc, rp2), m in file_miss_lines.items():
                if cc == c and rp2 == rp:
                    for ln in m:
                        if in_ranges(ln, ranges):
                            hits += 1
            if hits:
                got += hits
                if detail:
                    ev_detail[c].append(f"    LINE {rp}: {hits} 行")
        # (b) 逐文件 cap
        for rp, (cap, w, note) in BOT_FILE_CAPS.items():
            if c != "nemesisbot":
                continue
            m = file_miss.get(("nemesisbot", rp), 0)
            use = min(cap, m)
            if use:
                got += use
                if detail:
                    ev_detail[c].append(f"    FILE {rp}: min({cap},{m})={use}  [{w}] {note}")
        for rp, (cap, w, note) in WEB_FILE_CAPS.items():
            if c != "nemesis-web":
                continue
            m = file_miss.get(("nemesis-web", rp), 0)
            use = min(cap, m)
            if use:
                got += use
                if detail:
                    ev_detail[c].append(f"    FILE {rp}: min({cap},{m})={use}  [{w}] {note}")
        for dp, (cap, w, note) in WEB_DIR_CAPS.items():
            if c != "nemesis-web":
                continue
            m = sum(v for (cc, rp2), v in file_miss.items() if cc == "nemesis-web" and rp2.startswith(dp))
            use = min(cap, m)
            if use:
                got += use
                if detail:
                    ev_detail[c].append(f"    DIR  {dp}: min({cap},{m})={use}  [{w}] {note}")
        # (c) 整 crate cap
        for cc, (cap, w, note) in CRATE_CAPS.items():
            if c != cc:
                continue
            m = lf_lh[c][0] - lf_lh[c][1]
            use = min(cap, m)
            if use:
                got += use
                if detail:
                    ev_detail[c].append(f"    CRATE {cc}: min({cap},{m})={use}  [{w}] {note}")
        ev_total[c] = got

    print(f"{'crate':22s} {'LF':>7s} {'LH':>7s} {'raw%':>7s} {'miss':>6s} {'ev2':>6s} {'v1fl':>5s} {'ex2':>6s} {'adj2%':>7s}  verdict")
    rows = []
    tot_lf = tot_lh = tot_ex2 = tot_v1eff = 0
    for c, (lf, lh) in sorted(lf_lh.items()):
        miss = lf - lh
        ev = ev_total.get(c, 0)
        v1 = V1.get(c, 0)
        ex2 = min(miss, max(v1, ev))
        denom = lf - ex2
        adj2 = 100.0 * lh / denom if denom else 100.0
        verdict = "OK" if adj2 >= 95.0 else "BELOW-95"
        need = max(0, int(0.95 * denom + 0.999) - lh)
        need_s = "" if verdict == "OK" else f" need+{need}"
        raw = 100.0 * lh / lf if lf else 100.0
        rows.append((raw, c, lf, lh, miss, ev, v1, ex2, adj2, verdict, need_s))
        tot_lf += lf
        tot_lh += lh
        tot_ex2 += ex2
    rows.sort()
    for raw, c, lf, lh, miss, ev, v1, ex2, adj2, verdict, need_s in rows:
        print(f"{c:22s} {lf:7d} {lh:7d} {raw:6.2f}% {miss:6d} {ev:6d} {v1:5d} {ex2:6d} {adj2:6.2f}%  {verdict}{need_s}")
    print("-" * 100)
    adj2_tot = 100.0 * tot_lh / (tot_lf - tot_ex2)
    print(f"{'TOTAL':22s} {tot_lf:7d} {tot_lh:7d} {100.0*tot_lh/tot_lf:6.2f}% {'':6s} {'':6s} {'':5s} {tot_ex2:6d} {adj2_tot:6.2f}%   <- 可测面覆盖率(v2)")
    print(f"  (v1 同口径对照：exempt v1-effective = ", end="")
    tot_v1 = 0
    for c, (lf, lh) in lf_lh.items():
        v1 = V1.get(c)
        if v1 is not None:
            tot_v1 += min(v1, lf - lh)
    print(f"{tot_v1}, adj1 = {100.0*tot_lh/(tot_lf-tot_v1):.2f}%)")

    if detail:
        print("\n== evidence breakdown ==")
        for c in sorted(ev_detail.keys()):
            if not ev_detail[c]:
                continue
            print(f"  {c} (ev2={ev_total.get(c,0)}):")
            for l in ev_detail[c]:
                print(l)


if __name__ == "__main__":
    main()
