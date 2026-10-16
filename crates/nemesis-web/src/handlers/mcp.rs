//! MCP handler — status/servers/server.add/server.update/server.delete/config.get/config.save.

use crate::handlers::{mask_secret_entry, require_workspace, restore_masked_entries};
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::{Path, PathBuf};

pub struct McpHandler {
    _priv: (),
}

impl Default for McpHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl McpHandler {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

#[async_trait::async_trait]
impl ModuleHandler for McpHandler {
    fn module_name(&self) -> &str {
        "mcp"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            "status",
            "servers",
            "server.add",
            "server.update",
            "server.delete",
            "config.get",
            "config.save",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?;
        match cmd {
            "status" => self.status(workspace),
            "servers" => self.servers(workspace),
            "server.add" => {
                let data = data.ok_or("missing data")?;
                self.server_add(workspace, &data)
            }
            "server.update" => {
                let data = data.ok_or("missing data")?;
                self.server_update(workspace, &data)
            }
            "server.delete" => {
                let data = data.ok_or("missing data")?;
                let name = crate::handlers::get_str(&data, "name")?;
                self.server_delete(workspace, &name)
            }
            "config.get" => self.config_get(workspace),
            "config.save" => {
                let data = data.ok_or("missing data")?;
                self.config_save(workspace, &data)
            }
            _ => Err(format!("unknown command: mcp.{}", cmd)),
        }
    }
}

fn mcp_config_path(workspace: &str) -> PathBuf {
    // 委托 nemesis-path 唯一拼接点。
    nemesis_path::resolve_mcp_config_path_in_workspace(Path::new(workspace))
}

fn load_mcp_config(workspace: &str) -> Result<nemesis_config::McpConfig, String> {
    let path = mcp_config_path(workspace);
    nemesis_config::load_mcp_config(&path).map_err(|e| format!("failed to load MCP config: {}", e))
}

fn save_mcp_config(workspace: &str, config: &nemesis_config::McpConfig) -> Result<(), String> {
    let path = mcp_config_path(workspace);
    nemesis_config::save_mcp_config(&path, config)
        .map_err(|e| format!("failed to save MCP config: {}", e))
}

/// config.mcp.json RMW 事务唯一入口——写串行锁在函数内取得，罩
/// 「load → 闭包改 → 落盘」全程；闭包返回 Err 不落盘（abort 语义）。
/// 调用方结构上不可能忘锁（同类横扫四期补网，2026-10-13）。
/// 纪律：闭包内不得再调任何 update_* / [`nemesis_config::config_write_lock`]
/// （Mutex 非重入，嵌套即死锁）。
fn update_mcp_config<T>(
    workspace: &str,
    f: impl FnOnce(&mut nemesis_config::McpConfig) -> Result<T, String>,
) -> Result<T, String> {
    let _serial = nemesis_config::config_write_lock();
    let mut cfg = load_mcp_config(workspace)?;
    let out = f(&mut cfg)?;
    save_mcp_config(workspace, &cfg)?;
    Ok(out)
}

/// headers/env 是 `"Key: value"` / `"KEY=value"` 字符串列表——回显前逐条
/// 值部脱敏（凭据回显脱敏批次，2026-09-25；vault 方案 0.4.7 遗留）。
fn mask_string_list(list: &[String]) -> Vec<String> {
    list.iter().map(|s| mask_secret_entry(s)).collect()
}

/// 反序列化 headers/env 入参（宽容形态同 serde flexible_string_list），
/// 失败返回 None 走"字段未提供"语义。
fn parse_string_list(data: &serde_json::Value, key: &str) -> Option<Vec<String>> {
    data.get(key)
        .cloned()
        .and_then(|v| serde_json::from_value::<Vec<String>>(v).ok())
}

/// transport_type 入参归一（2026-10-15 第九轮值集闸 + 第十轮大小写归一）：
/// 大小写不敏感接受、**规范小写落盘**——CLI `mcp add` 写盘同款语义
/// （`AddRequest::build` 按 `to_ascii_lowercase` 匹配后存规范形态）；而
/// 分派真相源（`McpClient::from_server_config`）的别名匹配是大小写敏感
/// 的，非规范形态（如 "HTTP"）落盘会在挂载期被 loud 拒绝——API 输入面
/// 必须产出运行期能消费的字节。未知值（如 runtime 不支持的 "sse"）loud
/// 拒绝不落盘（兜底手写配置），此处收紧 API 输入面；错误信息保留用户
/// 原值便于定位。
fn canonical_transport_type(v: &str) -> Result<&'static str, String> {
    match v.to_ascii_lowercase().as_str() {
        "http" => Ok("http"),
        "stdio" => Ok("stdio"),
        _ => Err(format!(
            "不支持的 transport_type '{v}'（支持: http, stdio）"
        )),
    }
}

