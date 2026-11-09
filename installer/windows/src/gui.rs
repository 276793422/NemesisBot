//! GUI 向导（egui 0.27）：选项页（包摘要 + 签名分级横幅 + 红级知情确认 +
//! 安装目录 + 勾选项）→ 进度页（后台线程 + 共享状态轮询）→ 完成/失败页。
//! 卸载 GUI = rfd 确认对话框 + 结果对话框（ARP 入口轻量形态）。
//!
//! 纪律：
//! - 安装线程后台跑（std::thread），UI 只读共享状态——绝不阻塞 UI 线程；
//! - 红色级（签名异常）必须勾选知情确认才能点安装（GUI 形态的 §4a 闸；
//!   线程侧用 SigPolicy::Enforce 双保险——UI 勾选只解锁按钮，策略照走
//!   accept_risk 路径）；
//! - 全流程零弹窗依赖 except rfd（浏览目录/确认对话框）。

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

use egui::{
    Button, CentralPanel, Checkbox, Color32, Context, ProgressBar, RichText, ScrollArea, TextEdit,
};

use crate::ops::{self, InstallOutcome};
use setup_core::pkgfile::PackageInfo;

/// 退出码载体（窗口关闭后由 main 读取）。
pub type ExitCode = Arc<AtomicI32>;

/// 共享安装状态（后台线程写，UI 读）。
#[derive(Default)]
struct Shared {
    text: String,
    frac: Option<f32>,
    done: Option<anyhow::Result<InstallOutcome>>,
}

/// 页面（Clone 化：update 的 match 需要 owned 值拆 self 双借用）。
#[derive(Clone)]
enum Page {
    /// 选项页。
    Options,
    /// 进度页。
    Progress,
    /// 完成页。
    Done(InstallOutcome),
    /// 失败页。
    Failed(String),
}

struct InstallerApp {
    package: std::path::PathBuf,
    info: PackageInfo,
    /// 外壳自检（安装器本体；计划 §4a 第二条信任链）。
    shell_sig: setup_core::pkgfile::VerifyOutcomeDisplay,
    page: Page,
    dir: String,
    desktop_shortcut: bool,
    autostart: bool,
    accept_risk: bool,
    launch_after: bool,
    shared: Arc<Mutex<Shared>>,
    exit_code: ExitCode,
}

/// 安装包体积人读文本。
fn fmt_mb(bytes: u64) -> String {
    if bytes >= 1_048_576 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else {
        format!("{:.0} KB", bytes as f64 / 1_024.0)
    }
}

fn grade_color(grade: &setup_core::sig::SigGrade) -> Color32 {
    match grade {
        setup_core::sig::SigGrade::Green => Color32::from_rgb(0x4c, 0xaf, 0x50),
        setup_core::sig::SigGrade::Yellow => Color32::from_rgb(0xff, 0xc1, 0x07),
        setup_core::sig::SigGrade::Red => Color32::from_rgb(0xef, 0x53, 0x50),
    }
}

impl eframe::App for InstallerApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        // clone 后传入（self 双借用拆解：page 内容独立于 &mut self 方法）。
        match self.page.clone() {
            Page::Options => self.show_options(ctx),
            Page::Progress => self.show_progress(ctx),
            Page::Done(outcome) => self.show_done(ctx, &outcome),
            Page::Failed(msg) => self.show_failed(ctx, &msg),
        }
    }
}

