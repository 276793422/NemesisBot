//! NemesisBot utility library.
//!
//! Common utilities: string helpers, media detection, file operations,
//! message splitting, HTTP retry, skill parsing, zip handling.

pub mod env_sanitize;
// 单机实例身份（W1 #31，2026-11-19）：workspace/identity.json 持久
// instance_id——审计链（hash v2）/启动日志的归属锚点。
pub mod instance_identity;
// 子进程文本输出解码单一真相源（BUG-20261115-034）——exec 家族 stdout/
// stderr 严格 UTF-8 优先 + Windows OEM 码页（CJK 族）回落。
pub mod child_decode;
// 关键持久化写失败的进程级信号注册表——
// 生产者 nemesis-agent（chat_log/checkpoint/turn_wal），消费者 gateway
// 水泵（EventHub system-alert + /ready 面）。
pub mod degraded;
pub mod file;
pub mod fsutil;
// HTTP 响应体流式累计闸（BUG-20261107-005）：「bytes() 全量读入后才查上限」
// 形态的 workspace 共享单一真相源——读入中途超限即断。同 media/http_retry
// 走 http 门控（reqwest 不进未启用方的依赖图）。
#[cfg(feature = "http")]
pub mod http_cap;
pub mod http_retry;
// 跨主机重定向终止策略（BUG-20261109-014）：带凭据外呼 client 的重定向
// 策略单一真相源——reqwest 默认跨主机只剥标准四头，自定义凭据头
// （x-api-key 等）与 URL path token 会随 302 带去第三方主机。http 门控
// 同 media/http_cap（reqwest 不进未启用方的依赖图）。
#[cfg(feature = "http")]
pub mod http_redirect;
// 带帽单行读（横扫角度 B 续——行帧形态）：外部流行读统一实现，
// 裸 read_line 无界积累防 OOM；nemesis-mcp stdio 与 nemesis-channels
// 四通道同源消费，帽值调用方按协议传入。
pub mod json_shape;
pub mod line_cap;
pub mod llm_error_class;
// http 门控：media 模块核心是 reqwest 下载——未启用 `http`
// feature 的消费方（nemesis-guest musl 交叉构建）不编译本模块。
#[cfg(feature = "http")]
pub mod media;
pub mod message;
// 有界管道排干（横扫角度 B）：子进程 stdout/stderr 收集
// 的统一定义——1 MiB 丢头保尾，防高吞吐输出 OOM。
pub mod pipe_cap;
pub mod platform;
// 提示词安全常态化触发（W2 批三 #16）：人格资产变更后 detached spawn
// `eval prompt`（fire-and-forget；非 Windows 恒 no-op + SKIP 落日志）。
pub mod prompt_eval;
// 代理 URL 日志脱敏（SEC-002 口径，BUG-20261120-018 收口）：代理 URL 可
// 携带 userinfo 凭据，日志/错误上下文只落 scheme 不落完整 URL——全仓
// Proxy::all 消费面 Ok/Err 日志臂单一真相源（channels/providers/voice 同
// 依赖）。纯字符串函数零依赖，不设 feature 门。
pub mod proxy_redact;
pub mod sanitize;
pub mod skills;
// fire-and-forget spawn 收尸单一真相源（BUG-20261117-087：std Child drop
// 不收尸，Unix 长命 gateway 下僵尸堆积；oauth 浏览器唤起 + shell_open 打开
// 动作两消费方收敛至此）。
pub mod spawn_reap;
// SSE 事件边界定位单一真相源（原 nemesis-providers/nemesis-mcp
// 各自内联双形态扫描；规范行终止三态逐行独立，CR-only/混合行尾
// 此前零事件解析）。
pub mod sse;
// 存储世代化迁移链骨架（四条铁律的代码化
// 模板——零接线，未来破坏性格式变更时按设计文档契约接线）。
pub mod storage_gen;
// 统一 WorkPool 设施（CPU 准入 max(1,N-1) /
// reader-compute-writer 分池 / 双上限背压+类型化 overloaded / 取消三事实分离）。
pub mod workpool;
// （存储韧性）：SQLite 打开面前置守卫——feature 门控，
// 未启用 `sqlite-preflight` 的消费方不编译本模块（rusqlite 不进依赖图）。
#[cfg(feature = "sqlite-preflight")]
pub mod sqlite_preflight;
pub mod string_utils;
// 年龄/保留天数输入上界钳制单一真相源（R735 数字恐慌面：
// chrono Duration::days 恐慌带 + as-cast 回绕负 cutoff 全删带，统一收敛）。
pub mod time_bounds;
// 恒时 token 比较单一真相源（原 nemesis-web api_handlers
// 私有件；web re-export 保路径，nemesis-channels websocket 升级闸跨 crate 消费）。
pub mod verify;
pub mod wait_ready;
// YAML 嵌套深度词法估计单一真相源（BUG-20261110-044：原 nemesis-forge
// 私有件——plain scalar 撇号幻影引号跨行吞计深绕过守卫；晋升后
// nemesis-workflow YAML 解析面同源消费，serde_yaml 深嵌套栈溢出闸）。
pub mod yaml_depth;
pub mod zip_util;

// Re-export the most commonly used functions
pub use file::{
    ATOMIC_TMP_ORPHAN_GRACE, ATOMIC_TMP_PREFIX, ensure_dir, read_file_string, write_file_atomic,
    write_file_atomic_async,
};
#[cfg(feature = "http")]
pub use http_retry::do_request_with_retry_reqwest;
pub use http_retry::{
    RetryConfig, RetryableResponse, do_request_with_retry_simple, should_retry, sleep_with_cancel,
};
pub use json_shape::{ensure_object, json_type_name};
#[cfg(feature = "http")]
pub use media::{
    DownloadOptions, detect_media_type, download_file, download_file_simple,
    download_file_with_opts, is_audio_file, sanitize_filename,
};
pub use message::{format_message, sanitize_for_log, split_message};
pub use platform::{
    find_plugin_library, find_plugin_library_in, plugin_library_filename, plugin_library_label,
};
pub use sanitize::{sanitize_csv, sanitize_path_segment};
pub use skills::{extract_slug, normalize_skill_name};
pub use string_utils::{
    deref_str, format_datetime_compact, format_timestamp, is_blank, json_get_bool, json_get_f64,
    json_get_i64, json_get_str, parse_json, pretty_json, random_id, random_short_id, truncate,
};
pub use verify::verify_token;
pub use zip_util::{create_zip, extract_zip, is_path_within_dir, is_unsafe_rel_path};
