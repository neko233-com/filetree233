use crate::i18n::t;
use crate::{
    CleanupMode, ScanEngine, delete_checked, plan_description, read_checked_plan, save_plan_mode,
    scan_dispatch,
};
use eframe::egui::{self, Color32, RichText, Sense, Vec2, pos2};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
};

pub fn run(path: Option<&std::path::Path>, threads: u8) -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([700.0, 450.0]),
        ..Default::default()
    };
    let start = path.map(|p| p.to_path_buf());
    eframe::run_native(
        "filetree233",
        options,
        Box::new(move |cc| {
            install_system_font(&cc.egui_ctx);
            let mut app = App {
                threads,
                ..Default::default()
            };
            if let Some(start) = start {
                app.set_tree_path(&start);
                app.pending_scan = true;
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| e.to_string())
}
fn install_system_font(ctx: &egui::Context) {
    #[cfg(windows)]
    let candidates = [r"C:\Windows\Fonts\msyh.ttc", r"C:\Windows\Fonts\simsun.ttc"];
    #[cfg(target_os = "linux")]
    let candidates = [
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
    ];
    #[cfg(target_os = "macos")]
    let candidates = ["/System/Library/Fonts/PingFang.ttc"];
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    let candidates: [&str; 0] = [];
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert(
                "system-cjk".into(),
                egui::FontData::from_owned(bytes).into(),
            );
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts
                    .families
                    .entry(family)
                    .or_default()
                    .push("system-cjk".into());
            }
            ctx.set_fonts(fonts);
            break;
        }
    }
}
fn raw_entry_path(entry: &serde_json::Value) -> Option<PathBuf> {
    let units = entry.get("path_units")?.as_array()?;
    let encoded =
        serde_json::json!({"encoding":if cfg!(windows){"utf16"}else{"bytes"},"units":units});
    crate::native_path::from_value(&encoded).ok()
}
fn is_lossless_display_path(entry: &serde_json::Value) -> bool {
    let Some(path) = entry["path"].as_str() else {
        return false;
    };
    let Some(units) = entry["path_units"].as_array() else {
        return false;
    };
    let raw = crate::tree::raw_units(std::ffi::OsStr::new(path));
    raw.len() == units.len()
        && raw
            .iter()
            .zip(units)
            .all(|(a, b)| b.as_u64() == Some(u64::from(*a)))
}
fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
fn parent_id(tree: &crate::tree::Snapshot, id: u32) -> Option<u32> {
    let parent = tree.nodes[id as usize].parent;
    (parent != crate::tree::NONE).then_some(parent)
}
fn node_error<'a>(node: &'a crate::tree::Node, fallback: &'a str) -> &'a str {
    node.error.as_ref().map_or(fallback, |e| e.as_str())
}
/// What a tile's area is proportional to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Metric {
    Logical,
    Allocated,
    Files,
}
impl Metric {
    fn label(self) -> &'static str {
        match self {
            Metric::Logical => t("Size", "大小"),
            Metric::Allocated => t("On disk", "磁盘占用"),
            Metric::Files => t("Files", "文件"),
        }
    }
    fn next(self) -> Metric {
        match self {
            Metric::Logical => Metric::Allocated,
            Metric::Allocated => Metric::Files,
            Metric::Files => Metric::Logical,
        }
    }
}
fn tile_weight(node: &crate::tree::Node, metric: Metric, hide_dot: bool) -> u64 {
    if hide_dot && node.name.to_string_lossy().starts_with('.') {
        return 0;
    }
    match metric {
        Metric::Logical => node.logical,
        Metric::Allocated => node.allocated,
        Metric::Files => node.files,
    }
}
const AMBER: Color32 = Color32::from_rgb(214, 168, 74);
const DANGER: Color32 = Color32::from_rgb(150, 74, 68);
const HATCH: Color32 = Color32::from_rgb(232, 226, 210);
/// Diagonal hatching marks space a cleanup suggestion says could be had back.
fn paint_hatch(ui: &egui::Ui, rect: egui::Rect) {
    let spacing = 9.0_f32;
    let stroke = egui::Stroke::new(1.0_f32, HATCH.linear_multiply(0.42));
    let painter = ui.painter().with_clip_rect(rect);
    let mut offset = 0.0;
    while offset < rect.width() + rect.height() {
        let from = pos2(rect.min.x + offset, rect.min.y);
        let to = pos2(rect.min.x + offset - rect.height(), rect.max.y);
        painter.line_segment([from, to], stroke);
        offset += spacing;
    }
}
/// Open the entry in Explorer, Finder or the configured file manager.
fn reveal(path: &std::path::Path) -> Result<(), String> {
    #[cfg(windows)]
    let mut command = {
        let mut command = std::process::Command::new("explorer.exe");
        command.arg(format!("/select,{}", path.display()));
        command
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = std::process::Command::new(t("open", "打开"));
        command.arg("-R").arg(path);
        command
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(path.parent().unwrap_or(path));
        command
    };
    #[cfg(not(any(unix, windows)))]
    let mut command = {
        let _ = path;
        return Err(t(
            "revealing paths is unsupported on this platform",
            "此平台不支持在文件管理器中显示路径",
        )
        .into());
    };
    command.spawn().map(|_| ()).map_err(|e| {
        crate::t_format!(
            "could not open the file manager: {e}",
            "无法打开文件管理器: {e}"
        )
    })
}
#[derive(Clone, Copy, Debug)]
struct DiskSpace {
    total: u64,
    free: u64,
    available: u64,
}
enum ScanResult {
    Summary(serde_json::Value),
    Tree(crate::tree::Snapshot),
}
struct App {
    path: String,
    scan_root: PathBuf,
    pending_scan: bool,
    filter: String,
    volumes: Vec<crate::volumes::Volume>,
    volume_rx: Option<Receiver<Result<Vec<crate::volumes::Volume>, String>>>,
    volumes_requested: bool,
    volume_status: String,
    #[cfg(test)]
    volume_combo_rect: Option<egui::Rect>,
    #[cfg(test)]
    volume_item_rects: Vec<(PathBuf, egui::Rect)>,
    threads: u8,
    engine: ScanEngine,
    full_tree: bool,
    scan_is_tree: bool,
    treemap_tiles: Vec<crate::treemap::Tile>,
    treemap_state: Option<(u32, egui::Rect, Metric, bool, usize)>,
    metric: Metric,
    depth: usize,
    hide_dot: bool,
    marked: HashSet<u32>,
    suggestions: HashSet<u32>,
    reclaim_dirs: HashSet<u32>,
    hints: HashMap<u32, &'static str>,
    focus: Option<u32>,
    hover_tile: Option<u32>,
    review_open: bool,
    help_open: bool,
    disk: Option<DiskSpace>,
    disk_rx: Option<Receiver<Result<DiskSpace, String>>>,
    tree: Option<Arc<crate::tree::Snapshot>>,
    duplicates: Option<crate::duplicates::Report>,
    duplicate_rx: Option<Receiver<Result<crate::duplicates::Report, String>>>,
    duplicate_cancelled: Arc<AtomicBool>,
    duplicate_progress: Arc<AtomicU64>,
    duplicate_window_open: bool,
    recommendations: Option<crate::recommend::Report>,
    recommendation_rx: Option<Receiver<Result<crate::recommend::Report, String>>>,
    recommendation_window_open: bool,
    search_window_open: bool,
    search_query: String,
    search_glob: bool,
    search_path_match: bool,
    search_kind: crate::search::Kind,
    search_min_bytes: u64,
    search_limit: u16,
    search_report: Option<crate::search::Report>,
    search_rx: Option<Receiver<Result<crate::search::Report, String>>>,
    search_submitted: String,
    #[cfg(test)]
    search_hit_rects: Vec<(PathBuf, egui::Rect)>,
    #[cfg(test)]
    suggestion_rects: Vec<(PathBuf, egui::Rect)>,
    tree_current: u32,
    expanded: HashSet<u32>,
    visible_rows: Vec<(u32, usize)>,
    visible_dirty: bool,
    list_rows: Vec<u32>,
    list_key: Option<(u32, String, Metric, bool)>,
    cancelled: Arc<AtomicBool>,
    progress: Arc<AtomicU64>,
    entries: Vec<serde_json::Value>,
    selected: Vec<PathBuf>,
    status: String,
    rx: Option<Receiver<Result<ScanResult, String>>>,
    cleanup_mode: CleanupMode,
    export_path: String,
    export_rx: Option<Receiver<Result<serde_json::Value, String>>>,
    plan_path: String,
    plan_data: Option<Vec<u8>>,
    plan_details: String,
    token: String,
    confirm: String,
}
impl Default for App {
    fn default() -> Self {
        let start = std::env::current_dir().unwrap_or_default();
        Self {
            path: start.display().to_string(),
            scan_root: start,
            pending_scan: false,
            filter: String::new(),
            volumes: Vec::new(),
            volume_rx: None,
            volumes_requested: false,
            volume_status: String::new(),
            #[cfg(test)]
            volume_combo_rect: None,
            #[cfg(test)]
            volume_item_rects: Vec::new(),
            threads: 4,
            engine: ScanEngine::Walk,
            full_tree: true,
            scan_is_tree: false,
            treemap_tiles: Vec::new(),
            treemap_state: None,
            metric: Metric::Logical,
            depth: 2,
            hide_dot: false,
            marked: HashSet::new(),
            suggestions: HashSet::new(),
            reclaim_dirs: HashSet::new(),
            hints: HashMap::new(),
            focus: None,
            hover_tile: None,
            review_open: false,
            help_open: false,
            disk: None,
            disk_rx: None,
            tree: None,
            duplicates: None,
            duplicate_rx: None,
            duplicate_cancelled: Arc::new(AtomicBool::new(false)),
            duplicate_progress: Arc::new(AtomicU64::new(0)),
            duplicate_window_open: false,
            recommendations: None,
            recommendation_rx: None,
            recommendation_window_open: false,
            search_window_open: false,
            search_query: String::new(),
            search_glob: false,
            search_path_match: false,
            search_kind: crate::search::Kind::All,
            search_min_bytes: 0,
            search_limit: 100,
            search_report: None,
            search_rx: None,
            search_submitted: String::new(),
            #[cfg(test)]
            search_hit_rects: Vec::new(),
            #[cfg(test)]
            suggestion_rects: Vec::new(),
            tree_current: 0,
            expanded: HashSet::new(),
            visible_rows: Vec::new(),
            visible_dirty: true,
            list_rows: Vec::new(),
            list_key: None,
            cancelled: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(AtomicU64::new(0)),
            entries: vec![],
            selected: vec![],
            status: t(
                "Choose a directory and scan. Sizes are logical bytes.",
                "选择目录并扫描，大小按逻辑字节统计。",
            )
            .into(),
            rx: None,
            plan_path: std::env::temp_dir()
                .join("filetree233-plan.json")
                .display()
                .to_string(),
            cleanup_mode: CleanupMode::Recycle,
            export_path: String::new(),
            export_rx: None,
            plan_data: None,
            plan_details: String::new(),
            token: String::new(),
            confirm: String::new(),
        }
    }
}
impl App {
    fn volume_picker(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        #[cfg(test)]
        self.volume_item_rects.clear();
        ui.horizontal(|ui| {
            let combo = egui::ComboBox::from_id_salt("volumes")
                .selected_text(t("Choose volume", "选择卷"))
                .width(240.0)
                .show_ui(ui, |ui| {
                    if self.volumes.is_empty() {
                        ui.label(t("No volumes loaded", "未加载卷"));
                    }
                    for volume in &self.volumes {
                        let label = crate::t_format!(
                            "{} {} {}{}{} available {} / {}",
                            "{} {} {}{}{} 可用 {} / {}",
                            volume.mount.display(),
                            volume.name,
                            volume.filesystem,
                            if volume.removable {
                                t(" removable", " 可移动")
                            } else {
                                ""
                            },
                            if volume.read_only {
                                t(" read-only", " 只读")
                            } else {
                                ""
                            },
                            if volume.total == 0 {
                                t("unknown", "未知").into()
                            } else {
                                format_bytes(volume.available)
                            },
                            if volume.total == 0 {
                                t("unknown", "未知").into()
                            } else {
                                format_bytes(volume.total)
                            }
                        );
                        let Some(path) = volume.mount.to_str() else {
                            ui.add_enabled(false, egui::Button::new(label));
                            continue;
                        };
                        let response = ui.selectable_label(false, label);
                        #[cfg(test)]
                        self.volume_item_rects
                            .push((volume.mount.clone(), response.rect));
                        if response.clicked() {
                            self.path = path.to_owned();
                            ui.close();
                        }
                    }
                });
            #[cfg(test)]
            {
                self.volume_combo_rect = Some(combo.response.rect);
            }
            #[cfg(not(test))]
            let _ = combo;
            if ui
                .add_enabled(
                    self.volume_rx.is_none(),
                    egui::Button::new(t("Refresh volumes", "刷新卷")),
                )
                .clicked()
            {
                self.begin_volumes(ctx);
            }
            if !self.volume_status.is_empty() {
                ui.label(&self.volume_status);
            }
        });
    }
    fn set_tree_path(&mut self, path: &std::path::Path) {
        if let Some(text) = path.to_str() {
            self.path = text.to_owned();
        } else {
            self.status = t("This path cannot be represented in the text field; navigation remains in the current snapshot", "该路径无法显示在文本框内；导航仍在当前快照中进行").into();
        }
    }
    fn begin_export(&mut self, format: crate::tree::ExportFormat, ctx: &egui::Context) {
        if self.export_rx.is_some() {
            return;
        }
        let Some(tree) = self.tree.as_ref().cloned() else {
            return;
        };
        let output = PathBuf::from(&self.export_path);
        let (tx, rx) = mpsc::channel();
        self.export_rx = Some(rx);
        self.status = t("Exporting snapshot...", "正在导出快照…").into();
        let ctx = ctx.clone();
        let cancelled = self.cancelled.clone();
        thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::tree::export_snapshot_controlled(&tree, &output, format, &cancelled)
            }))
            .unwrap_or_else(|_| {
                Err(t(
                    "export worker failed unexpectedly; a partial file may remain",
                    "导出任务意外失败，可能留下不完整文件",
                )
                .into())
            });
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
    fn begin_volumes(&mut self, ctx: &egui::Context) {
        if self.volume_rx.is_some() {
            return;
        }
        self.volumes_requested = true;
        self.volume_status = t("Loading volumes...", "正在加载卷…").into();
        let (tx, rx) = mpsc::channel();
        self.volume_rx = Some(rx);
        let ctx = ctx.clone();
        thread::spawn(move || {
            let result = std::panic::catch_unwind(crate::volumes::list).unwrap_or_else(|_| {
                Err(t("volume discovery failed unexpectedly", "卷发现意外失败").into())
            });
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
    fn begin_scan(&mut self, ctx: &egui::Context) {
        if self.rx.is_some() {
            return;
        }
        let path = PathBuf::from(&self.path);
        self.scan_root = path.clone();
        let threads = self.threads;
        let engine = self.engine;
        let tree_enabled = self.full_tree;
        self.scan_is_tree = tree_enabled;
        self.treemap_state = None;
        self.treemap_tiles.clear();
        self.marked.clear();
        self.suggestions.clear();
        self.reclaim_dirs.clear();
        self.hints.clear();
        self.focus = None;
        self.hover_tile = None;
        self.review_open = false;
        self.disk = None;
        self.disk_rx = None;
        let (cancelled, progress) = crate::tree::new_controls();
        self.cancelled = cancelled.clone();
        self.progress = progress.clone();
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        self.status = t("Scanning...", "正在扫描…").into();
        self.entries.clear();
        self.tree = None;
        self.duplicate_cancelled.store(true, Ordering::Relaxed);
        self.duplicate_rx = None;
        self.duplicates = None;
        self.duplicate_window_open = false;
        self.recommendations = None;
        self.recommendation_rx = None;
        self.recommendation_window_open = false;
        self.search_window_open = false;
        self.search_rx = None;
        self.search_report = None;
        self.tree_current = 0;
        self.expanded.clear();
        self.expanded.insert(0);
        self.visible_rows.clear();
        self.visible_dirty = true;
        self.list_rows.clear();
        self.list_key = None;
        self.selected.clear();
        self.plan_data = None;
        self.plan_details.clear();
        self.rx = Some(rx);
        thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if tree_enabled {
                    crate::scan_full_tree(&path, threads, engine, &cancelled, &progress)
                        .map(ScanResult::Tree)
                } else {
                    scan_dispatch(&path, threads, engine).map(ScanResult::Summary)
                }
            }))
            .unwrap_or_else(|_| {
                Err(t("scan worker failed unexpectedly", "扫描任务意外失败").into())
            });
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.pending_scan {
            self.pending_scan = false;
            self.begin_scan(ctx);
        }
        if !self.volumes_requested {
            self.begin_volumes(ctx);
        }
        if let Some(rx) = &self.volume_rx
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(volumes) => {
                    self.volume_status = if volumes.is_empty() {
                        t("No volumes detected", "未检测到卷").into()
                    } else {
                        String::new()
                    };
                    self.volumes = volumes;
                }
                Err(error) => {
                    self.volume_status =
                        crate::t_format!("Volume discovery: {error}", "卷发现: {error}")
                }
            }
            self.volume_rx = None;
        }
        if self.volume_rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        if let Some(rx) = &self.export_rx
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(value) => self.status = crate::t_format!("Exported: {value}", "已导出: {value}"),
                Err(error) => self.status = error,
            }
            self.export_rx = None;
        }
        if self.export_rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        if let Some(rx) = &self.disk_rx
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(space) => self.disk = Some(space),
                Err(error) => {
                    self.status = crate::t_format!("Volume capacity: {error}", "卷容量: {error}")
                }
            }
            self.disk_rx = None;
        }
        if self.disk_rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        if let Some(rx) = &self.rx
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(ScanResult::Tree(tree)) => {
                    let root_representable = if let Some(text) = tree.root.to_str() {
                        self.path = text.to_owned();
                        true
                    } else {
                        false
                    };
                    self.status = crate::t_format!(
                        "{} tree: {} entries, logical {} bytes, allocated {} bytes{}",
                        "{} 树: {} 项，逻辑 {} 字节，分配 {} 字节{}",
                        tree.engine,
                        tree.nodes.len().saturating_sub(1),
                        tree.nodes[0].logical,
                        tree.nodes[0].allocated,
                        if tree.nodes[0].complete {
                            ""
                        } else {
                            t(" (incomplete)", "（不完整）")
                        }
                    );
                    if !root_representable {
                        self.status.push_str(t(
                            "; root path cannot be represented in the text field",
                            "；根路径无法显示在文本框内",
                        ));
                    }
                    if let Some(reason) = &tree.fallback_reason {
                        self.status.push_str(&crate::t_format!(
                            "; fallback: {reason}",
                            "；回退原因: {reason}"
                        ));
                    }
                    self.scan_root = tree.root.clone();
                    let snapshot = Arc::new(tree);
                    self.begin_capacity(&snapshot.root, ctx);
                    self.tree = Some(snapshot);
                }
                Ok(ScanResult::Summary(v)) => {
                    self.entries = v["entries"].as_array().cloned().unwrap_or_default();
                    self.status = crate::t_format!(
                        "{}: {} entries. Attributed allocation: {} bytes{}",
                        "{}: {} 项，归属分配量 {} 字节{}",
                        v["engine"].as_str().unwrap_or("unknown"),
                        self.entries.len(),
                        v["total_allocated_bytes"].as_u64().unwrap_or(0),
                        if v["complete"] == false {
                            t(" (incomplete)", "（不完整）")
                        } else {
                            ""
                        }
                    );
                    if let Some(reason) = v["fallback_reason"].as_str() {
                        self.status.push_str(&format!("; fallback: {reason}"));
                    }
                }
                Err(e) => self.status = e,
            }
            self.rx = None;
        }
        if let Some(rx) = &self.duplicate_rx
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(report) => {
                    self.status = crate::t_format!(
                        "Duplicate search: {} groups{}",
                        "重复文件搜索: {} 组{}",
                        report.groups.len(),
                        if report.complete {
                            ""
                        } else {
                            t(" (incomplete)", "（不完整）")
                        }
                    );
                    self.duplicates = Some(report);
                    self.duplicate_window_open = true;
                }
                Err(e) => self.status = e,
            }
            self.duplicate_rx = None;
        }
        if let Some(rx) = &self.recommendation_rx
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(report) => {
                    self.status = crate::t_format!(
                        "Cleanup suggestions: {}{}",
                        "清理建议: {}{}",
                        report.total_candidates,
                        if report.complete {
                            ""
                        } else {
                            t(" (incomplete scan)", "（扫描不完整）")
                        }
                    );
                    let marks = self
                        .tree
                        .as_ref()
                        .map(|tree| suggestion_marks(tree, &report));
                    let (candidates, reclaim_dirs, hints) = marks.unwrap_or_default();
                    self.suggestions = candidates;
                    self.reclaim_dirs = reclaim_dirs;
                    self.hints = hints;
                    self.recommendations = Some(report);
                    self.recommendation_window_open = true;
                }
                Err(error) => self.status = error,
            }
            self.recommendation_rx = None;
        }
        if let Some(rx) = &self.search_rx
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(report) => {
                    self.status = crate::t_format!(
                        "Search: {} matches{}",
                        "搜索: {} 个匹配{}",
                        report.total_matches,
                        if report.complete {
                            ""
                        } else {
                            t(" (incomplete scan)", "（扫描不完整）")
                        }
                    );
                    self.search_report = Some(report);
                }
                Err(error) => {
                    self.status = error;
                    self.search_report = None;
                }
            }
            self.search_rx = None;
        }
        if self.search_rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        if self.recommendation_rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        if self.duplicate_rx.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.heading("filetree233");
            self.volume_picker(ui, ctx);
            ui.horizontal(|ui| {
                ui.label(t("Directory", "目录"));
                ui.add(egui::TextEdit::singleline(&mut self.path).desired_width(600.0));
                if ui.button(t("Scan", "扫描")).clicked() {
                    self.begin_scan(ctx);
                }
                if ui.button(t("Up", "上级")).clicked() {
                    self.goto_parent(ctx);
                }
                if self.rx.is_some()
                    && self.scan_is_tree
                    && ui.button(t("Cancel", "取消")).clicked()
                {
                    self.cancelled.store(true, Ordering::Relaxed);
                }
            });
            ui.horizontal(|ui| {
                egui::ComboBox::from_label(t("Engine", "引擎"))
                    .selected_text(format!("{:?}", self.engine))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.engine, ScanEngine::Walk, t("Walk", "遍历"));
                        ui.selectable_value(
                            &mut self.engine,
                            ScanEngine::Mft,
                            t("MFT (experimental)", "MFT（实验性）"),
                        );
                        ui.selectable_value(
                            &mut self.engine,
                            ScanEngine::Auto,
                            t("Auto (experimental)", "自动（实验性）"),
                        );
                    });
                ui.checkbox(&mut self.full_tree, t("Full tree", "完整树"));
                if self.rx.is_some() && self.scan_is_tree {
                    ui.label(crate::t_format!(
                        "{} entries",
                        "{} 个条目",
                        self.progress.load(Ordering::Relaxed)
                    ));
                    ctx.request_repaint_after(std::time::Duration::from_millis(250));
                }
                ui.label(t("Workers", "线程"));
                ui.add(egui::DragValue::new(&mut self.threads).range(1..=32));
                ui.label(t("Filter", "筛选"));
                ui.text_edit_singleline(&mut self.filter);
                ui.label(&self.status);
            });
        });
        egui::TopBottomPanel::bottom("bottom")
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(t("Export file", "导出文件"));
                    ui.add(egui::TextEdit::singleline(&mut self.export_path).desired_width(420.0));
                    let enabled = self.tree.is_some() && self.export_rx.is_none();
                    if ui
                        .add_enabled(enabled, egui::Button::new(t("Export CSV", "导出 CSV")))
                        .clicked()
                    {
                        self.begin_export(crate::tree::ExportFormat::Csv, ctx);
                    }
                    if ui
                        .add_enabled(enabled, egui::Button::new(t("Export JSONL", "导出 JSONL")))
                        .clicked()
                    {
                        self.begin_export(crate::tree::ExportFormat::Jsonl, ctx);
                    }
                    if self.export_rx.is_some() {
                        ui.label(t("Exporting...", "正在导出…"));
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(t("Plan file", "计划文件"));
                    ui.add(egui::TextEdit::singleline(&mut self.plan_path).desired_width(420.0));
                    egui::ComboBox::from_id_salt("cleanup_mode")
                        .selected_text(match self.cleanup_mode {
                            CleanupMode::Recycle => t("Recycle bin", "回收站"),
                            CleanupMode::Permanent => t("Permanent", "永久删除"),
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.cleanup_mode,
                                CleanupMode::Recycle,
                                t("Recycle bin", "回收站"),
                            );
                            ui.selectable_value(
                                &mut self.cleanup_mode,
                                CleanupMode::Permanent,
                                t("Permanent", "永久删除"),
                            );
                        });
                    if ui
                        .button(t("Create cleanup plan", "创建清理计划"))
                        .clicked()
                    {
                        self.create_plan();
                    }
                });
                if self.plan_data.is_some() && self.tree.is_none() {
                    ui.label(RichText::new(&self.plan_details).color(Color32::LIGHT_RED));
                    ui.horizontal(|ui| {
                        ui.label(t("Confirmation", "确认"));
                        ui.text_edit_singleline(&mut self.confirm);
                        if ui
                            .button(t("Apply confirmed plan", "执行已确认的计划"))
                            .clicked()
                        {
                            self.apply_plan(ctx);
                        }
                    });
                }
            });
        self.show_help(ctx);
        if self.tree.is_some() {
            self.show_tree(ctx);
            self.show_duplicates(ctx);
            self.show_recommendations(ctx);
            self.show_search(ctx);
            return;
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label(t(
                "Logical bytes | allocated bytes* | path (*hard links attributed once across scan)",
                "逻辑字节 | 分配字节* | 路径（*硬链接在整次扫描中只计一次）",
            ));
            let total: u64 = self
                .entries
                .iter()
                .filter_map(|e| e["bytes"].as_u64())
                .sum();
            let available = ui.available_width();
            ui.horizontal(|ui| {
                for (i, entry) in self.entries.iter().take(30).enumerate() {
                    let n = entry["bytes"].as_u64().unwrap_or(0);
                    if n == 0 || total == 0 {
                        continue;
                    }
                    let width = available * n as f32 / total as f32;
                    if width < 3.0 {
                        continue;
                    }
                    let (rect, response) =
                        ui.allocate_exact_size(Vec2::new(width, 42.0), Sense::hover());
                    let color = Color32::from_rgb(
                        40 + (i as u8).wrapping_mul(41) % 120,
                        90 + (i as u8).wrapping_mul(29) % 120,
                        140,
                    );
                    ui.painter().rect_filled(rect, 1.0, color);
                    response.on_hover_text(crate::t_format!(
                        "{}: {} bytes",
                        "{}: {} 字节",
                        entry["path"].as_str().unwrap_or("?"),
                        n
                    ));
                }
            });
            ui.separator();
            let mut navigate = None;
            egui::ScrollArea::vertical().show(ui, |ui| {
                for entry in &self.entries {
                    let path = entry["path"].as_str().unwrap_or("?");
                    if !path.to_lowercase().contains(&self.filter.to_lowercase()) {
                        continue;
                    }
                    let is_dir = entry["directories"].as_u64().unwrap_or(0) > 0;
                    let lossless = is_lossless_display_path(entry);
                    let raw_path = raw_entry_path(entry);
                    ui.horizontal(|ui| {
                        {
                            let mut selected =
                                raw_path.as_ref().is_some_and(|p| self.selected.contains(p));
                            if ui
                                .add_enabled(
                                    raw_path.is_some(),
                                    egui::Checkbox::new(&mut selected, ""),
                                )
                                .changed()
                                && let Some(p) = raw_path
                            {
                                if selected {
                                    self.selected.push(p);
                                } else {
                                    self.selected.retain(|x| x != &p);
                                }
                                self.plan_data = None;
                            }
                        }
                        if let Some(n) = entry["bytes"].as_u64() {
                            ui.monospace(format!(
                                "{n:>14} | {:>14}",
                                entry["allocated_attributed_bytes"].as_u64().unwrap_or(0)
                            ));
                        } else {
                            ui.colored_label(Color32::LIGHT_RED, t("ERROR", "错误"));
                        }
                        if is_dir && lossless {
                            if ui.link(path).clicked() {
                                navigate = Some(path.to_string());
                            }
                        } else {
                            ui.label(path);
                            if !lossless {
                                ui.colored_label(
                                    Color32::YELLOW,
                                    t(
                                        "raw path; see exact code units in plan",
                                        "原始路径；精确码元见计划文件",
                                    ),
                                );
                            }
                        }
                        if entry["complete"] == false {
                            ui.colored_label(
                                Color32::LIGHT_RED,
                                crate::t_format!(
                                    "Incomplete: {}",
                                    "不完整: {}",
                                    entry["error_count"].as_u64().unwrap_or(1)
                                ),
                            );
                        }
                        if let Some(error) = entry["error"].as_str() {
                            ui.colored_label(Color32::LIGHT_RED, error);
                        }
                    });
                }
            });
            if let Some(path) = navigate {
                self.path = path;
                self.begin_scan(ctx);
            }
        });
    }
}
impl App {
    fn show_tree(&mut self, ctx: &egui::Context) {
        let tree = self.tree.take().expect("tree is present");
        let mut navigate = None;
        egui::SidePanel::left("tree-panel")
            .resizable(true)
            .default_width(280.0)
            .show(ctx, |ui| {
                ui.heading(t("Directory tree", "目录树"));
                if ui.button(t("Search entire tree", "搜索整棵树")).clicked() {
                    self.search_window_open = true;
                }

                ui.horizontal(|ui| {
                    if self.duplicate_rx.is_none() {
                        if ui.button(t("Find duplicates", "查找重复文件")).clicked() {
                            self.begin_duplicates(Arc::clone(&tree), ctx);
                        }
                        if self.duplicates.is_some()
                            && ui.button(t("Show duplicates", "显示重复文件")).clicked()
                        {
                            self.duplicate_window_open = true;
                        }
                    } else {
                        ui.label(crate::t_format!(
                            "Hashing: {}",
                            "正在计算哈希: {}",
                            self.duplicate_progress.load(Ordering::Relaxed)
                        ));
                        if ui.button(t("Cancel search", "取消搜索")).clicked() {
                            self.duplicate_cancelled.store(true, Ordering::Relaxed);
                        }
                    }
                });
                ui.horizontal(|ui| {
                    if self.recommendation_rx.is_none() {
                        if ui.button(t("Suggest cleanup", "清理建议")).clicked() {
                            self.begin_recommendations(Arc::clone(&tree), ctx);
                        }
                        if self.recommendations.is_some()
                            && ui.button(t("Show suggestions", "显示建议")).clicked()
                        {
                            self.recommendation_window_open = true;
                        }
                    } else {
                        ui.label(t("Analyzing suggestions...", "正在分析清理建议…"));
                    }
                });
                if self.visible_dirty {
                    self.visible_rows.clear();
                    let mut pending = vec![(0u32, 0usize)];
                    while let Some((id, depth)) = pending.pop() {
                        self.visible_rows.push((id, depth));
                        if tree.nodes[id as usize].is_dir && self.expanded.contains(&id) {
                            for child in tree.child_ids(id).collect::<Vec<u32>>().into_iter().rev()
                            {
                                pending.push((child, depth + 1));
                            }
                        }
                    }
                    self.visible_dirty = false;
                }
                let rows = std::mem::take(&mut self.visible_rows);
                egui::ScrollArea::vertical().show_rows(ui, 22.0, rows.len(), |ui, range| {
                    for index in range {
                        let (id, depth) = rows[index];
                        let node = &tree.nodes[id as usize];
                        ui.horizontal(|ui| {
                            ui.add_space((depth.min(20) * 14) as f32);
                            if node.is_dir {
                                let symbol = if self.expanded.contains(&id) {
                                    "▾"
                                } else {
                                    "▸"
                                };
                                if ui.small_button(symbol).clicked() {
                                    if !self.expanded.remove(&id) {
                                        self.expanded.insert(id);
                                    }
                                    self.visible_dirty = true;
                                }
                            } else {
                                ui.label("  ");
                            }
                            let label = if id == 0 {
                                tree.root.display().to_string()
                            } else {
                                node.name.to_string_lossy().into_owned()
                            };
                            if ui
                                .selectable_label(id == self.tree_current, label)
                                .clicked()
                                && node.is_dir
                            {
                                navigate = Some(id);
                            }
                            if !node.complete {
                                ui.colored_label(Color32::LIGHT_RED, "!").on_hover_text(
                                    node_error(
                                        node,
                                        t("Contains incomplete entries", "包含不完整条目"),
                                    ),
                                );
                            }
                        });
                    }
                });
                self.visible_rows = rows;
            });
        self.handle_keys(ctx, &tree);
        self.show_info_panel(ctx, &tree);
        egui::CentralPanel::default().show(ctx, |ui| {
            self.treemap_toolbar(ui, &tree, ctx);
            let current = self.tree_current;
            let list_height = 220.0_f32;
            let size = Vec2::new(
                ui.available_width(),
                (ui.available_height() - list_height).max(200.0),
            );
            let (area, _) = ui.allocate_exact_size(size, Sense::hover());
            let state = (current, area, self.metric, self.hide_dot, self.depth);
            if self.treemap_state != Some(state) {
                let metric = self.metric;
                let hide_dot = self.hide_dot;
                self.treemap_tiles = crate::treemap::layout(
                    &tree,
                    current,
                    area,
                    2048,
                    self.depth,
                    &move |node: &crate::tree::Node| tile_weight(node, metric, hide_dot),
                );
                self.treemap_state = Some(state);
            }
            let mut tiles = std::mem::take(&mut self.treemap_tiles);
            let mut open_target = None;
            let mut mark_target = None;
            let mut reveal_target = None;
            let mut hovered = None;
            for tile in tiles.iter_mut() {
                let id = tile.id;
                let rect = tile.rect;
                let node = &tree.nodes[id as usize];
                let marked = self.marked.contains(&id);
                let covered = !tile.remainder && self.marked_ancestor(&tree, id).is_some();
                let reclaim = !tile.remainder && self.reclaim_dirs.contains(&id);
                let suggestion_rule = self.hints.get(&id).copied();
                let base = if tile.remainder {
                    Color32::from_rgb(66, 66, 74)
                } else {
                    crate::classify::shade(
                        crate::classify::classify(&node.name, node.is_dir).color(),
                        tile.depth,
                    )
                };
                let fill = if marked {
                    DANGER
                } else if covered {
                    DANGER.linear_multiply(0.7)
                } else {
                    base
                };
                ui.painter().rect_filled(rect.shrink(0.5), 1.0, fill);
                if reclaim && !marked && !covered {
                    paint_hatch(ui, rect);
                }
                let label = if tile.remainder {
                    t("other entries", "其他条目").to_owned()
                } else if id == 0 {
                    tree.root.to_string_lossy().into_owned()
                } else {
                    node.name.to_string_lossy().into_owned()
                };
                if rect.width() > 44.0 && rect.height() > 15.0 {
                    let room_for_size = rect.width() > 108.0 && rect.height() > 30.0;
                    let amount = if self.metric == Metric::Files {
                        crate::t_format!("{} files", "{} 个文件", tile.weight)
                    } else {
                        format_bytes(tile.weight)
                    };
                    let text = if room_for_size {
                        format!("{label}\n{amount}")
                    } else {
                        label.clone()
                    };
                    ui.painter().with_clip_rect(rect).text(
                        rect.min + Vec2::new(4.0, 3.0),
                        egui::Align2::LEFT_TOP,
                        text,
                        egui::FontId::proportional(if tile.depth == 0 { 13.0 } else { 11.0 }),
                        Color32::from_gray(238),
                    );
                }
                if self.focus == Some(id) {
                    ui.painter().rect_stroke(
                        rect.shrink(1.5),
                        1.0,
                        egui::Stroke::new(2.0_f32, AMBER),
                        egui::StrokeKind::Inside,
                    );
                } else if marked {
                    ui.painter().rect_stroke(
                        rect.shrink(1.5),
                        1.0,
                        egui::Stroke::new(1.5_f32, Color32::WHITE),
                        egui::StrokeKind::Inside,
                    );
                }
                let response = ui
                    .interact(rect, ui.id().with(("tile", id, tile.depth)), Sense::click())
                    .on_hover_ui(|ui| {
                        if tile.remainder {
                            ui.label(crate::t_format!(
                                "Entries too small to draw\n{}",
                                "条目太小无法绘制\n{}",
                                format_bytes(tile.weight)
                            ));
                            return;
                        }
                        ui.label(tree.path(id).display().to_string());
                        ui.monospace(crate::t_format!(
                            "{} logical\n{} on disk\n{} files",
                            "{} 逻辑\n{} 磁盘\n{} 文件",
                            format_bytes(node.logical),
                            format_bytes(node.allocated),
                            node.files
                        ));
                        let root_total = tree.nodes[0].logical.max(1);
                        ui.label(crate::t_format!(
                            "{:.1}% of the scan by size",
                            "占扫描总量的 {:.1}%（按大小）",
                            node.logical as f64 * 100.0 / root_total as f64
                        ));
                        if let Some(rule) = suggestion_rule {
                            ui.colored_label(
                                HATCH,
                                crate::t_format!("cleanup hint: {rule}", "清理提示: {rule}"),
                            );
                        }
                        if marked {
                            ui.colored_label(DANGER, t("marked for cleanup", "已标记清理"));
                        } else if covered {
                            ui.colored_label(
                                DANGER,
                                t("inside a marked directory", "位于已标记的目录内"),
                            );
                        }
                        if !node.complete {
                            ui.colored_label(
                                Color32::LIGHT_RED,
                                node_error(node, t("incomplete", "不完整")),
                            );
                        }
                        ui.label(t(
                            "Click a directory to open it, Space to mark, / for keys.",
                            "单击目录进入，空格标记，/ 查看快捷键。",
                        ));
                    });
                if response.hovered() {
                    hovered = Some(id);
                }
                let held = ctx.input(|i| i.modifiers.command || i.modifiers.ctrl);
                if response.clicked() && !tile.remainder {
                    self.focus = Some(id);
                    if held || !node.is_dir {
                        mark_target = Some(id);
                    } else if id != current {
                        open_target = Some(id);
                    }
                }
                if response.secondary_clicked() && !tile.remainder {
                    self.focus = Some(id);
                }
                if !tile.remainder {
                    response.context_menu(|ui| {
                        if ui
                            .button(if marked {
                                t("Unmark", "取消标记")
                            } else {
                                t("Mark for cleanup", "标记为待清理")
                            })
                            .clicked()
                        {
                            mark_target = Some(id);
                            ui.close();
                        }
                        if node.is_dir && ui.button(t("Open here", "在此打开")).clicked() {
                            open_target = Some(id);
                            ui.close();
                        }
                        if ui
                            .button(t("Show in file manager", "在文件管理器中显示"))
                            .clicked()
                        {
                            reveal_target = Some(tree.path(id));
                            ui.close();
                        }
                    });
                }
            }
            self.treemap_tiles = tiles;
            self.hover_tile = hovered;
            if let Some(id) = mark_target {
                self.toggle_mark(&tree, id);
            }
            if let Some(path) = reveal_target
                && let Err(error) = reveal(&path)
            {
                self.status = error;
            }
            if let Some(id) = open_target {
                navigate = Some(id);
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(crate::t_format!(
                    "{} entries here | logical | on disk | files | name",
                    "{} 项 | 逻辑 | 磁盘 | 文件 | 名称",
                    tree.child_ids(current).count()
                ));
                if self.rx.is_none()
                    && ui
                        .small_button(if self.marked.contains(&current) {
                            t("Unmark this folder", "取消标记此目录")
                        } else {
                            t("Mark this folder", "标记此目录")
                        })
                        .clicked()
                {
                    self.toggle_mark(&tree, current);
                }
            });
            let key = (current, self.filter.clone(), self.metric, self.hide_dot);
            if self.list_key.as_ref() != Some(&key) {
                let filter = self.filter.to_lowercase();
                let mut rows: Vec<u32> = tree
                    .child_ids(current)
                    .filter(|id| {
                        let node = &tree.nodes[*id as usize];
                        tile_weight(node, self.metric, self.hide_dot) > 0
                            && node.name.to_string_lossy().to_lowercase().contains(&filter)
                    })
                    .collect();
                rows.sort_by(|a, b| {
                    let (a, b) = (&tree.nodes[*a as usize], &tree.nodes[*b as usize]);
                    tile_weight(b, key.2, key.3)
                        .cmp(&tile_weight(a, key.2, key.3))
                        .then_with(|| a.name.cmp(&b.name))
                });
                self.list_rows = rows;
                self.list_key = Some(key);
            }
            let children = std::mem::take(&mut self.list_rows);
            egui::ScrollArea::vertical()
                .id_salt("child-list")
                .show_rows(ui, 23.0, children.len(), |ui, range| {
                    for row in range {
                        let id = children[row];
                        let node = &tree.nodes[id as usize];
                        let marked = self.marked.contains(&id);
                        let covered = self.marked_ancestor(&tree, id).is_some();
                        ui.horizontal(|ui| {
                            let mut checked = marked;
                            let checkbox =
                                ui.add_enabled(!covered, egui::Checkbox::new(&mut checked, ""));
                            if checkbox.changed() {
                                self.toggle_mark(&tree, id);
                            }
                            ui.monospace(format!(
                                "{:>9} | {:>9} | {:>7}",
                                format_bytes(node.logical),
                                format_bytes(node.allocated),
                                node.files
                            ));
                            if node.is_dir {
                                if ui.link(node.name.to_string_lossy()).clicked() {
                                    navigate = Some(id);
                                }
                            } else {
                                ui.label(node.name.to_string_lossy());
                            }
                            if marked {
                                ui.colored_label(DANGER, t("marked", "已标记"));
                            } else if covered {
                                ui.colored_label(DANGER, t("in marked folder", "在已标记目录内"))
                                    .on_hover_text(t(
                                        "A parent directory is already marked",
                                        "上级目录已被标记",
                                    ));
                            }
                            if self.reclaim_dirs.contains(&id) {
                                ui.colored_label(AMBER, t("reclaimable", "可能回收"));
                            }
                            if !node.complete {
                                ui.colored_label(Color32::LIGHT_RED, t("incomplete", "不完整"))
                                    .on_hover_text(node_error(
                                        node,
                                        t("Contains incomplete entries", "包含不完整条目"),
                                    ));
                            }
                        });
                    }
                });
            self.list_rows = children;
        });
        if let Some(id) = navigate {
            self.open_node(&tree, id);
        }
        self.show_review(ctx, &tree);
        if self.rx.is_none() {
            self.tree = Some(tree);
        }
    }
}