impl InstallerApp {
    fn show_options(&mut self, ctx: &Context) {
        let grade = self.info.sig.grade.clone();
        let sig_text = self.info.sig.text.clone();
        let shell_red = self.shell_sig.grade == setup_core::sig::SigGrade::Red;
        let is_red = grade == setup_core::sig::SigGrade::Red || shell_red;
        let shell_text = self.shell_sig.text.clone();
        let version = self.info.manifest.bot_version.clone();
        let total = self.info.manifest.total_size;
        let files = self.info.manifest.files.len();

        // 目录变化时的安装计划预览（每帧一次 stat，成本可忽略）。
        let plan_line = setup_core::install::plan(std::path::Path::new(&self.dir), Some(&version))
            .map(|p| ops::plan_text(&p))
            .unwrap_or_else(|e| format!("⚠ 现有安装探测失败: {e}"));

        CentralPanel::default().show(ctx, |ui| {
            ui.heading(format!("安装 NemesisBot {version}"));
            ui.add_space(8.0);
            ui.label(format!(
                "载荷：{files} 个文件，{total}（安装约需 {}）",
                fmt_mb(ops::required_disk_bytes(total))
            ));
            ui.add_space(8.0);

            // 签名分级横幅。
            let (banner, level_text) = match grade {
                setup_core::sig::SigGrade::Green => ("签名有效", ""),
                setup_core::sig::SigGrade::Yellow => {
                    ("未签名安装包（开发构建常态），请确认来源可信", "")
                }
                setup_core::sig::SigGrade::Red => ("签名异常", sig_text.as_str()),
            };
            ui.colored_label(
                grade_color(&grade),
                RichText::new(if level_text.is_empty() {
                    banner.to_string()
                } else {
                    format!("{banner}：{level_text}")
                }),
            );
            // 外壳自检行（第二条信任链；黄/红态才显示）。
            if self.shell_sig.grade != setup_core::sig::SigGrade::Green {
                ui.colored_label(
                    grade_color(&self.shell_sig.grade),
                    format!("安装器本体: {shell_text}"),
                );
            }
            ui.add_space(8.0);
            ui.separator();

            // 安装位置。
            ui.horizontal(|ui| {
                ui.label("安装位置:");
                let dir_edit =
                    TextEdit::singleline(&mut self.dir).desired_width(ui.available_width() - 90.0);
                ui.add(dir_edit);
                if ui.button("浏览...").clicked()
                    && let Some(picked) = rfd::FileDialog::new()
                        .set_title("选择安装位置")
                        .pick_folder()
                {
                    self.dir = picked.to_string_lossy().into_owned();
                }
            });
            ui.label(RichText::new(plan_line).weak());
            ui.add_space(8.0);

            // 勾选项。
            ui.add(Checkbox::new(
                &mut self.desktop_shortcut,
                "创建桌面快捷方式",
            ));
            ui.add(Checkbox::new(&mut self.autostart, "开机自启动"));
            ui.add(Checkbox::new(
                &mut self.launch_after,
                "安装完成后启动 NemesisBot",
            ));
            if is_red {
                ui.add_space(4.0);
                ui.add(Checkbox::new(
                    &mut self.accept_risk,
                    "我已知晓签名异常风险，仍要继续安装",
                ));
            }
            ui.add_space(12.0);

            let can_install = !self.dir.trim().is_empty() && (!is_red || self.accept_risk);
            ui.horizontal(|ui| {
                let btn = Button::new(RichText::new("安装").strong());
                if ui.add_enabled(can_install, btn).clicked() {
                    self.start_install();
                }
                if ui.button("退出").clicked() {
                    self.exit_code.store(3, Ordering::SeqCst);
                    request_close(ctx);
                }
            });
        });
    }

    fn start_install(&mut self) {
        let shared = self.shared.clone();
        let opts = ops::InstallOptions {
            package: self.package.clone(),
            target_dir: std::path::PathBuf::from(self.dir.trim()),
            accept_risk: self.accept_risk,
            force_downgrade: false,
            desktop_shortcut: self.desktop_shortcut,
            no_shortcuts: false,
            autostart: self.autostart,
            launch: self.launch_after,
        };
        std::thread::spawn(move || {
            let cb_shared = shared.clone();
            let progress = move |p: setup_core::pkgfile::Progress| {
                let mut s = cb_shared.lock().unwrap_or_else(|e| e.into_inner());
                match p {
                    setup_core::pkgfile::Progress::Hashing(f) => {
                        s.text = format!("校验载荷 {:.0}%", f * 100.0);
                        s.frac = Some(f * 0.5);
                    }
                    setup_core::pkgfile::Progress::Extracting(d, t) => {
                        s.text = format!("释放文件 {d}/{t}");
                        s.frac = Some(0.5 + 0.5 * d as f32 / t.max(1) as f32);
                    }
                }
            };
            let result = ops::run_install(&opts, Some(&progress));
            let mut s = shared.lock().unwrap_or_else(|e| e.into_inner());
            s.done = Some(result);
        });
        self.page = Page::Progress;
    }

    fn show_progress(&mut self, ctx: &Context) {
        let (text, frac, done) = {
            let s = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            (s.text.clone(), s.frac, s.done.is_some())
        };
        CentralPanel::default().show(ctx, |ui| {
            ui.heading("正在安装");
            ui.add_space(12.0);
            let bar = ProgressBar::new(frac.unwrap_or(0.0)).text(&text);
            ui.add(bar);
            if done {
                let result = self
                    .shared
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .done
                    .take();
                match result {
                    Some(Ok(outcome)) => {
                        self.exit_code.store(0, Ordering::SeqCst);
                        self.page = Page::Done(outcome);
                    }
                    Some(Err(e)) => {
                        let code = ops::exit_code_of(&e);
                        self.exit_code.store(code, Ordering::SeqCst);
                        self.page = Page::Failed(format!("{e:#}"));
                    }
                    None => {}
                }
            }
        });
    }

