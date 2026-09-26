//! Skins WSAPI handler — Dashboard 设置页「皮肤」tab 的管理面。
//!
//! 命令：`list` / `reload`（现扫即最新，语义锚点）/ `detail` / `set_active`。扫描与验证在 `crate::skins::scan_skins`（管理面
//! 按需现扫现验，stateless scan-per-call 无缓存失效面；数据面分发不验签）。
//!
//! `set_active` 是唯一的信任决策写点：校验（存在 + status=ok + 有主题
//! 载荷 + require_signed 策略）→ 写 config.json `ui.skin`（live 优先
//! 装配，与 config handler 同一 helper）→ 翻内存共享锁（免重启热切，
//! 下一请求即生效）。`id="default"` = 关皮肤（active.css 404，前端既有
//! 回落链生效）。
//!
//! 装配：web_init 注入 {skins 目录, 激活 id 共享锁句柄} 到模块级槽位
//!（PROJECTS_BRIDGE 同款模式）；未注入 = 全部命令诚实报「未装配」。

#![cfg(feature = "skins")]

use crate::skins::{SkinSignature, SkinStatus, scan_skins};
use crate::ws_router::{ModuleHandler, RequestContext};
use parking_lot::RwLock;
use serde_json::{Value, json};
use std::sync::Arc;

/// 模块级装配槽：{skins 目录, 激活 id 共享锁}。
static SKINS_SLOT: RwLock<Option<SkinsHandle>> = RwLock::new(None);

#[derive(Clone)]
struct SkinsHandle {
    /// exe 同级 `skins/` 目录（None = exe 路径不可定位）。
    dir: Option<String>,
    /// 激活 id 共享锁——与 router 内 SkinHost 同一把锁（热切语义）。
    active: Arc<RwLock<String>>,
}

/// web_init 装配点：注入目录 + 锁句柄。
pub fn set_handle(dir: Option<String>, active: Arc<RwLock<String>>) {
    *SKINS_SLOT.write() = Some(SkinsHandle { dir, active });
}

fn take_handle() -> Result<SkinsHandle, String> {
    SKINS_SLOT
        .read()
        .clone()
        .ok_or_else(|| "皮肤系统未装配（web_init 未注入句柄）".to_string())
}

pub struct SkinsHandler;

impl Default for SkinsHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl SkinsHandler {
    pub fn new() -> Self {
        Self
    }

    fn list(&self) -> Result<Option<Value>, String> {
        let h = take_handle()?;
        let Some(dir) = h.dir else {
            // exe 路径不可定位的极端装配：诚实空表。
            return Ok(Some(
                json!({ "dir": Value::Null, "dir_exists": false, "skins": [] }),
            ));
        };
        let dir_exists = std::path::Path::new(&dir).is_dir();
        let skins = scan_skins(&dir);
        Ok(Some(
            json!({ "dir": dir, "dir_exists": dir_exists, "skins": skins }),
        ))
    }

    fn detail(&self, data: Option<Value>) -> Result<Option<Value>, String> {
        let h = take_handle()?;
        let id = data
            .as_ref()
            .and_then(|d| d.get("id"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "缺少 id".to_string())?;
        let dir = h
            .dir
            .as_deref()
            .ok_or_else(|| "皮肤系统未装配".to_string())?;
        let entry = scan_skins(dir)
            .into_iter()
            .find(|e| e.id == id)
            .ok_or_else(|| format!("皮肤不存在：{id}"))?;
        Ok(Some(json!({ "dir": dir, "skin": entry })))
    }

    fn set_active(
        &self,
        data: Option<Value>,
        ctx: &RequestContext,
    ) -> Result<Option<Value>, String> {
        let h = take_handle()?;
        let id = data
            .as_ref()
            .and_then(|d| d.get("id"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "缺少 id".to_string())?;
        let home = ctx
            .home
            .clone()
            .ok_or_else(|| "home not configured".to_string())?;

        // config 一次装载：require_signed 读取与 ui.skin 写入同源。
        let mut cfg = super::config::load_config(&home)?;

        // ① 非默认 id：先过包体/策略校验，全绿才动 config 与锁。
        if id != "default" {
            let dir = h
                .dir
                .as_deref()
                .ok_or_else(|| "皮肤系统未装配".to_string())?;
            let entry = scan_skins(dir)
                .into_iter()
                .find(|e| e.id == id)
                .ok_or_else(|| format!("皮肤不存在：{id}"))?;
            if entry.status != SkinStatus::Ok {
                let reason = entry.status_detail.unwrap_or_else(|| "未知原因".into());
                return Err(format!("皮肤包损坏，无法启用（{reason}）"));
            }
            if entry.manifest.entry.is_none() {
                return Err("该皮肤包无主题载荷（skin/ CSS），无法设为默认观感".to_string());
            }
            // require_signed 后手开关（默认 false；闸在信任决策点，数据面不拦）。
            let require_signed = cfg
                .ui
                .as_ref()
                .map(|u| u.skins.require_signed)
                .unwrap_or(false);
            if require_signed && entry.signature != SkinSignature::Verified {
                return Err("ui.skins.require_signed 已开启：拒绝非 verified 皮肤包".to_string());
            }
        }

        // ② 写 config.json `ui.skin`（typed save，未类型化键保留的回归已锁）。
        let mut ui = cfg.ui.take().unwrap_or_default();
        ui.skin = id.to_string();
        cfg.ui = Some(ui);
        super::config::save_config_to_disk(&home, &mut cfg)?;

        // ③ 翻内存锁：下一请求 active.css 立即切到新皮肤（免重启）。
        *h.active.write() = id.to_string();
        Ok(Some(json!({ "active": id })))
    }
}

#[async_trait::async_trait]
impl ModuleHandler for SkinsHandler {
    fn module_name(&self) -> &str {
        "skins"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["list", "detail", "reload", "set_active"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<Value>,
        ctx: &RequestContext,
    ) -> Result<Option<Value>, String> {
        match cmd {
            "list" | "reload" => self.list(),
            "detail" => self.detail(data),
            "set_active" => self.set_active(data, ctx),
            _ => Err(format!("unknown command: skins.{cmd}")),
        }
    }
}

#[cfg(test)]
mod tests;