/// Direction the keyboard focus travels across treemap tiles.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// `id` and every directory above it, closest parent first.
fn ancestor_ids(tree: &crate::tree::Snapshot, id: u32) -> Vec<u32> {
    let mut chain = Vec::new();
    let mut cursor = parent_id(tree, id);
    while let Some(parent) = cursor {
        chain.push(parent);
        cursor = parent_id(tree, parent);
    }
    chain
}

fn is_ancestor(tree: &crate::tree::Snapshot, ancestor: u32, id: u32) -> bool {
    ancestor_ids(tree, id).contains(&ancestor)
}

/// Nodes covered by a cleanup suggestion: the candidates themselves plus every
/// directory that contains one, so a hatch on a parent means the space can be
/// had back somewhere below it.
fn suggestion_marks(
    tree: &crate::tree::Snapshot,
    report: &crate::recommend::Report,
) -> (HashSet<u32>, HashSet<u32>, HashMap<u32, &'static str>) {
    let mut candidates = HashSet::new();
    let mut reclaim = HashSet::new();
    let mut hints = HashMap::new();
    for candidate in &report.candidates {
        let id = candidate.node;
        if tree.nodes.get(id as usize).is_none() {
            continue;
        }
        candidates.insert(id);
        hints.insert(id, candidate.rule);
        if reclaim.insert(id) {
            for parent in ancestor_ids(tree, id) {
                if !reclaim.insert(parent) {
                    break;
                }
            }
        }
    }
    (candidates, reclaim, hints)
}