/// http 别名判定（与运行期分派真相源 `McpClient::from_server_config` 的
/// 别名集一致——手写存量条目可能是 "streamable_http"/"streamable-http"）。
fn is_http_transport(t: &str) -> bool {
    matches!(t, "http" | "streamable_http" | "streamable-http")
}

impl McpHandler {
    fn status(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let config = load_mcp_config(workspace)?;
        Ok(Some(serde_json::json!({
            "enabled": config.enabled,
            "servers_count": config.servers.len(),
        })))
    }

    fn servers(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let config = load_mcp_config(workspace)?;
        let servers: Vec<_> = config
            .servers
            .iter()
            .map(|s| {
                // Normalize for display
                let url = if s.url.is_empty() { &s.command } else { &s.url };
                let transport_type = if s.transport_type.is_empty() {
                    "stdio"
                } else {
                    &s.transport_type
                };
                serde_json::json!({
                    "name": s.name,
                    "transport_type": transport_type,
                    "url": url,
                    "description": s.description,
                    "headers": mask_string_list(&s.headers),
                    "args": s.args,
                    "env": mask_string_list(&s.env),
                    // 线上键名保持 timeout（UI 契约）；值来自规范字段 timeout_secs
                    "timeout": s.timeout_secs,
                    "provider_name": s.provider_name,
                    "provider_url": s.provider_url,
                    "tags": s.tags,
                })
            })
            .collect();
        Ok(Some(serde_json::json!({ "servers": servers })))
    }