    fn show_done(&mut self, ctx: &Context, outcome: &InstallOutcome) {
        CentralPanel::default().show(ctx, |ui| {
            ui.heading(format!("安装完成（{}）", outcome.bot_version));
            ui.add_space(8.0);
            ui.label(format!(
                "{}：{} 个文件 → {}",
                outcome.plan_text,
                outcome.files_installed,
                outcome.install_dir.display()
            ));
            if outcome.workspace_skipped > 0 {
                ui.label(format!(
                    "升级模式跳过 workspace 数据 {} 个文件（用户数据保留）",
                    outcome.workspace_skipped
                ));
            }
            for w in &outcome.warnings {
                ui.colored_label(Color32::from_rgb(0xff, 0xc1, 0x07), format!("⚠ {w}"));
            }
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                if ui.button("启动 NemesisBot").clicked() {
                    let dir = outcome.install_dir.clone();
                    if let Err(e) = ops::launch_dashboard(&dir) {
                        ui.colored_label(
                            Color32::from_rgb(0xef, 0x53, 0x50),
                            format!("启动失败: {e:#}"),
                        );
                    }
                }
                if ui.button("完成").clicked() {
                    request_close(ctx);
                }
            });
        });
    }

    fn show_failed(&mut self, ctx: &Context, msg: &str) {
        CentralPanel::default().show(ctx, |ui| {
            ui.heading(RichText::new("安装失败").color(Color32::from_rgb(0xef, 0x53, 0x50)));
            ui.add_space(8.0);
            ScrollArea::vertical().show(ui, |ui| {
                ui.label(msg);
            });
            ui.add_space(12.0);
            if ui.button("关闭").clicked() {
                request_close(ctx);
            }
        });
    }
}

/// 请求关闭窗口（eframe 0.27 viewport close）。
fn request_close(ctx: &Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
}

/// GUI 安装向导入口（返回退出码）。检视失败直接错误对话框。
pub fn run_install_wizard(package: std::path::PathBuf, default_dir: std::path::PathBuf) -> i32 {
    let info = match ops::inspect_package(&package) {
        Ok(i) => i,
        Err(e) => {
            rfd::MessageDialog::new()
                .set_title("安装包异常")
                .set_description(format!("安装包无法读取：{e:#}"))
                .set_buttons(rfd::MessageButtons::Ok)
                .set_level(rfd::MessageLevel::Error)
                .show();
            return ops::exit_code_of(&e);
        }
    };

    let exit_code: ExitCode = Arc::new(AtomicI32::new(0));
    let app = InstallerApp {
        dir: default_dir.to_string_lossy().into_owned(),
        shared: Arc::new(Mutex::new(Shared::default())),
        page: Page::Options,
        desktop_shortcut: false,
        autostart: false,
        accept_risk: false,
        launch_after: true,
        shell_sig: setup_core::sig::shell_self_check(),
        package,
        info,
        exit_code: exit_code.clone(),
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([580.0, 460.0])
            .with_title("NemesisBot 安装"),
        ..Default::default()
    };
    let run_result = eframe::run_native("NemesisBot 安装", options, Box::new(|_cc| Box::new(app)));
    if let Err(e) = run_result {
        eprintln!("GUI 运行失败: {e}");
        return 1;
    }
    exit_code.load(Ordering::SeqCst)
}

/// GUI 卸载入口（确认对话框 + 结果对话框；返回退出码）。
pub fn run_uninstall_wizard(install_dir: &std::path::Path) -> i32 {
    let confirmed = rfd::MessageDialog::new()
        .set_title("卸载 NemesisBot")
        .set_description(format!(
            "将从 {} 移除 NemesisBot。\n用户数据（~/.nemesisbot）与看板/日志保留。\n\n确定卸载？",
            install_dir.display()
        ))
        .set_buttons(rfd::MessageButtons::YesNo)
        .show();
    if confirmed != rfd::MessageDialogResult::Yes {
        return 3;
    }
    match ops::run_uninstall(install_dir) {
        Ok(outcome) => {
            let mut msg = format!("卸载完成：删除 {} 个文件。", outcome.deleted);
            if !outcome.failures.is_empty() {
                msg.push_str(&format!(
                    "\n部分文件删除失败（可能被占用）：{:?}",
                    outcome.failures
                ));
            }
            if outcome.self_delete_scheduled {
                msg.push_str("\n卸载器将自行退出并清理。");
            }
            rfd::MessageDialog::new()
                .set_title("卸载完成")
                .set_description(msg)
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
            0
        }
        Err(e) => {
            rfd::MessageDialog::new()
                .set_title("卸载失败")
                .set_description(format!("{e:#}"))
                .set_buttons(rfd::MessageButtons::Ok)
                .set_level(rfd::MessageLevel::Error)
                .show();
            ops::exit_code_of(&e)
        }
    }
}

/// 安装包体积人读（供单元测试锚定）。
#[cfg(test)]
mod tests {
    use super::fmt_mb;

    #[test]
    fn fmt_mb_units() {
        assert_eq!(fmt_mb(512), "0 KB");
        assert_eq!(fmt_mb(1024), "1 KB");
        assert_eq!(fmt_mb(1_048_576), "1.0 MB");
        assert_eq!(fmt_mb(115_000_000), "109.7 MB");
    }
}