fn key_help() -> [(&'static str, &'static str); 15] {
    [
        (
            "click",
            t(
                "open a directory, mark a file",
                "单击目录进入，单击文件标记",
            ),
        ),
        (
            "ctrl+click",
            t("mark a directory without entering it", "仅标记目录，不进入"),
        ),
        (
            "space",
            t("mark or unmark the focused item", "标记或取消标记聚焦项"),
        ),
        (
            "x",
            t(
                "mark the focused directory and go up",
                "标记聚焦的目录并返回上级",
            ),
        ),
        ("enter", t("open the focused directory", "进入聚焦的目录")),
        (
            "arrows",
            t("move focus between tiles", "在方块之间移动焦点"),
        ),
        (
            "tab",
            t(
                "next and previous entry here (shift+tab back)",
                "在本级条目间前后切换（Shift+Tab 反向）",
            ),
        ),
        ("backspace", t("go up one directory", "返回上一级目录")),
        ("g", t("go to the scan root", "回到扫描根目录")),
        ("t", t("change what tile area means", "切换方块面积的指标")),
        (
            "i",
            t("ignore entries starting with a dot", "忽略点号开头的条目"),
        ),
        (
            "[ / ]",
            t("draw fewer or more nesting levels", "减少或增加绘制层数"),
        ),
        ("c", t("review the marked items", "核对已标记项")),
        (
            "o",
            t(
                "show the focused item in the file manager",
                "在文件管理器中显示聚焦项",
            ),
        ),
        (
            "r, /, q",
            t("rescan, this help, quit", "重新扫描、快捷键帮助、退出"),
        ),
    ]
}