    fn server_add(
        &self,
        workspace: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let name = crate::handlers::get_str(data, "name")?;
        // transport_type 入参归一（2026-10-15 第九/十轮）：runtime 不支持
        // 的值（如 "sse"）loud 拒绝不落盘——此前原样写盘，挂载时才由分派
        // 真相源拒绝；大小写变体（"HTTP"）规范为小写落盘（CLI `mcp add`
        // 写盘同款语义），否则分派真相源（大小写敏感别名匹配）挂载期拒绝。
        let transport_raw = crate::handlers::get_opt_str(data, "transport_type")
            .unwrap_or_else(|| "stdio".to_string());
        let transport_type = canonical_transport_type(&transport_raw)?;
        let mut server = nemesis_config::McpServerConfig {
            name: name.clone(),
            transport_type: transport_type.to_string(),
            url: crate::handlers::get_opt_str(data, "url").unwrap_or_default(),
            description: crate::handlers::get_opt_str(data, "description").unwrap_or_default(),
            headers: parse_string_list(data, "headers").unwrap_or_default(),
            args: parse_string_list(data, "args").unwrap_or_default(),
            env: parse_string_list(data, "env").unwrap_or_default(),
            // 线上键名 timeout；缺省 30（与 serde default 一致；旧值 0 会被
            // discover 的 >0 判断回落 30，直接落 30 语义更直白）
            timeout_secs: data.get("timeout").and_then(|v| v.as_u64()).unwrap_or(30),
            provider_name: crate::handlers::get_opt_str(data, "provider_name").unwrap_or_default(),
            provider_url: crate::handlers::get_opt_str(data, "provider_url").unwrap_or_default(),
            tags: parse_string_list(data, "tags").unwrap_or_default(),
            // command 入参消费化（2026-10-15 第十三轮）：此前硬编码空串——
            // 载荷里的 command 被**静默丢弃**，`{"command": "node"}` 形态
            // （CLI/既有调用方约定）产出 command/url 双空死条目，挂载期
            // spawn 空命令才炸。读入后由下方 normalize 抄到 url（url 空时），
            // 两种调用约定（url 存命令 / command 直填）产出同一条可挂载条目。
            command: crate::handlers::get_opt_str(data, "command").unwrap_or_default(),
            extra: std::collections::BTreeMap::new(),
        };
        // 新建无存量可还原：掩码值一律 loud 拒绝，绝不把掩码当真值落盘。
        for (label, list) in [("headers", &server.headers), ("env", &server.env)] {
            if list.iter().any(|e| e.contains("****")) {
                return Err(format!("{label} 含掩码值（****），新建时请输入完整值"));
            }
        }
        // 头行语法闸（2026-10-15 第十二轮）：与 CLI `mcp add` 同源（规则
        // 单一真相源 nemesis_config::validate_mcp_header_line）——此前
        // Dashboard 放行运行期必跳过/必失败的行（缺冒号、保留头、引用名带
        // 空格），失败延迟到挂载期 401 且 WARN 对用户不可见。
        for line in &server.headers {
            if let Err(reason) = nemesis_config::validate_mcp_header_line(line) {
                return Err(format!("headers 校验失败：{reason}"));
            }
        }

        // env 行语法闸（2026-10-15 第十三轮）：与 headers 同源（单一真相源
        // nemesis_config::validate_mcp_env_line）——此前坏行静默落盘，挂载
        // 期被运行期跳过，变量无声消失。
        for line in &server.env {
            if let Err(reason) = nemesis_config::validate_mcp_env_line(line) {
                return Err(format!("env 校验失败：{reason}"));
            }
        }
        // env 行语法闸（2026-10-15 第十三轮）：与 headers 同源（单一真相源
        // nemesis_config::validate_mcp_env_line）——此前坏行静默落盘，挂载
        // 期被运行期跳过，变量无声消失。

        // Legacy compat: if url empty but command provided, use command as url
        server.normalize();
        // 结构闸（2026-10-15 第十三轮）：normalize 后条目必须挂载期可消费
        // ——url 仍空 = stdio 双空死条目（spawn 空命令）或 http 无端点，
        // 在此 loud 拒绝，失败不再延迟到挂载期（与 CLI `mcp add` 恰给其
        // 一纪律同源；运行期 stdio 臂回落 url→command 同样要求其非空）。
        if server.url.trim().is_empty() {
            return Err(if is_http_transport(&server.transport_type) {
                format!(
                    "HTTP MCP server '{}' 需要 url（如 http://127.0.0.1:8808/mcp）",
                    name
                )
            } else {
                format!(
                    "stdio MCP server '{}' 需要启动命令（command 或 url 字段）",
                    name
                )
            });
        }
        // 锁在 update_mcp_config 内：权威去重 + push 全程串行（source_add
        // 同款——检测窗口内他人写入不丢）。
        update_mcp_config(workspace, |config| {
            if config.servers.iter().any(|s| s.name == name) {
                return Err(format!("MCP server '{}' already exists", name));
            }
            config.servers.push(server);
            Ok(())
        })?;
        // 第十四轮缺陷 17：sanitize 前缀碰撞写面 warn（与加载面
        // McpManager::load_config 同源 helper）——碰撞名热重载只挂首个、
        // 启动挂载工具键互相覆盖；纯可观测提示不拒绝（名字本身合法）。
        if let Ok(config) = load_mcp_config(workspace) {
            for (prefix, group) in nemesis_mcp::adapter::sanitize_prefix_collisions(
                config.servers.iter().map(|s| s.name.as_str()),
            ) {
                if group.iter().any(|n| n == &name) {
                    tracing::warn!(
                        "[mcp] Server names [{}] collide on tool prefix '{}' — hot-reload mounts only the first; rename one (Dashboard 删除后重建)",
                        group.join(", "),
                        prefix
                    );
                }
            }
        }
        Ok(Some(serde_json::json!({ "added": true, "name": name })))
    }

