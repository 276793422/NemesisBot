//! 进程内活跃文件注册表（active-files registry）。
//!
//! 背景：Unix 上 `unlink` 对打开中的句柄不设防——持有 append 句柄的文件
//! 仍会被 `remove_file` 成功断链，后续写入进入已断链 inode，对目录遍历者
//! 静默消失。Windows 上 `OpenOptions::append` 默认不带 `FILE_SHARE_DELETE`，
//! remove 撞 sharing violation 被偶然挡住——同一代码两平台行为分裂，不能
//! 依赖平台巧合当保护。
//!
//! 本模块建立进程内「**文件打开 ⇒ 路径在册**」的结构不变量：长期持有
//! append 句柄的写入方（B4 后台任务 tee 镜像、PTY 审计日志等连接/任务
//! 生命周期常驻句柄）在打开成功时 [`ActiveFile::acquire`] 注册路径，守卫
//! 与句柄同存于宿主结构体，句柄退场随 Drop 注销。mtime 型 TTL 清扫器对
//! 「活着但静默」的文件（超 TTL 无输出 → mtime 停在最后写入）必须先
//! [`is_active`] 检查跳过——mtime 新鲜假设对静默文件为假，Windows 句柄
//! 屏障是平台巧合，注册表才是跨平台单一真相源。
//!
//! 匹配语义：路径按**字面拼写**精确比对（不做 canonicalize——`\\?\` 扩展
//! 前缀等形式差异会把相等判成不等）。注册方与清扫方必须从同一 workspace
//! 根以相同 join 形态构造路径（两处现有接线：tee 的 `dir` 同值直传清扫器、
//! sweeper 与 AuditWriter 同源于 workspace 根，均按此成立）。拼写漂移时
//! 检查退化为 miss（清扫照旧执行，不误跳过），退化为保守方向。
//!
//! 线程安全：全局 `Mutex<HashSet>`，锁内只做 insert/remove/contains 微操作，
//! 无 IO 无重入，不存在死锁面；锁中毒按 into_inner 降级续用（注册表是
//! best-effort 家务面，不放大成 panic）。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};

static ACTIVE_FILES: LazyLock<Mutex<HashSet<PathBuf>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

fn lock() -> MutexGuard<'static, HashSet<PathBuf>> {
    ACTIVE_FILES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 活跃文件守卫：存活期间路径在册，Drop 时注销。
///
/// 与文件句柄生命周期绑定使用——持有句柄的结构体存一份本守卫（字段名
/// 惯例 `_active`），结构体退场即注销，不需要手写 Drop。
#[derive(Debug)]
pub struct ActiveFile {
    path: PathBuf,
}

impl ActiveFile {
    /// 注册路径。调用点应在文件句柄**成功打开之后**（在册 = 句柄开着）。
    pub fn acquire(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        lock().insert(path.clone());
        Self { path }
    }

    /// 注册的路径。
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ActiveFile {
    fn drop(&mut self) {
        lock().remove(&self.path);
    }
}

/// 路径当前是否被某个 [`ActiveFile`] 守卫持有。TTL 清扫器删除前检查；
/// true = 有写入方仍持句柄，跳过留给句柄退场后的后续轮次。
pub fn is_active(path: &Path) -> bool {
    lock().contains(path)
}