impl App {
    fn open_node(&mut self, tree: &crate::tree::Snapshot, id: u32) {
        self.tree_current = id;
        let mut expand = ancestor_ids(tree, id);
        expand.push(id);
        for parent in expand {
            if self.expanded.insert(parent) {
                self.visible_dirty = true;
            }
        }
        let path = tree.path(id);
        self.set_tree_path(&path);
    }
    fn goto_parent(&mut self, ctx: &egui::Context) {
        let snapshot = self.tree.clone();
        if let Some(tree) = &snapshot {
            if let Some(parent) = parent_id(tree, self.tree_current) {
                self.open_node(tree, parent);
            }
        } else if let Some(parent) = Path::new(&self.path).parent() {
            let text = parent.display().to_string();
            self.path = text;
            self.begin_scan(ctx);
        }
    }
    /// Closest marked directory above `id`; its mark already covers this space.
    fn marked_ancestor(&self, tree: &crate::tree::Snapshot, id: u32) -> Option<u32> {
        ancestor_ids(tree, id)
            .into_iter()
            .find(|parent| self.marked.contains(parent))
    }
    fn toggle_mark(&mut self, tree: &crate::tree::Snapshot, id: u32) {
        if self.marked.remove(&id) {
            self.forget_plan();
            return;
        }
        if let Some(covered) = self
            .marked
            .iter()
            .copied()
            .find(|other| is_ancestor(tree, *other, id))
        {
            self.status = crate::t_format!(
                "{} is already inside the marked {}",
                "{} 已在已标记的 {} 内",
                tree.path(id).display(),
                tree.path(covered).display()
            );
            return;
        }
        self.marked.insert(id);
        // Overlapping targets cannot be planned, so collapse nested marks.
        let inside: Vec<u32> = self
            .marked
            .iter()
            .copied()
            .filter(|other| *other != id && is_ancestor(tree, id, *other))
            .collect();
        for other in inside {
            self.marked.remove(&other);
        }
        self.forget_plan();
    }
    fn forget_plan(&mut self) {
        self.plan_data = None;
        self.plan_details.clear();
    }
    /// Plan targets: the top-level marks plus explicit picks from the dialogs.
    fn plan_inputs(&self, tree: &crate::tree::Snapshot) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self
            .marked
            .iter()
            .copied()
            .map(|id| tree.path(id))
            .collect();
        for path in &self.selected {
            if !paths.contains(path) {
                paths.push(path.clone());
            }
        }
        paths.sort();
        paths
    }
    /// Bytes covered by marks. Nested marks collapse, so nothing counts twice.
    fn marked_sizes(&self, tree: &crate::tree::Snapshot) -> (u64, u64) {
        self.marked.iter().fold((0u64, 0u64), |acc, id| {
            let Some(node) = tree.nodes.get(*id as usize) else {
                return acc;
            };
            (
                acc.0.saturating_add(node.logical),
                acc.1.saturating_add(node.allocated),
            )
        })
    }
    fn forget_target(&mut self, tree: &crate::tree::Snapshot, path: &Path) {
        let marked = self
            .marked
            .iter()
            .copied()
            .find(|id| tree.path(*id) == path);
        if let Some(id) = marked {
            self.marked.remove(&id);
        }
        let path = path.to_path_buf();
        self.selected.retain(|p| p != &path);
        self.forget_plan();
    }
    fn rescan(&mut self, ctx: &egui::Context) {
        if let Some(text) = self.scan_root.to_str() {
            self.path = text.to_owned();
        }
        self.begin_scan(ctx);
    }
    fn create_plan(&mut self) {
        let inputs = match &self.tree {
            Some(tree) => self.plan_inputs(tree),
            None => self.selected.clone(),
        };
        let output = PathBuf::from(&self.plan_path);
        match save_plan_mode(&inputs, &output, self.cleanup_mode) {
            Ok(_) => match read_checked_plan(&output) {
                Ok((data, plan)) => {
                    let (message, token) = plan_description(&data, &plan);
                    self.plan_details = message;
                    self.token = token;
                    self.confirm.clear();
                    self.plan_data = Some(data);
                    self.status = t(
                        "Cleanup plan verified; type the confirmation code to apply.",
                        "清理计划已校验，输入确认码后执行。",
                    )
                    .into();
                }
                Err(e) => self.status = e,
            },
            Err(e) => self.status = e,
        }
    }
    fn apply_plan(&mut self, ctx: &egui::Context) {
        if self.confirm != self.token {
            self.status = t(
                "Confirmation did not match; nothing deleted",
                "确认码不匹配，未删除任何文件",
            )
            .into();
            return;
        }
        let Some(data) = self.plan_data.take() else {
            self.status = t("Create a cleanup plan first", "请先创建清理计划").into();
            return;
        };
        match delete_checked(&PathBuf::from(&self.plan_path), &data) {
            Ok(v) => {
                self.status = format!("{v}");
                self.rescan(ctx);
            }
            Err(e) => self.status = e,
        }
    }
    fn begin_capacity(&mut self, root: &Path, ctx: &egui::Context) {
        if self.disk_rx.is_some() {
            return;
        }
        let path = root.to_path_buf();
        let (tx, rx) = mpsc::channel();
        self.disk_rx = Some(rx);
        let ctx = ctx.clone();
        thread::spawn(move || {
            let result = std::panic::catch_unwind(|| crate::volumes::capacity(&path))
                .unwrap_or_else(|_| {
                    Err(t(
                        "capacity worker failed unexpectedly",
                        "容量查询任务意外失败",
                    )
                    .into())
                })
                .map(|value| DiskSpace {
                    total: value["total_bytes"].as_u64().unwrap_or(0),
                    free: value["free_bytes"].as_u64().unwrap_or(0),
                    available: value["available_bytes"].as_u64().unwrap_or(0),
                });
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
    fn treemap_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        tree: &crate::tree::Snapshot,
        ctx: &egui::Context,
    ) {
        ui.horizontal_wrapped(|ui| {
            let mut chain = vec![self.tree_current];
            chain.extend(ancestor_ids(tree, self.tree_current).into_iter().rev());
            for (index, id) in chain.iter().copied().enumerate() {
                if index > 0 {
                    ui.label("/");
                }
                let label = if id == 0 {
                    tree.root.to_string_lossy().into_owned()
                } else {
                    tree.nodes[id as usize].name.to_string_lossy().into_owned()
                };
                if ui
                    .selectable_label(id == self.tree_current, label)
                    .clicked()
                {
                    self.open_node(tree, id);
                }
            }
        });
        ui.horizontal(|ui| {
            if ui
                .button(crate::t_format!(
                    "Area: {}",
                    "面积: {}",
                    self.metric.label()
                ))
                .clicked()
            {
                self.metric = self.metric.next();
            }
            ui.add(
                egui::DragValue::new(&mut self.depth)
                    .range(1..=4)
                    .suffix(t(" levels", " 层")),
            );
            ui.checkbox(&mut self.hide_dot, t("Hide dotfiles", "隐藏点号文件"));
            if !self.marked.is_empty() {
                let (logical, _) = self.marked_sizes(tree);
                if ui
                    .button(crate::t_format!(
                        "Review {} ({})",
                        "核对 {}（{}）",
                        self.marked.len(),
                        format_bytes(logical)
                    ))
                    .clicked()
                {
                    self.review_open = true;
                }
                if ui.small_button(t("Clear marks", "清除标记")).clicked() {
                    self.marked.clear();
                    self.forget_plan();
                }
            }
            if ui
                .add_enabled(
                    self.rx.is_none(),
                    egui::Button::new(t("Rescan", "重新扫描")),
                )
                .clicked()
            {
                self.rescan(ctx);
            }
            if ui.button(t("Keys", "快捷键")).clicked() {
                self.help_open = true;
            }
            let next_language = match crate::i18n::current() {
                crate::i18n::Lang::Zh => crate::i18n::Lang::En,
                crate::i18n::Lang::En => crate::i18n::Lang::Zh,
            };
            let next_label = match next_language {
                crate::i18n::Lang::Zh => "中文",
                crate::i18n::Lang::En => "English",
            };
            if ui
                .button(next_label)
                .on_hover_text(t("Switch interface language", "切换界面语言"))
                .clicked()
            {
                crate::i18n::set(next_language);
            }
        });
        ui.separator();
    }
    fn show_info_panel(&mut self, ctx: &egui::Context, tree: &crate::tree::Snapshot) {
        egui::SidePanel::right("details")
            .resizable(true)
            .default_width(300.0)
            .min_width(220.0)
            .show(ctx, |ui| {
                ui.heading(t("Volume", "卷"));
                self.disk_panel(ui, tree);
                ui.separator();
                ui.heading(t("Entry", "条目"));
                match self.hover_tile.or(self.focus) {
                    Some(id) => self.node_panel(ui, tree, id),
                    None => {
                        ui.label(t(
                            "Hover a tile or press an arrow key to inspect an entry.",
                            "悬停方块或按方向键查看条目。",
                        ));
                    }
                }
                ui.separator();
                ui.heading(t("Marks", "标记清单"));
                self.marks_panel(ui, tree);
                ui.separator();
                ui.heading(t("Worth a look", "值得一看"));
                self.suggestion_panel(ui, tree);
            });
    }
    fn disk_panel(&mut self, ui: &mut egui::Ui, tree: &crate::tree::Snapshot) {
        match self.disk {
            Some(space) if space.total > 0 => {
                let used = space.total.saturating_sub(space.available);
                let fraction = used as f32 / space.total as f32;
                ui.add(egui::ProgressBar::new(fraction).text(crate::t_format!(
                    "{:.0}% used",
                    "{:.0}% 已用",
                    fraction * 100.0
                )));
                ui.label(crate::t_format!(
                    "{} available ({} free) of {}",
                    "{} 可用（{} 空闲），共 {}",
                    format_bytes(space.available),
                    format_bytes(space.free),
                    format_bytes(space.total)
                ));
                let (logical, allocated) = self.marked_sizes(tree);
                if logical > 0 {
                    let projected = space.available.saturating_add(logical).min(space.total);
                    ui.label(crate::t_format!(
                        "{} marked — free space could become {}",
                        "已标记 {} — 可用空间可变为 {}",
                        format_bytes(logical),
                        format_bytes(projected)
                    ));
                    ui.label(crate::t_format!(
                        "{} attributed on disk, not guaranteed reclaimed space",
                        "{} 归属磁盘占用，不保证可回收",
                        format_bytes(allocated)
                    ));
                }
            }
            Some(_) => {
                ui.label(t(
                    "This volume did not report a capacity.",
                    "该卷未报告容量。",
                ));
            }
            None => {
                ui.label(t(
                    "Reading the capacity of the scanned volume...",
                    "正在读取扫描卷的容量…",
                ));
            }
        }
    }
    fn node_panel(&mut self, ui: &mut egui::Ui, tree: &crate::tree::Snapshot, id: u32) {
        let Some(node) = tree.nodes.get(id as usize) else {
            ui.label(t(
                "This entry is no longer in the snapshot.",
                "该条目已不在快照中。",
            ));
            return;
        };
        let name = if id == 0 {
            tree.root.to_string_lossy().into_owned()
        } else {
            node.name.to_string_lossy().into_owned()
        };
        ui.strong(name);
        ui.label(tree.path(id).display().to_string());
        let kind = crate::classify::classify(&node.name, node.is_dir);
        ui.colored_label(kind.color(), format!("{kind:?}"));
        ui.monospace(crate::t_format!(
            "{} logical | {} on disk | {} files | {} dirs",
            "{} 逻辑 | {} 磁盘 | {} 文件 | {} 目录",
            format_bytes(node.logical),
            format_bytes(node.allocated),
            node.files,
            node.directories
        ));
        if let Some(rule) = self.hints.get(&id).copied() {
            ui.colored_label(
                AMBER,
                crate::t_format!("suggested by rule: {rule}", "规则建议: {rule}"),
            );
        }
        if self.marked.contains(&id) {
            ui.colored_label(DANGER, t("marked for cleanup", "已标记清理"));
        } else if let Some(covered) = self.marked_ancestor(tree, id) {
            ui.colored_label(
                DANGER,
                crate::t_format!(
                    "inside marked {}",
                    "位于已标记的 {} 内",
                    tree.path(covered).display()
                ),
            );
        }
        if !node.complete {
            ui.colored_label(
                Color32::LIGHT_RED,
                node_error(node, t("Contains incomplete entries", "包含不完整条目")),
            );
        }
        let path = tree.path(id);
        let marked = self.marked.contains(&id);
        ui.horizontal(|ui| {
            if ui
                .small_button(if marked {
                    t("Unmark", "取消标记")
                } else {
                    t("Mark", "标记")
                })
                .clicked()
            {
                self.toggle_mark(tree, id);
            }
            if node.is_dir
                && id != self.tree_current
                && ui.small_button(t("Open", "打开")).clicked()
            {
                self.open_node(tree, id);
            }
            if ui.small_button(t("Show", "显示")).clicked()
                && let Err(error) = reveal(&path)
            {
                self.status = error;
            }
        });
    }
    fn marks_panel(&mut self, ui: &mut egui::Ui, tree: &crate::tree::Snapshot) {
        if self.marked.is_empty() {
            ui.label(t(
                "Nothing marked. Space marks the focused tile, ctrl+click marks a directory.",
                "尚未标记。空格标记聚焦方块，Ctrl+单击标记目录。",
            ));
            return;
        }
        let mut rows: Vec<(u32, PathBuf)> = self
            .marked
            .iter()
            .copied()
            .map(|id| (id, tree.path(id)))
            .collect();
        rows.sort_by(|a, b| a.1.cmp(&b.1));
        let mut unmark = None;
        egui::ScrollArea::vertical()
            .id_salt("mark-list")
            .max_height(160.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (id, path) in &rows {
                    let logical = tree
                        .nodes
                        .get(*id as usize)
                        .map(|node| node.logical)
                        .unwrap_or(0);
                    ui.horizontal(|ui| {
                        let label = path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string());
                        ui.label(format!("{} · {}", format_bytes(logical), label))
                            .on_hover_text(path.display().to_string());
                        if ui.small_button(t("unmark", "取消标记")).clicked() {
                            unmark = Some(*id);
                        }
                    });
                }
            });
        if let Some(id) = unmark {
            self.toggle_mark(tree, id);
        }
    }
    fn suggestion_panel(&mut self, ui: &mut egui::Ui, tree: &crate::tree::Snapshot) {
        let Some(report) = self.recommendations.as_ref() else {
            ui.label(t(
                "Run Suggest cleanup in the tree panel to hatch reclaimable space.",
                "在目录树面板运行“清理建议”，可斜纹标注可能回收的空间。",
            ));
            return;
        };
        if report.candidates.is_empty() {
            ui.label(t(
                "No rules matched. That is not proof the space is needed.",
                "没有规则命中，但这并不证明这些空间是必需的。",
            ));
            return;
        }
        ui.label(crate::t_format!(
            "{} suggestions, {} listed",
            "{} 条建议，列出 {} 条",
            report.total_candidates,
            report.candidates.len()
        ));
        let mut open_target = None;
        let mut mark_target = None;
        egui::ScrollArea::vertical()
            .id_salt("suggestion-list")
            .max_height(200.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for candidate in report.candidates.iter().take(12) {
                    if tree.nodes.get(candidate.node as usize).is_none() {
                        continue;
                    }
                    ui.horizontal(|ui| {
                        ui.label(format!(
                            "{} · {}",
                            format_bytes(candidate.logical),
                            candidate.rule
                        ))
                        .on_hover_text(candidate.path.display().to_string());
                        if ui.small_button(t("open", "打开")).clicked() {
                            open_target = Some(candidate.node);
                        }
                        if ui.small_button(t("mark", "标记")).clicked() {
                            mark_target = Some(candidate.node);
                        }
                    });
                }
            });
        if let Some(id) = open_target {
            let node = &tree.nodes[id as usize];
            let target = if node.is_dir { id } else { node.parent };
            if target != crate::tree::NONE {
                self.open_node(tree, target);
            }
        }
        if let Some(id) = mark_target {
            self.toggle_mark(tree, id);
        }
    }
    fn show_review(&mut self, ctx: &egui::Context, tree: &crate::tree::Snapshot) {
        if !self.review_open {
            return;
        }
        let mut open = self.review_open;
        egui::Window::new(t("Review marked items", "核对标记项"))
            .open(&mut open)
            .resizable(true)
            .default_size([620.0, 420.0])
            .show(ctx, |ui| {
                ui.label(t("Marks become the cleanup plan targets. Nothing is deleted until the plan is re-read from disk and you type the confirmation code.", "标记项将成为清理计划的目标。只有从磁盘重新读取计划并输入确认码后才会删除。"));
                let inputs = self.plan_inputs(tree);
                if inputs.is_empty() {
                    ui.label(t("Nothing marked yet.", "还没有标记任何内容。"));
                }
                egui::ScrollArea::vertical()
                    .id_salt("review-list")
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for path in &inputs {
                            ui.horizontal(|ui| {
                                ui.label(path.display().to_string());
                                if ui.small_button(t("remove", "移除")).clicked() {
                                    self.forget_target(tree, path);
                                }
                            });
                        }
                    });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button(t("Create cleanup plan", "创建清理计划")).clicked() {
                        self.create_plan();
                    }
                    if ui.small_button(t("Clear marks", "清除标记")).clicked() {
                        self.marked.clear();
                        self.forget_plan();
                    }
                    egui::ComboBox::from_id_salt("review_cleanup_mode")
                        .selected_text(match self.cleanup_mode {
                            CleanupMode::Recycle => t("Recycle bin", "回收站"),
                            CleanupMode::Permanent => t("Permanent", "永久删除"),
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.cleanup_mode,
                                CleanupMode::Recycle,
                                t("Recycle bin", "回收站"),
                            );
                            ui.selectable_value(
                                &mut self.cleanup_mode,
                                CleanupMode::Permanent,
                                t("Permanent", "永久删除"),
                            );
                        });
                });
                if self.plan_data.is_some() {
                    ui.label(RichText::new(&self.plan_details).color(Color32::LIGHT_RED));
                    ui.horizontal(|ui| {
                        ui.label(t("Confirmation", "确认"));
                        ui.text_edit_singleline(&mut self.confirm);
                        if ui.button(t("Apply and rescan", "执行并重新扫描")).clicked() {
                            self.apply_plan(ctx);
                        }
                    });
                }
            });
        self.review_open = open;
    }
    fn show_help(&mut self, ctx: &egui::Context) {
        if !self.help_open {
            return;
        }
        let mut open = self.help_open;
        egui::Window::new(t("Keys", "快捷键"))
            .open(&mut open)
            .show(ctx, |ui| {
                for (key, what) in key_help() {
                    ui.horizontal(|ui| {
                        ui.monospace(key);
                        ui.label(what);
                    });
                }
                ui.separator();
                ui.label(t("Tiles are coloured by name and hatched where a cleanup rule says the space could be had back. Both are hints, never authorization.", "方块按名称着色，可能回收的空间用斜纹标注。两者都只是线索，绝不构成删除授权。"));
            });
        self.help_open = open;
    }
    fn move_focus(&mut self, direction: Direction) {
        let focused = self.focus.filter(|id| {
            self.treemap_tiles
                .iter()
                .any(|tile| !tile.remainder && tile.id == *id)
        });
        let center = focused
            .and_then(|id| {
                self.treemap_tiles
                    .iter()
                    .find(|tile| !tile.remainder && tile.id == id)
            })
            .map(|tile| tile.rect.center());
        let Some(center) = center else {
            if let Some(tile) = self
                .treemap_tiles
                .iter()
                .filter(|tile| !tile.remainder)
                .max_by_key(|tile| tile.weight)
            {
                self.focus = Some(tile.id);
            }
            return;
        };
        let mut best: Option<(f32, u32)> = None;
        for tile in self
            .treemap_tiles
            .iter()
            .filter(|tile| !tile.remainder && Some(tile.id) != focused)
        {
            let to = tile.rect.center();
            let (along, across) = match direction {
                Direction::Left => (center.x - to.x, (to.y - center.y).abs()),
                Direction::Right => (to.x - center.x, (to.y - center.y).abs()),
                Direction::Up => (center.y - to.y, (to.x - center.x).abs()),
                Direction::Down => (to.y - center.y, (to.x - center.x).abs()),
            };
            if along <= 0.5 {
                continue;
            }
            let score = along + across * 3.0;
            if best.is_none_or(|(previous, _)| score < previous) {
                best = Some((score, tile.id));
            }
        }
        if let Some((_, id)) = best {
            self.focus = Some(id);
        }
    }
    fn cycle_sibling(&mut self, tree: &crate::tree::Snapshot, step: i32) {
        let siblings: Vec<u32> = if self
            .list_key
            .as_ref()
            .is_some_and(|key| key.0 == self.tree_current)
        {
            self.list_rows.clone()
        } else {
            tree.child_ids(self.tree_current).collect()
        };
        if siblings.is_empty() {
            return;
        }
        let position = self
            .focus
            .and_then(|id| siblings.iter().position(|sibling| *sibling == id));
        let last = siblings.len() as i32;
        let next = match position {
            Some(index) => (index as i32 + step.signum() + last) % last,
            None if step < 0 => last - 1,
            None => 0,
        };
        self.focus = Some(siblings[next as usize]);
    }
    fn handle_keys(&mut self, ctx: &egui::Context, tree: &crate::tree::Snapshot) {
        if ctx.memory(|memory| memory.focused().is_some()) {
            return;
        }
        let pressed = |key: egui::Key| ctx.input(|input| input.key_pressed(key));
        let shift = ctx.input(|input| input.modifiers.shift);
        let focus = self.focus;
        if pressed(egui::Key::Space)
            && let Some(id) = focus
        {
            self.toggle_mark(tree, id);
        }
        if pressed(egui::Key::X)
            && let Some(id) = focus
        {
            self.toggle_mark(tree, id);
            if let Some(parent) = parent_id(tree, id) {
                self.open_node(tree, parent);
            }
        }
        if pressed(egui::Key::Enter)
            && let Some(id) = focus
            && tree.nodes.get(id as usize).is_some_and(|node| node.is_dir)
        {
            self.open_node(tree, id);
        }
        for (key, direction) in [
            (egui::Key::ArrowLeft, Direction::Left),
            (egui::Key::ArrowRight, Direction::Right),
            (egui::Key::ArrowUp, Direction::Up),
            (egui::Key::ArrowDown, Direction::Down),
        ] {
            if pressed(key) {
                self.move_focus(direction);
            }
        }
        if pressed(egui::Key::Tab) {
            self.cycle_sibling(tree, if shift { -1 } else { 1 });
        }
        if pressed(egui::Key::Backspace) {
            self.goto_parent(ctx);
        }
        if pressed(egui::Key::Escape) {
            if self.review_open {
                self.review_open = false;
            } else if self.help_open {
                self.help_open = false;
            } else {
                self.goto_parent(ctx);
            }
        }
        if pressed(egui::Key::G) {
            self.open_node(tree, 0);
        }
        if pressed(egui::Key::T) {
            self.metric = self.metric.next();
        }
        if pressed(egui::Key::I) {
            self.hide_dot = !self.hide_dot;
        }
        if pressed(egui::Key::OpenBracket) {
            self.depth = self.depth.saturating_sub(1).max(1);
        }
        if pressed(egui::Key::CloseBracket) {
            self.depth = (self.depth + 1).min(4);
        }
        if pressed(egui::Key::C) {
            self.review_open = !self.review_open;
        }
        if pressed(egui::Key::O)
            && let Some(id) = focus
            && tree.nodes.get(id as usize).is_some()
        {
            let path = tree.path(id);
            if let Err(error) = reveal(&path) {
                self.status = error;
            }
        }
        if pressed(egui::Key::R) && self.rx.is_none() {
            self.rescan(ctx);
        }
        if pressed(egui::Key::Slash) || pressed(egui::Key::H) {
            self.help_open = !self.help_open;
        }
        if pressed(egui::Key::Q) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl App {
    fn begin_duplicates(&mut self, tree: Arc<crate::tree::Snapshot>, ctx: &egui::Context) {
        let (cancelled, progress) = crate::tree::new_controls();
        self.duplicate_cancelled = cancelled.clone();
        self.duplicate_progress = progress.clone();
        self.duplicates = None;
        let (tx, rx) = mpsc::channel();
        self.duplicate_rx = Some(rx);
        let ctx = ctx.clone();
        let threads = self.threads;
        thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::duplicates::find(&tree, threads, &cancelled, &progress)
            }))
            .unwrap_or_else(|_| {
                Err(t(
                    "duplicate search failed unexpectedly",
                    "重复文件搜索意外失败",
                )
                .into())
            });
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
    fn show_duplicates(&mut self, ctx: &egui::Context) {
        if !self.duplicate_window_open {
            return;
        }
        let Some(report) = self.duplicates.take() else {
            return;
        };
        let mut open = self.duplicate_window_open;
        egui::Window::new(t("Content-verified duplicates", "内容校验一致的重复文件")).open(&mut open).resizable(true).default_size([700.0,500.0]).show(ctx,|ui| {
            ui.label(t("Select files explicitly, then create a cleanup plan. Hard links are aliases of the same file; actual reclaimed space may differ.", "请显式选择文件后再创建清理计划。硬链接是同一文件的别名，实际回收空间可能不同。"));
            if !report.complete { ui.colored_label(Color32::LIGHT_RED,crate::t_format!("Incomplete: {} read errors or incomplete tree", "不完整: {} 个读取错误或树不完整",report.error_count)); }
            egui::ScrollArea::vertical().show(ui,|ui| {
                for group in &report.groups {
                    ui.separator();ui.strong(crate::t_format!("{} bytes — {} independent files — SHA-256 {}", "{} 字节 — {} 个独立文件 — SHA-256 {}",group.size,group.objects.len(),group.sha256));
                    for object in &group.objects {
                        for (path,alias) in std::iter::once((&object.path,false)).chain(object.aliases.iter().map(|p|(p,true))) {
                            ui.horizontal(|ui| {
                                let mut selected=self.selected.contains(path);
                                if ui.checkbox(&mut selected,"").changed() { if selected {self.selected.push(path.clone());} else {self.selected.retain(|p|p!=path);}self.plan_data=None; }
                                ui.label(path.to_string_lossy());if alias {ui.label(t("(hard-link alias)", "（硬链接别名）"));}
                            });
                        }
                    }
                }
                if report.groups.is_empty() {ui.label(t("No content-verified duplicate files found.", "未找到内容校验一致的重复文件。"));}
                for error in &report.errors {ui.colored_label(Color32::LIGHT_RED,error);}
            });
        });
        self.duplicate_window_open = open;
        self.duplicates = Some(report);
    }
}