    fn server_update(
        &self,
        workspace: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let name = crate::handlers::get_str(data, "name")?;
        // 锁在 update_mcp_config 内：patch 字段全程串行（掩码还原要读存量
        // 同名条目，读→改→写一事务）。
        update_mcp_config(workspace, |config| {
            let server = config
                .servers
                .iter_mut()
                .find(|s| s.name == name)
                .ok_or_else(|| format!("MCP server '{}' not found", name))?;

            if let Some(v) = data.get("transport_type").and_then(|v| v.as_str()) {
                // 规范小写落盘（与 server.add 同源，2026-10-15 第十轮）。
                server.transport_type = canonical_transport_type(v)?.to_string();
            }
            if let Some(v) = data.get("url").and_then(|v| v.as_str()) {
                server.url = v.to_string();
            }
            if let Some(v) = data.get("description").and_then(|v| v.as_str()) {
                server.description = v.to_string();
            }
            if let Some(mut v) = parse_string_list(data, "headers") {
                // 先记掩码条目下标（restore 会原位替换成存量原值，之后无法
                // 再区分新行与透传的存量行）。
                let masked: Vec<usize> = v
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| l.contains("****"))
                    .map(|(i, _)| i)
                    .collect();
                // 回显是脱敏形态：掩码条目按键还原存量原值，找不到 = loud 拒绝。
                restore_masked_entries(&mut v, &server.headers)?;
                // 头行语法闸（2026-10-15 第十二轮）：只验**新/改动**行——
                // 掩码透传的存量行放行（老条目可能先于校验存在，不许因
                // 无关字段的更新被连带拒绝）；规则与 CLI/server.add 同源。
                for (i, line) in v.iter().enumerate() {
                    if masked.contains(&i) {
                        continue;
                    }
                    if let Err(reason) = nemesis_config::validate_mcp_header_line(line) {
                        return Err(format!("headers 校验失败：{reason}"));
                    }
                }
                server.headers = v;
            }
            if let Some(v) = parse_string_list(data, "args") {
                server.args = v;
            }
            if let Some(mut v) = parse_string_list(data, "env") {
                // 先记掩码条目下标（restore 原位替换后无法再区分新行与透传
                // 存量行）——与 headers 臂同款。
                let masked: Vec<usize> = v
                    .iter()
                    .enumerate()
                    .filter(|(_, l)| l.contains("****"))
                    .map(|(i, _)| i)
                    .collect();
                restore_masked_entries(&mut v, &server.env)?;
                // env 行语法闸（2026-10-15 第十三轮）：只验**新/改动**行——
                // 掩码透传的存量行放行（老条目可能先于校验存在，不许因无关
                // 字段的更新被连带拒绝）；规则与 CLI/server.add 同源。
                for (i, line) in v.iter().enumerate() {
                    if masked.contains(&i) {
                        continue;
                    }
                    if let Err(reason) = nemesis_config::validate_mcp_env_line(line) {
                        return Err(format!("env 校验失败：{reason}"));
                    }
                }
                server.env = v;
            }
            if let Some(v) = data
                .get("timeout")
                .or_else(|| data.get("timeout_secs"))
                .and_then(|v| v.as_u64())
            {
                server.timeout_secs = v;
            }
            if let Some(v) = data.get("provider_name").and_then(|v| v.as_str()) {
                server.provider_name = v.to_string();
            }
            if let Some(v) = data.get("provider_url").and_then(|v| v.as_str()) {
                server.provider_url = v.to_string();
            }
            if let Some(v) = data.get("tags").cloned()
                && let Ok(parsed) = serde_json::from_value::<Vec<String>>(v)
            {
                server.tags = parsed;
            }
            // 结构闸（2026-10-15 第十三轮）：patch 后条目必须仍挂载可消费
            // ——url 空（http 别名 = 无端点必败；stdio 且无 command = 双空
            // 死条目）loud 拒绝整次更新（闭包 Err = 事务 abort 不落盘）。
            // CLI 写盘的 stdio 条目 command 可独立成立（url 空）不受影响。
            if server.url.trim().is_empty()
                && (is_http_transport(&server.transport_type) || server.command.trim().is_empty())
            {
                return Err(if is_http_transport(&server.transport_type) {
                    format!("更新后的 MCP server '{}' 不可挂载：http 形态需要 url", name)
                } else {
                    format!(
                        "更新后的 MCP server '{}' 不可挂载：stdio 形态需要启动命令",
                        name
                    )
                });
            }
            Ok(())
        })?;
        Ok(Some(serde_json::json!({ "updated": true, "name": name })))
    }

    fn server_delete(
        &self,
        workspace: &str,
        name: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        // 锁在 update_mcp_config 内。
        update_mcp_config(workspace, |config| {
            let before = config.servers.len();
            config.servers.retain(|s| s.name != name);
            if config.servers.len() == before {
                return Err(format!("MCP server '{}' not found", name));
            }
            Ok(())
        })?;
        Ok(Some(serde_json::json!({ "deleted": true, "name": name })))
    }

    fn config_get(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let config = load_mcp_config(workspace)?;
        let mut json =
            serde_json::to_value(&config).map_err(|e| format!("failed to serialize: {}", e))?;
        // servers[].headers/env 逐条值部脱敏（凭据回显脱敏批次）。
        if let Some(servers) = json.get_mut("servers").and_then(|v| v.as_array_mut()) {
            for s in servers.iter_mut() {
                for key in ["headers", "env"] {
                    if let Some(serde_json::Value::Array(arr)) = s.get_mut(key) {
                        for entry in arr.iter_mut() {
                            if let serde_json::Value::String(es) = entry {
                                *es = mask_secret_entry(es);
                            }
                        }
                    }
                }
            }
        }
        Ok(Some(json))
    }

    fn config_save(
        &self,
        workspace: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let mut new_cfg: nemesis_config::McpConfig = serde_json::from_value(data.clone())
            .map_err(|e| format!("invalid MCP config: {}", e))?;
        // 锁在 update_mcp_config 内：掩码还原要读存量（读→合并→整写 = RMW）。
        update_mcp_config(workspace, |cfg| {
            // 整包回存同源还原：掩码条目按同名存量 server 还原；无存量 loud 拒绝。
            for server in new_cfg.servers.iter_mut() {
                if let Some(old) = cfg.servers.iter().find(|s| s.name == server.name) {
                    restore_masked_entries(&mut server.headers, &old.headers)?;
                    restore_masked_entries(&mut server.env, &old.env)?;
                } else {
                    restore_masked_entries(&mut server.headers, &[])?;
                    restore_masked_entries(&mut server.env, &[])?;
                }
            }
            *cfg = new_cfg;
            Ok(())
        })?;
        Ok(Some(serde_json::json!({ "saved": true })))
    }
}

// Phase 3 覆盖率（2026-08-25）：servers 列表 legacy 归一化显示 +
// server.update 全可选字段 patch 臂（旧清单只盖 url/args/env/timeout）。
#[cfg(test)]
mod tests;

// AGT 覆盖率批次（2026-09-25）：Default 转发体（本文件唯一缺失面）。
#[cfg(test)]
mod agt_tests;