impl App {
    fn begin_recommendations(&mut self, tree: Arc<crate::tree::Snapshot>, ctx: &egui::Context) {
        if self.recommendation_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.recommendation_rx = Some(rx);
        self.recommendations = None;
        let ctx = ctx.clone();
        thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::recommend::analyze(&tree)
            }))
            .map_err(|_| {
                t("suggestion worker failed unexpectedly", "建议任务意外失败").to_string()
            });
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
    fn show_recommendations(&mut self, ctx: &egui::Context) {
        if !self.recommendation_window_open {
            return;
        }
        let Some(report) = self.recommendations.take() else {
            return;
        };
        let mut open = self.recommendation_window_open;
        #[cfg(test)]
        self.suggestion_rects.clear();
        egui::Window::new(t("Cleanup suggestions", "清理建议")).open(&mut open).resizable(true).default_size([760.0, 520.0]).show(ctx, |ui| {
            ui.label(t("Heuristics only. Review each path and risk; no files are selected automatically. Create a cleanup plan to verify and confirm.", "仅为启发式线索。请逐项核对路径与风险；不会自动选择任何文件。创建清理计划后校验并确认。"));
            if !report.complete { ui.colored_label(Color32::LIGHT_RED, t("Scan incomplete; suggestions may be missing or sizes inaccurate.", "扫描不完整；建议可能缺失或大小不准确。")); }
            if report.total_candidates > report.candidates.len() as u64 { ui.colored_label(Color32::LIGHT_RED, crate::t_format!("Showing {} of {} suggestions", "显示 {} / {} 条建议",report.candidates.len(),report.total_candidates)); }
            egui::ScrollArea::vertical().show(ui, |ui| {
                for c in &report.candidates {
                    ui.separator();
                    ui.horizontal(|ui| {
                        let mut selected = self.selected.contains(&c.path);
                        let response = ui.checkbox(&mut selected, "");
                        #[cfg(test)]
                        self.suggestion_rects.push((c.path.clone(), response.rect));
                        if response.changed() {
                            if selected { self.selected.push(c.path.clone()); } else { self.selected.retain(|p| p != &c.path); }
                            self.plan_data = None;
                        }
                        ui.strong(c.path.to_string_lossy());
                    });
                    ui.label(crate::t_format!("{} | {} logical bytes | {} attributed allocated bytes | {} files{}", "{} | {} 逻辑字节 | {} 归属分配字节 | {} 个文件{}",c.rule,c.logical,c.allocated,c.files,if c.complete {""} else {" | INCOMPLETE"}));
                    ui.label(c.reason);
                    ui.colored_label(Color32::YELLOW, c.risk);
                }
                if report.candidates.is_empty() { ui.label(t("No matching suggestions.", "没有匹配的清理建议。")); }
            });
        });
        self.recommendation_window_open = open;
        self.recommendations = Some(report);
    }
}

impl App {
    fn begin_search(&mut self, ctx: &egui::Context) {
        if self.search_rx.is_some() {
            return;
        }
        let Some(tree) = self.tree.as_ref().cloned() else {
            return;
        };
        let options = crate::search::Options {
            query: self.search_query.clone(),
            glob: self.search_glob,
            path_match: self.search_path_match,
            kind: self.search_kind,
            min_bytes: self.search_min_bytes,
            limit: usize::from(self.search_limit),
        };
        self.search_submitted = options.query.clone();
        self.search_report = None;
        let (tx, rx) = mpsc::channel();
        self.search_rx = Some(rx);
        let ctx = ctx.clone();
        thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::search::find(&tree, &options)
            }))
            .unwrap_or_else(|_| {
                Err(t("search worker failed unexpectedly", "搜索任务意外失败").into())
            });
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
    fn show_search(&mut self, ctx: &egui::Context) {
        if !self.search_window_open {
            return;
        }
        let mut open = self.search_window_open;
        let mut navigate = None;
        #[cfg(test)]
        self.search_hit_rects.clear();
        egui::Window::new(t("Search entire tree", "搜索整棵树")).open(&mut open).resizable(true).default_size([800.0,520.0]).show(ctx,|ui| {
            ui.horizontal(|ui| { ui.label(t("Query", "查询"));ui.text_edit_singleline(&mut self.search_query);ui.checkbox(&mut self.search_glob,t("Glob", "通配符"));ui.checkbox(&mut self.search_path_match,t("Full path", "完整路径")); });
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("search-kind").selected_text(format!("{:?}",self.search_kind)).show_ui(ui,|ui| {
                    ui.selectable_value(&mut self.search_kind,crate::search::Kind::All,t("All", "全部"));ui.selectable_value(&mut self.search_kind,crate::search::Kind::File,t("Files", "文件"));ui.selectable_value(&mut self.search_kind,crate::search::Kind::Directory,t("Directories", "目录"));
                });
                ui.label(t("Min logical bytes", "最小逻辑字节"));ui.add(egui::DragValue::new(&mut self.search_min_bytes));
                ui.label(t("Limit", "上限"));ui.add(egui::DragValue::new(&mut self.search_limit).range(1..=10000));
                if ui.add_enabled(self.search_rx.is_none(),egui::Button::new(t("Search", "搜索"))).clicked(){self.begin_search(ctx);}
                if self.search_rx.is_some(){ui.label(t("Searching...", "正在搜索…"));}
            });
            if let Some(report)=&self.search_report {
                ui.label(crate::t_format!("Query {:?}: {} matches, showing {} largest{}", "查询 {:?}: {} 个匹配，显示最大的 {} 项{}",self.search_submitted,report.total_matches,report.hits.len(),if report.complete {""} else {" (incomplete scan)"}));
                egui::ScrollArea::vertical().show_rows(ui,25.0,report.hits.len(),|ui,range| {
                    for i in range {let hit=&report.hits[i];ui.horizontal(|ui| {
                        let mut selected=self.selected.contains(&hit.path);
                        let response=ui.checkbox(&mut selected,"");
                        #[cfg(test)]
                        self.search_hit_rects.push((hit.path.clone(),response.rect));
                        if response.changed(){if selected {self.selected.push(hit.path.clone());}else{self.selected.retain(|p|p!=&hit.path);}self.plan_data=None;}
                        if ui.small_button(t("Open", "打开")).clicked(){navigate=Some(hit.node);}
                        ui.label(crate::t_format!("{} bytes", "{} 字节",hit.logical));ui.label(hit.path.to_string_lossy());if !hit.complete {ui.colored_label(Color32::LIGHT_RED,"incomplete");}
                    });}
                });
            }
            ui.label(t("Selection only; create and confirm a cleanup plan to act. Text matching can be lossy for non-Unicode names.", "仅选中，不会执行操作；创建并确认清理计划后才会生效。非 Unicode 名称的文本匹配可能有损。"));
        });
        self.search_window_open = open;
        if let Some(id) = navigate
            && let Some(tree) = self.tree.as_ref().cloned()
        {
            let selected = if tree.nodes[id as usize].is_dir {
                id
            } else {
                tree.nodes[id as usize].parent
            };
            self.tree_current = selected;
            let mut current = selected;
            loop {
                self.expanded.insert(current);
                if let Some(parent) = parent_id(&tree, current) {
                    current = parent;
                } else {
                    break;
                }
            }
            self.visible_dirty = true;
            self.set_tree_path(&tree.path(selected));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    #[test]
    fn renders_tree_and_treemap_from_demo_snapshot() {
        let root = std::env::temp_dir().join(format!(
            "filetree233-gui-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("nested/雪.txt"), b"demo").unwrap();
        let (stop, progress) = crate::tree::new_controls();
        let tree = crate::tree::scan(&root, 2, &stop, &progress).unwrap();
        let mut app = App {
            tree: Some(Arc::new(tree)),
            ..Default::default()
        };
        app.expanded.insert(0);
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 720.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| app.show_tree(ctx));
        assert!(!out.shapes.is_empty());
        assert!(app.tree.is_some());
        assert!(!app.treemap_tiles.is_empty());
        let file_tile = |app: &App| {
            app.treemap_tiles
                .iter()
                .find(|t| !t.remainder && !app.tree.as_ref().unwrap().nodes[t.id as usize].is_dir)
                .map(|t| (t.id, t.rect.center()))
                .unwrap()
        };
        let marked_id = file_tile(&app).0;
        // Panels can settle on the first pass, which moves the tiles; click where the
        // latest layout draws the tile rather than at the first frame's position.
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1100.0, 720.0),
                )),
                ..Default::default()
            },
            |ctx| app.show_tree(ctx),
        );
        let position = file_tile(&app).1;
        let expected = app.tree.as_ref().unwrap().path(marked_id);
        let input = |pressed| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 720.0),
            )),
            events: vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::default(),
                },
            ],
            ..Default::default()
        };
        let _ = ctx.run(input(true), |ctx| app.show_tree(ctx));
        let _ = ctx.run(input(false), |ctx| app.show_tree(ctx));
        assert!(
            app.marked.contains(&marked_id),
            "click at {position:?} should mark tile {marked_id} ({expected:?}); marks: {:?}",
            app.marked
        );
        assert_eq!(app.focus, Some(marked_id));
        let tree = app.tree.as_ref().unwrap();
        assert!(app.plan_inputs(tree).contains(&expected));
        assert!(app.plan_data.is_none());
        assert!(root.join("nested/雪.txt").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn marking_a_directory_covers_its_children() {
        let root = std::env::temp_dir().join(format!(
            "filetree233-gui-mark-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("nested/one.bin"), vec![7u8; 4096]).unwrap();
        let (stop, progress) = crate::tree::new_controls();
        let tree = Arc::new(crate::tree::scan(&root, 2, &stop, &progress).unwrap());
        let dir = tree
            .child_ids(0)
            .find(|id| tree.nodes[*id as usize].is_dir)
            .unwrap();
        let child = tree.child_ids(dir).next().unwrap();
        let mut app = App {
            tree: Some(Arc::clone(&tree)),
            ..Default::default()
        };
        app.toggle_mark(&tree, child);
        assert_eq!(
            app.marked_sizes(&tree).0,
            tree.nodes[child as usize].logical
        );
        app.toggle_mark(&tree, dir);
        assert_eq!(app.marked.len(), 1);
        assert!(app.marked.contains(&dir));
        assert_eq!(app.marked_sizes(&tree).0, tree.nodes[dir as usize].logical);
        assert_eq!(app.marked_ancestor(&tree, child), Some(dir));
        // Marking inside an already marked directory is refused: the plan could
        // not carry overlapping targets anyway.
        let before = app.status.clone();
        app.toggle_mark(&tree, child);
        assert_eq!(app.marked.len(), 1);
        assert_ne!(app.status, before);
        assert_eq!(
            app.plan_inputs(&tree),
            vec![tree.path(dir)],
            "only the outer mark becomes a target"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn space_key_marks_the_focused_tile() {
        let root = std::env::temp_dir().join(format!(
            "filetree233-gui-keys-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("nested")).unwrap();
        std::fs::write(root.join("nested/two.bin"), vec![9u8; 2048]).unwrap();
        let (stop, progress) = crate::tree::new_controls();
        let tree = Arc::new(crate::tree::scan(&root, 2, &stop, &progress).unwrap());
        let target = tree
            .child_ids(0)
            .find(|id| tree.nodes[*id as usize].is_dir)
            .unwrap();
        let mut app = App {
            tree: Some(Arc::clone(&tree)),
            focus: Some(target),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let frame = |events| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 720.0),
            )),
            events,
            ..Default::default()
        };
        let space = |pressed| egui::Event::Key {
            key: egui::Key::Space,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        };
        let _ = ctx.run(frame(vec![space(true)]), |ctx| app.show_tree(ctx));
        let _ = ctx.run(frame(vec![space(false)]), |ctx| app.show_tree(ctx));
        assert!(app.marked.contains(&target));
        assert!(!app.review_open);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn tile_weight_follows_the_metric_and_hidden_entries() {
        let mut node = crate::tree::Node::new(OsString::from("node_modules"), crate::tree::NONE);
        node.is_dir = true;
        node.logical = 500;
        node.allocated = 900;
        node.files = 3;
        assert_eq!(tile_weight(&node, Metric::Logical, false), 500);
        assert_eq!(tile_weight(&node, Metric::Allocated, false), 900);
        assert_eq!(tile_weight(&node, Metric::Files, false), 3);
        assert_eq!(tile_weight(&node, Metric::Logical, true), 500);
        let hidden = crate::tree::Node::new(OsString::from(".cache"), crate::tree::NONE);
        assert_eq!(tile_weight(&hidden, Metric::Logical, true), 0);
    }
    #[test]
    fn suggestion_dialog_requires_explicit_selection() {
        let root = std::env::temp_dir().join(format!(
            "filetree233-gui-suggest-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("Cargo.toml"), b"[package]").unwrap();
        std::fs::write(root.join("target/output"), b"demo").unwrap();
        let (stop, progress) = crate::tree::new_controls();
        let tree = crate::tree::scan(&root, 2, &stop, &progress).unwrap();
        let report = crate::recommend::analyze(&tree);
        assert_eq!(report.candidates.len(), 1);
        let expected = report.candidates[0].path.clone();
        let mut app = App {
            recommendations: Some(report),
            recommendation_window_open: true,
            plan_data: Some(vec![1]),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let size = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1100.0, 720.0),
        ));
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: size,
                ..Default::default()
            },
            |ctx| app.show_recommendations(ctx),
        );
        assert!(app.selected.is_empty());
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: size,
                ..Default::default()
            },
            |ctx| app.show_recommendations(ctx),
        );
        let position = app.suggestion_rects[0].1.center();
        for pressed in [true, false] {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: size,
                    events: vec![
                        egui::Event::PointerMoved(position),
                        egui::Event::PointerButton {
                            pos: position,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::default(),
                        },
                    ],
                    ..Default::default()
                },
                |ctx| app.show_recommendations(ctx),
            );
        }
        assert_eq!(app.selected, vec![expected]);
        assert!(app.plan_data.is_none());
        assert!(root.join("target/output").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn volume_picker_sets_scan_path_only_after_click() {
        let root = std::env::temp_dir();
        let volume = crate::volumes::Volume {
            mount: root.clone(),
            name: "demo".into(),
            filesystem: "test".into(),
            total: 1000,
            available: 200,
            removable: false,
            read_only: false,
        };
        let mut app = App {
            path: "original".into(),
            volumes: vec![volume],
            volumes_requested: true,
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let size = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1100.0, 720.0),
        ));
        let render = |ctx: &egui::Context, app: &mut App| {
            egui::CentralPanel::default().show(ctx, |ui| app.volume_picker(ui, ctx));
        };
        for _ in 0..2 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: size,
                    ..Default::default()
                },
                |ctx| render(ctx, &mut app),
            );
        }
        assert_eq!(app.path, "original");
        let button = app.volume_combo_rect.unwrap().center();
        for pressed in [true, false] {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: size,
                    events: vec![
                        egui::Event::PointerMoved(button),
                        egui::Event::PointerButton {
                            pos: button,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::default(),
                        },
                    ],
                    ..Default::default()
                },
                |ctx| render(ctx, &mut app),
            );
        }
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: size,
                ..Default::default()
            },
            |ctx| render(ctx, &mut app),
        );
        assert_eq!(app.volume_item_rects.len(), 1);
        let choice = app.volume_item_rects[0].1.center();
        for pressed in [true, false] {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: size,
                    events: vec![
                        egui::Event::PointerMoved(choice),
                        egui::Event::PointerButton {
                            pos: choice,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::default(),
                        },
                    ],
                    ..Default::default()
                },
                |ctx| render(ctx, &mut app),
            );
        }
        assert_eq!(app.path, root.to_str().unwrap());
        assert!(app.rx.is_none());
    }
    #[test]
    fn summary_recovers_lossy_paths_from_raw_units() {
        let valid = serde_json::json!({"path":"abc", "path_units":crate::tree::raw_units(std::ffi::OsStr::new("abc"))});
        assert!(is_lossless_display_path(&valid));
        let mut lossy = valid.clone();
        lossy["path_units"] = serde_json::json!([97, 98, 255]);
        assert!(!is_lossless_display_path(&lossy));
        assert!(!is_lossless_display_path(
            &serde_json::json!({"error":"unavailable"})
        ));
    }
    #[test]
    fn search_results_require_explicit_selection() {
        let root = std::env::temp_dir().join(format!(
            "filetree233-gui-search-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("demo.log"), b"demo").unwrap();
        let (stop, progress) = crate::tree::new_controls();
        let tree = crate::tree::scan(&root, 2, &stop, &progress).unwrap();
        let opts = crate::search::Options {
            query: ".log".into(),
            glob: false,
            path_match: false,
            kind: crate::search::Kind::File,
            min_bytes: 0,
            limit: 100,
        };
        let report = crate::search::find(&tree, &opts).unwrap();
        let expected = report.hits[0].path.clone();
        let mut app = App {
            tree: Some(Arc::new(tree)),
            search_report: Some(report),
            search_window_open: true,
            search_submitted: ".log".into(),
            plan_data: Some(vec![1]),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let size = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1100.0, 720.0),
        ));
        for _ in 0..2 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: size,
                    ..Default::default()
                },
                |ctx| app.show_search(ctx),
            );
        }
        assert!(app.selected.is_empty());
        let pos = app.search_hit_rects[0].1.center();
        for pressed in [true, false] {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: size,
                    events: vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: egui::Modifiers::default(),
                        },
                    ],
                    ..Default::default()
                },
                |ctx| app.show_search(ctx),
            );
        }
        assert_eq!(app.selected, vec![expected]);
        assert!(app.plan_data.is_none());
        assert!(root.join("demo.log").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn export_worker_uses_snapshot_and_does_not_overwrite() {
        let base = std::env::temp_dir().join(format!(
            "filetree233-gui-export-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = base.join("scan");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("file"), b"demo").unwrap();
        let (stop, progress) = crate::tree::new_controls();
        let tree = crate::tree::scan(&root, 2, &stop, &progress).unwrap();
        let output = base.join("snapshot.csv");
        let mut app = App {
            tree: Some(Arc::new(tree)),
            export_path: output.display().to_string(),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        app.begin_export(crate::tree::ExportFormat::Csv, &ctx);
        let result = app
            .export_rx
            .take()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
            .unwrap();
        assert_eq!(result["format"], "csv");
        let original = std::fs::read(&output).unwrap();
        app.begin_export(crate::tree::ExportFormat::Csv, &ctx);
        assert!(
            app.export_rx
                .take()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap()
                .is_err()
        );
        assert_eq!(std::fs::read(&output).unwrap(), original);
        assert!(root.join("file").exists());
        std::fs::remove_dir_all(base).unwrap();
    }
}
