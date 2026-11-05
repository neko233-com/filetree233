use crate::i18n::t;
use crate::{
    CleanupMode, ScanEngine, delete_checked, plan_description, read_checked_plan, save_plan_mode,
    scan_dispatch,
};
use eframe::egui::{
    self, Align, Color32, CornerRadius, Layout, Margin, Rect, RichText, Sense, Stroke, Vec2, pos2,
};
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

#[cfg(windows)]
fn release_owned_console() {
    use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleProcessList};
    // Explorer and the Start Menu give the console-subsystem exe a console of
    // its own; detaching closes that window or terminal tab. A console shared
    // with a terminal stays so CLI output works.
    unsafe {
        let mut processes = [0u32; 2];
        if GetConsoleProcessList(processes.as_mut_ptr(), processes.len() as u32) == 1 {
            FreeConsole();
        }
    }
}

pub fn run(path: Option<&std::path::Path>, threads: u8) -> Result<(), String> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1180.0, 760.0])
        .with_min_inner_size([860.0, 540.0]);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let start = path.map(|p| p.to_path_buf());
    eframe::run_native(
        "filetree233",
        options,
        Box::new(move |cc| {
            #[cfg(windows)]
            release_owned_console();
            crate::theme::apply(&cc.egui_ctx);
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
fn window_icon() -> Option<egui::IconData> {
    let image = image::load_from_memory(include_bytes!("../assets/icon.png"))
        .ok()?
        .into_rgba8();
    let (width, height) = image.dimensions();
    Some(egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}
/// Drop the verbatim `\\?\` prefix that Windows paths carry in the snapshot.
fn pretty_path(path: &Path) -> String {
    let text = path.display().to_string();
    if let Some(rest) = text.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{rest}")
    } else if let Some(rest) = text.strip_prefix("\\\\?\\") {
        rest.to_owned()
    } else {
        text
    }
}
/// A flat folder glyph, 12 by 10 points.
fn paint_folder(painter: &egui::Painter, rect: Rect, color: Color32) {
    let body = Rect::from_min_max(
        pos2(rect.min.x, rect.min.y + 2.0),
        pos2(rect.max.x, rect.max.y),
    );
    let tab = Rect::from_min_max(
        rect.min,
        pos2(rect.min.x + rect.width() * 0.45, rect.min.y + 3.0),
    );
    painter.rect_filled(tab, CornerRadius::same(1), color);
    painter.rect_filled(body, CornerRadius::same(2), color);
}
/// A page glyph with a folded corner, 10 by 12 points.
fn paint_file(painter: &egui::Painter, rect: Rect, color: Color32) {
    let page = Rect::from_min_max(pos2(rect.min.x, rect.min.y), pos2(rect.max.x, rect.max.y));
    painter.rect_filled(page, CornerRadius::same(2), color);
    painter.rect_filled(
        Rect::from_min_max(
            pos2(page.max.x - 3.5, page.min.y),
            pos2(page.max.x, page.min.y + 3.5),
        ),
        CornerRadius::same(1),
        Color32::from_black_alpha(70),
    );
}
/// A card with a quiet title, used by the inspector column.
fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    crate::theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(title).size(11.5).strong().color(MUTED));
        ui.add_space(4.0);
        body(ui);
    });
    ui.add_space(8.0);
}
fn paint_logo(painter: &egui::Painter, rect: Rect) {
    painter.rect_filled(
        rect,
        CornerRadius::same((rect.width() * 0.28) as u8),
        ACCENT,
    );
    let inner = rect.shrink(rect.width() * 0.24);
    let gap = inner.width() * 0.13;
    let width = (inner.width() - gap * 2.0) / 3.0;
    for (index, factor) in [0.5_f32, 1.0, 0.72].into_iter().enumerate() {
        let height = inner.height() * factor;
        painter.rect_filled(
            Rect::from_min_size(
                pos2(
                    inner.left() + index as f32 * (width + gap),
                    inner.bottom() - height,
                ),
                Vec2::new(width, height),
            ),
            CornerRadius::same(2),
            ACCENT_INK.gamma_multiply(0.9),
        );
    }
}
fn volume_card(ui: &mut egui::Ui, volume: &crate::volumes::Volume, width: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 108.0), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let hovered = response.hovered();
    let painter = ui.painter();
    painter.rect_filled(
        rect,
        CornerRadius::same(12),
        if hovered { SURFACE_HOVER } else { SURFACE },
    );
    painter.rect_stroke(
        rect,
        CornerRadius::same(12),
        Stroke::new(
            1.0_f32,
            if hovered {
                ACCENT.gamma_multiply(0.75)
            } else {
                STROKE
            },
        ),
        egui::StrokeKind::Inside,
    );
    let mount = volume.mount.display().to_string();
    let mut detail = volume.name.clone();
    if detail.is_empty() || detail == mount {
        detail = volume.filesystem.clone();
    } else if !volume.filesystem.is_empty() {
        detail = format!("{} · {}", detail, volume.filesystem);
    }
    if volume.read_only {
        detail.push_str(t(" · read-only", " · 只读"));
    } else if volume.removable {
        detail.push_str(t(" · removable", " · 可移动"));
    }
    painter.text(
        pos2(rect.left() + 16.0, rect.top() + 13.0),
        egui::Align2::LEFT_TOP,
        &mount,
        egui::FontId::proportional(19.0),
        crate::theme::TEXT,
    );
    let used = volume.total.saturating_sub(volume.available);
    let fraction = if volume.total == 0 {
        0.0
    } else {
        used as f32 / volume.total as f32
    };
    let fill = if volume.total == 0 {
        crate::theme::FAINT
    } else if fraction >= 0.88 {
        DANGER
    } else if fraction >= 0.7 {
        AMBER
    } else {
        ACCENT
    };
    let percent = if volume.total == 0 {
        t("capacity unknown", "容量未知").to_owned()
    } else {
        crate::t_format!("{:.0}% used", "{:.0}% 已用", fraction * 100.0)
    };
    painter.text(
        pos2(rect.right() - 16.0, rect.top() + 16.0),
        egui::Align2::RIGHT_TOP,
        percent,
        egui::FontId::proportional(12.0),
        fill,
    );
    painter.text(
        pos2(rect.left() + 16.0, rect.top() + 42.0),
        egui::Align2::LEFT_TOP,
        detail,
        egui::FontId::proportional(11.0),
        crate::theme::FAINT,
    );
    let bar = Rect::from_min_size(
        pos2(rect.left() + 16.0, rect.top() + 68.0),
        Vec2::new((rect.width() - 32.0).max(0.0), 6.0),
    );
    painter.rect_filled(bar, CornerRadius::same(3), crate::theme::BG);
    if fraction > 0.0 {
        painter.rect_filled(
            Rect::from_min_size(
                bar.min,
                Vec2::new(bar.width() * fraction.clamp(0.0, 1.0), bar.height()),
            ),
            CornerRadius::same(3),
            fill,
        );
    }
    let footer = if volume.total == 0 {
        t("Capacity was not reported", "未报告容量").to_owned()
    } else {
        crate::t_format!(
            "{} free of {}",
            "{} 可用 / 共 {}",
            format_bytes(volume.available),
            format_bytes(volume.total)
        )
    };
    painter.text(
        pos2(rect.left() + 16.0, rect.bottom() - 13.0),
        egui::Align2::LEFT_BOTTOM,
        footer,
        egui::FontId::proportional(10.5),
        MUTED,
    );
    painter.text(
        pos2(rect.right() - 16.0, rect.bottom() - 13.0),
        egui::Align2::RIGHT_BOTTOM,
        t("Scan →", "扫描 →"),
        egui::FontId::proportional(11.0),
        if hovered { ACCENT_STRONG } else { MUTED },
    );
    response.on_hover_text(t("Scan this volume", "扫描此卷"))
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
const AMBER: Color32 = crate::theme::AMBER;
const DANGER: Color32 = crate::theme::DANGER;
const HATCH: Color32 = crate::theme::HATCH;
const ACCENT: Color32 = crate::theme::ACCENT;
const ACCENT_STRONG: Color32 = crate::theme::ACCENT_STRONG;
const ACCENT_INK: Color32 = crate::theme::ACCENT_INK;
const MUTED: Color32 = crate::theme::MUTED;
const FAINT: Color32 = crate::theme::FAINT;
const BG: Color32 = crate::theme::BG;
const SURFACE: Color32 = crate::theme::SURFACE;
const SURFACE_HOVER: Color32 = crate::theme::SURFACE_HOVER;
const STROKE: Color32 = crate::theme::STROKE;
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
            depth: 3,
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
        let selected = self.selected_volume_label();
        let response = crate::theme::dropdown_button(
            ui,
            RichText::new(selected).size(13.0).color(crate::theme::TEXT),
            220.0,
        );
        #[cfg(test)]
        {
            self.volume_combo_rect = Some(response.rect);
        }
        let volumes = self.volumes.clone();
        let mut chosen = None;
        let mut refresh = false;
        egui::Popup::menu(&response).width(380.0).show(|ui| {
            ui.set_width(350.0);
            if volumes.is_empty() {
                ui.label(
                    RichText::new(t("No volumes detected", "未检测到卷"))
                        .size(12.0)
                        .color(FAINT),
                );
            }
            for volume in &volumes {
                let total = if volume.total == 0 {
                    t("unknown", "未知").into()
                } else {
                    format_bytes(volume.total)
                };
                let available = if volume.total == 0 {
                    t("unknown", "未知").into()
                } else {
                    format_bytes(volume.available)
                };
                let (row, item) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 46.0), Sense::click());
                if item.hovered() {
                    ui.painter()
                        .rect_filled(row.shrink(1.0), CornerRadius::same(8), SURFACE_HOVER);
                }
                ui.painter().text(
                    pos2(row.left() + 10.0, row.top() + 9.0),
                    egui::Align2::LEFT_TOP,
                    format!("{}   {}", volume.mount.display(), volume.name),
                    egui::FontId::proportional(12.5),
                    crate::theme::TEXT,
                );
                ui.painter().text(
                    pos2(row.left() + 10.0, row.bottom() - 9.0),
                    egui::Align2::LEFT_BOTTOM,
                    crate::t_format!(
                        "{} free of {} · {}",
                        "{} 可用 / 共 {} · {}",
                        available,
                        total,
                        volume.filesystem
                    ),
                    egui::FontId::proportional(11.0),
                    MUTED,
                );
                #[cfg(test)]
                self.volume_item_rects
                    .push((volume.mount.clone(), item.rect));
                if item.clicked() {
                    chosen = Some(volume.mount.clone());
                    ui.close();
                }
            }
            ui.add_space(4.0);
            ui.separator();
            if ui
                .add_sized(
                    [ui.available_width(), 28.0],
                    egui::Button::new(t("Refresh volumes", "刷新卷")),
                )
                .clicked()
            {
                refresh = true;
                ui.close();
            }
        });
        if let Some(mount) = chosen
            && let Some(path) = mount.to_str()
        {
            self.path = path.to_owned();
            self.status = crate::t_format!(
                "Selected {}. Press Scan to start.",
                "已选择 {}，点击“扫描”开始。",
                mount.display()
            );
        }
        if refresh {
            self.begin_volumes(ctx);
        }
        if !self.volume_status.is_empty() && self.volumes.is_empty() {
            ui.label(RichText::new(&self.volume_status).size(11.0).color(FAINT));
        }
    }
    fn selected_volume_label(&self) -> String {
        let path = Path::new(&self.path);
        self.volumes
            .iter()
            .filter(|volume| path.starts_with(&volume.mount))
            .max_by_key(|volume| volume.mount.as_os_str().len())
            .map(|volume| format!("{}   {}", volume.mount.display(), volume.name))
            .unwrap_or_else(|| t("Choose volume", "选择卷").into())
    }
    fn set_tree_path(&mut self, path: &std::path::Path) {
        if path.to_str().is_some() {
            self.path = pretty_path(path);
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
                    let root_representable = if tree.root.to_str().is_some() {
                        self.path = pretty_path(&tree.root);
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
        egui::TopBottomPanel::top("top")
            .frame(
                egui::Frame::new()
                    .fill(crate::theme::PANEL)
                    .inner_margin(Margin::symmetric(18, 12)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let (logo, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
                    paint_logo(ui.painter(), logo);
                    ui.add_space(2.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.label(
                            RichText::new("filetree233")
                                .size(17.0)
                                .strong()
                                .color(crate::theme::TEXT),
                        );
                        ui.label(
                            RichText::new(t("disk space explorer", "磁盘空间透视"))
                                .size(11.0)
                                .color(FAINT),
                        );
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let next_language = match crate::i18n::current() {
                            crate::i18n::Lang::Zh => crate::i18n::Lang::En,
                            crate::i18n::Lang::En => crate::i18n::Lang::Zh,
                        };
                        let next_label = match next_language {
                            crate::i18n::Lang::Zh => "中文",
                            crate::i18n::Lang::En => "English",
                        };
                        if ui
                            .add_sized(
                                [72.0, 28.0],
                                egui::Button::new(RichText::new(next_label).size(12.0))
                                    .fill(crate::theme::SURFACE)
                                    .stroke(Stroke::new(1.0_f32, STROKE))
                                    .corner_radius(CornerRadius::same(9)),
                            )
                            .on_hover_text(t("Switch interface language", "切换界面语言"))
                            .clicked()
                        {
                            crate::i18n::set(next_language);
                        }
                        if ui
                            .add_sized(
                                [32.0, 28.0],
                                egui::Button::new(RichText::new("?").size(13.0))
                                    .fill(crate::theme::SURFACE)
                                    .stroke(Stroke::new(1.0_f32, STROKE))
                                    .corner_radius(CornerRadius::same(9)),
                            )
                            .on_hover_text(t("Keys and help", "快捷键与帮助"))
                            .clicked()
                        {
                            self.help_open = true;
                        }
                    });
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    self.volume_picker(ui, ctx);
                    let cancel_space = if self.rx.is_some() && self.scan_is_tree {
                        84.0
                    } else {
                        8.0
                    };
                    let path_width =
                        (ui.available_width() - 92.0 - 64.0 - cancel_space - 16.0).max(200.0);
                    ui.add_sized(
                        [path_width, 30.0],
                        egui::TextEdit::singleline(&mut self.path)
                            .vertical_align(Align::Center)
                            .margin(Margin::symmetric(10, 6))
                            .hint_text(t("Type or paste a folder", "输入或粘贴文件夹路径")),
                    );
                    let scan = egui::Button::new(
                        RichText::new(t("Scan", "扫描"))
                            .strong()
                            .color(crate::theme::ACCENT_INK),
                    )
                    .fill(ACCENT)
                    .corner_radius(CornerRadius::same(9))
                    .min_size(Vec2::new(88.0, 30.0));
                    if ui
                        .add(scan)
                        .on_hover_text(t("Scan the folder above", "扫描上方目录"))
                        .clicked()
                    {
                        self.begin_scan(ctx);
                    }
                    if ui
                        .add_sized(
                            [60.0, 30.0],
                            egui::Button::new(t("Up", "上级"))
                                .fill(crate::theme::SURFACE)
                                .stroke(Stroke::new(1.0_f32, STROKE))
                                .corner_radius(CornerRadius::same(9)),
                        )
                        .on_hover_text(t("Go to the parent folder", "返回上级目录"))
                        .clicked()
                    {
                        self.goto_parent(ctx);
                    }
                    if self.rx.is_some()
                        && self.scan_is_tree
                        && ui
                            .add_sized(
                                [76.0, 30.0],
                                egui::Button::new(RichText::new(t("Cancel", "取消")).color(DANGER))
                                    .fill(crate::theme::SURFACE)
                                    .stroke(Stroke::new(1.0_f32, STROKE))
                                    .corner_radius(CornerRadius::same(9)),
                            )
                            .clicked()
                    {
                        self.cancelled.store(true, Ordering::Relaxed);
                    }
                });
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(t("Engine", "引擎")).size(11.5).color(FAINT));
                    let engine_label = match self.engine {
                        ScanEngine::Walk => t("Walk", "遍历"),
                        ScanEngine::Mft => t("MFT (experimental)", "MFT（实验性）"),
                        ScanEngine::Auto => t("Auto (experimental)", "自动（实验性）"),
                    };
                    let engine = crate::theme::dropdown_button(
                        ui,
                        RichText::new(engine_label)
                            .size(12.0)
                            .color(crate::theme::TEXT),
                        164.0,
                    );
                    egui::Popup::menu(&engine).width(210.0).show(|ui| {
                        for (value, label) in [
                            (ScanEngine::Walk, t("Walk", "遍历")),
                            (ScanEngine::Mft, t("MFT (experimental)", "MFT（实验性）")),
                            (ScanEngine::Auto, t("Auto (experimental)", "自动（实验性）")),
                        ] {
                            if ui.selectable_label(self.engine == value, label).clicked() {
                                self.engine = value;
                                ui.close();
                            }
                        }
                    });
                    if crate::theme::toggle_chip(ui, self.full_tree, t("Full tree", "完整树"))
                        .on_hover_text(t(
                            "Build the full tree used by the treemap",
                            "构建矩形树所需的完整目录树",
                        ))
                        .clicked()
                    {
                        self.full_tree = !self.full_tree;
                    }
                    ui.add_space(4.0);
                    ui.label(RichText::new(t("Workers", "线程")).size(11.5).color(FAINT));
                    ui.add(egui::DragValue::new(&mut self.threads).range(1..=32));
                    ui.add_space(4.0);
                    ui.label(RichText::new(t("Filter", "筛选")).size(11.5).color(FAINT));
                    ui.add_sized(
                        [190.0, 28.0],
                        egui::TextEdit::singleline(&mut self.filter)
                            .vertical_align(Align::Center)
                            .margin(Margin::symmetric(9, 5))
                            .hint_text(t("name contains", "名称包含")),
                    );
                    if self.rx.is_some() && self.scan_is_tree {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.add(egui::Spinner::new().size(14.0));
                            crate::theme::badge(
                                ui,
                                crate::t_format!(
                                    "{} entries",
                                    "{} 个条目",
                                    self.progress.load(Ordering::Relaxed)
                                ),
                                ACCENT,
                            );
                            ctx.request_repaint_after(std::time::Duration::from_millis(250));
                        });
                    }
                });
            });
        let has_results =
            self.tree.is_some() || !self.entries.is_empty() || self.plan_data.is_some();
        egui::TopBottomPanel::bottom("bottom")
            .frame(
                egui::Frame::new()
                    .fill(crate::theme::PANEL)
                    .inner_margin(Margin::symmetric(18, 8)),
            )
            .show(ctx, |ui| {
                if has_results {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(t("Export", "导出")).size(11.5).color(FAINT));
                        ui.add_sized(
                            [300.0, 28.0],
                            egui::TextEdit::singleline(&mut self.export_path)
                                .vertical_align(Align::Center)
                                .margin(Margin::symmetric(9, 5))
                                .hint_text(t("CSV or JSONL output file", "CSV 或 JSONL 输出文件")),
                        );
                        let enabled = self.tree.is_some() && self.export_rx.is_none();
                        if ui
                            .add_enabled(
                                enabled,
                                egui::Button::new(RichText::new("CSV").size(12.0))
                                    .fill(crate::theme::SURFACE)
                                    .stroke(Stroke::new(1.0_f32, STROKE))
                                    .corner_radius(CornerRadius::same(9)),
                            )
                            .on_hover_text(t("Export the snapshot as CSV", "将快照导出为 CSV"))
                            .clicked()
                        {
                            self.begin_export(crate::tree::ExportFormat::Csv, ctx);
                        }
                        if ui
                            .add_enabled(
                                enabled,
                                egui::Button::new(RichText::new("JSONL").size(12.0))
                                    .fill(crate::theme::SURFACE)
                                    .stroke(Stroke::new(1.0_f32, STROKE))
                                    .corner_radius(CornerRadius::same(9)),
                            )
                            .on_hover_text(t(
                                "Export the snapshot as JSON lines",
                                "将快照导出为 JSON 行",
                            ))
                            .clicked()
                        {
                            self.begin_export(crate::tree::ExportFormat::Jsonl, ctx);
                        }
                        if self.export_rx.is_some() {
                            ui.label(
                                RichText::new(t("Exporting...", "正在导出…"))
                                    .size(11.5)
                                    .color(AMBER),
                            );
                        }
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(t("Plan file", "计划文件"))
                                .size(11.5)
                                .color(FAINT),
                        );
                        ui.add_sized(
                            [300.0, 28.0],
                            egui::TextEdit::singleline(&mut self.plan_path)
                                .vertical_align(Align::Center)
                                .margin(Margin::symmetric(9, 5))
                                .hint_text(t("Cleanup plan path", "清理计划路径")),
                        );
                        let mode_label = match self.cleanup_mode {
                            CleanupMode::Recycle => t("Recycle bin", "回收站"),
                            CleanupMode::Permanent => t("Permanent", "永久删除"),
                        };
                        let mode = crate::theme::dropdown_button(
                            ui,
                            RichText::new(mode_label)
                                .size(12.0)
                                .color(crate::theme::TEXT),
                            126.0,
                        );
                        egui::Popup::menu(&mode).width(150.0).show(|ui| {
                            for (value, label) in [
                                (CleanupMode::Recycle, t("Recycle bin", "回收站")),
                                (CleanupMode::Permanent, t("Permanent", "永久删除")),
                            ] {
                                if ui
                                    .selectable_label(self.cleanup_mode == value, label)
                                    .clicked()
                                {
                                    self.cleanup_mode = value;
                                    ui.close();
                                }
                            }
                        });
                        let create = egui::Button::new(
                            RichText::new(t("Create cleanup plan", "创建清理计划"))
                                .strong()
                                .color(crate::theme::ACCENT_INK),
                        )
                        .fill(ACCENT)
                        .corner_radius(CornerRadius::same(9))
                        .min_size(Vec2::new(0.0, 30.0));
                        if ui
                            .add(create)
                            .on_hover_text(t(
                                "Writes a plan for the marked entries; nothing is deleted yet",
                                "为已标记条目写出计划；此时不会删除任何内容",
                            ))
                            .clicked()
                        {
                            self.create_plan();
                        }
                    });
                    if self.plan_data.is_some() && self.tree.is_none() {
                        ui.add_space(6.0);
                        egui::Frame::new()
                            .fill(SURFACE)
                            .stroke(Stroke::new(1.0_f32, DANGER.gamma_multiply(0.7)))
                            .corner_radius(CornerRadius::same(10))
                            .inner_margin(Margin::same(10))
                            .show(ui, |ui| {
                                ui.label(RichText::new(&self.plan_details).color(DANGER));
                                ui.horizontal(|ui| {
                                    ui.label(t("Confirmation", "确认"));
                                    ui.text_edit_singleline(&mut self.confirm);
                                    let apply = egui::Button::new(
                                        RichText::new(t(
                                            "Apply confirmed plan",
                                            "执行已确认的计划",
                                        ))
                                        .strong()
                                        .color(crate::theme::ACCENT_INK),
                                    )
                                    .fill(ACCENT)
                                    .corner_radius(CornerRadius::same(9));
                                    if ui.add(apply).clicked() {
                                        self.apply_plan(ctx);
                                    }
                                });
                            });
                    }
                    ui.add_space(6.0);
                }
                ui.horizontal(|ui| {
                    let dot = if self.rx.is_some() {
                        AMBER
                    } else if self.tree.is_some() || !self.entries.is_empty() {
                        ACCENT
                    } else {
                        FAINT
                    };
                    let (circle, _) = ui.allocate_exact_size(Vec2::splat(8.0), Sense::hover());
                    ui.painter().circle_filled(circle.center(), 3.0, dot);
                    let label_width = (ui.available_width() - 190.0).max(120.0);
                    ui.allocate_ui_with_layout(
                        Vec2::new(label_width, 16.0),
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.set_width(label_width);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&self.status).size(11.5).color(MUTED),
                                )
                                .truncate(),
                            );
                        },
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!("filetree233 {}", env!("CARGO_PKG_VERSION")))
                                .size(10.5)
                                .color(FAINT),
                        );
                    });
                });
            });
        self.show_help(ctx);
        if self.tree.is_some() {
            self.show_tree(ctx);
            self.show_duplicates(ctx);
            self.show_recommendations(ctx);
            self.show_search(ctx);
            return;
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(crate::theme::PANEL)
                    .inner_margin(Margin::symmetric(14, 12)),
            )
            .show(ctx, |ui| {
            if self.entries.is_empty() {
                self.show_welcome(ui, ctx);
                return;
            }
            ui.label(
                RichText::new(t(
                    "Logical bytes | allocated bytes* | path (*hard links attributed once across scan)",
                    "逻辑字节 | 分配字节* | 路径（*硬链接在整次扫描中只计一次）",
                ))
                .size(11.5)
                .color(FAINT),
            );
            let total: u64 = self
                .entries
                .iter()
                .filter_map(|e| e["bytes"].as_u64())
                .sum();
            let available = ui.available_width();
            let palette = [
                ACCENT,
                Color32::from_rgb(95, 143, 212),
                Color32::from_rgb(150, 121, 198),
                Color32::from_rgb(214, 152, 82),
                Color32::from_rgb(105, 172, 121),
                Color32::from_rgb(198, 108, 132),
            ];
            ui.add_space(4.0);
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
                        ui.allocate_exact_size(Vec2::new(width, 46.0), Sense::hover());
                    ui.painter()
                        .rect_filled(rect.shrink(1.0), CornerRadius::same(4), palette[i % palette.len()]);
                    response.on_hover_text(crate::t_format!(
                        "{}: {} bytes",
                        "{}: {} 字节",
                        entry["path"].as_str().unwrap_or("?"),
                        n
                    ));
                }
            });
            ui.add_space(8.0);
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
                            ui.label(
                                RichText::new(format!(
                                    "{n:>14}  {:>14}",
                                    entry["allocated_attributed_bytes"].as_u64().unwrap_or(0)
                                ))
                                .monospace()
                                .size(12.0)
                                .color(MUTED),
                            );
                        } else {
                            ui.colored_label(DANGER, t("ERROR", "错误"));
                        }
                        let shown = pretty_path(Path::new(path));
                        if is_dir && lossless {
                            if ui
                                .link(RichText::new(shown).color(ACCENT_STRONG))
                                .clicked()
                            {
                                navigate = Some(path.to_string());
                            }
                        } else {
                            ui.label(shown);
                            if !lossless {
                                ui.colored_label(
                                    AMBER,
                                    t(
                                        "raw path; see exact code units in plan",
                                        "原始路径；精确码元见计划文件",
                                    ),
                                );
                            }
                        }
                        if entry["complete"] == false {
                            ui.colored_label(
                                DANGER,
                                crate::t_format!(
                                    "Incomplete: {}",
                                    "不完整: {}",
                                    entry["error_count"].as_u64().unwrap_or(1)
                                ),
                            );
                        }
                        if let Some(error) = entry["error"].as_str() {
                            ui.colored_label(DANGER, error);
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
    fn show_welcome(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::ScrollArea::vertical()
            .id_salt("welcome")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let available = ui.available_width();
                let content = available.min(1040.0);
                let pad = ((available - content) * 0.5).max(0.0);
                let mut scan_now: Option<PathBuf> = None;
                let mut refresh = false;
                ui.horizontal(|ui| {
                    ui.add_space(pad);
                    ui.vertical(|ui| {
                        ui.set_width(content);
                        ui.vertical_centered(|ui| {
                            ui.add_space(14.0);
                            let (logo, _) =
                                ui.allocate_exact_size(Vec2::splat(54.0), Sense::hover());
                            paint_logo(ui.painter(), logo);
                            ui.add_space(10.0);
                            ui.label(
                                RichText::new(t(
                                    "See where your disk space went.",
                                    "看清磁盘空间到底去了哪里。",
                                ))
                                .size(25.0)
                                .strong()
                                .color(crate::theme::TEXT),
                            );
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new(t(
                                    "Pick a volume below or type any folder above, then press Scan.",
                                    "在下方选择磁盘卷，或在上方输入任意文件夹，然后点击“扫描”。",
                                ))
                                .size(13.0)
                                .color(MUTED),
                            );
                            ui.add_space(3.0);
                            ui.label(
                                RichText::new(t(
                                    "Marking is only a note to yourself - deleting always needs a written plan and its confirmation code.",
                                    "标记只是记下来 —— 删除始终需要一份书面计划与确认码。",
                                ))
                                .size(11.0)
                                .color(FAINT),
                            );
                        });
                        ui.add_space(16.0);
                        if self.rx.is_some() {
                            ui.vertical_centered(|ui| {
                                ui.add(egui::Spinner::new().size(20.0).color(ACCENT));
                                ui.add_space(10.0);
                                ui.label(
                                    RichText::new(crate::t_format!(
                                        "Scanning {} - {} entries so far",
                                        "正在扫描 {} —— 已有 {} 个条目",
                                        pretty_path(&self.scan_root),
                                        self.progress.load(Ordering::Relaxed)
                                    ))
                                    .size(12.5)
                                    .color(MUTED),
                                );
                                ui.add_space(10.0);
                                if crate::theme::ghost_button(
                                    ui,
                                    RichText::new(t("Cancel scan", "取消扫描")).color(DANGER),
                                )
                                .clicked()
                                {
                                    self.cancelled.store(true, Ordering::Relaxed);
                                }
                            });
                        } else {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(t("Volumes", "磁盘卷"))
                                        .size(13.5)
                                        .strong()
                                        .color(crate::theme::TEXT),
                                );
                                ui.with_layout(
                                    Layout::right_to_left(Align::Center),
                                    |ui| {
                                        if crate::theme::ghost_button(
                                            ui,
                                            RichText::new(t("Refresh", "刷新")).size(12.0),
                                        )
                                        .clicked()
                                        {
                                            refresh = true;
                                        }
                                    },
                                );
                            });
                            ui.add_space(8.0);
                            if self.volumes.is_empty() {
                                egui::Frame::new()
                                    .fill(SURFACE)
                                    .corner_radius(CornerRadius::same(12))
                                    .inner_margin(Margin::same(16))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        ui.horizontal(|ui| {
                                            ui.add(
                                                egui::Spinner::new().size(14.0).color(MUTED),
                                            );
                                            ui.label(
                                                RichText::new(if self.volume_status.is_empty() {
                                                    t(
                                                        "Looking for mounted volumes...",
                                                        "正在查找已挂载的卷…",
                                                    )
                                                    .to_owned()
                                                } else {
                                                    self.volume_status.clone()
                                                })
                                                .size(12.0)
                                                .color(MUTED),
                                            );
                                        });
                                    });
                            } else {
                                let gap = 12.0;
                                let columns = (((content + gap) / 292.0).floor() as usize)
                                    .clamp(1, 3);
                                let card_width = (content
                                    - gap * columns.saturating_sub(1) as f32)
                                    / columns as f32;
                                for row in self.volumes.chunks(columns) {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = gap;
                                        for volume in row {
                                            if volume_card(ui, volume, card_width).clicked() {
                                                scan_now = Some(volume.mount.clone());
                                            }
                                        }
                                    });
                                    ui.add_space(gap);
                                }
                            }
                            ui.add_space(2.0);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new(t(
                                        "Hard links are counted once. Scanning never changes anything on disk.",
                                        "硬链接只计一次。扫描不会改动磁盘上的任何内容。",
                                    ))
                                    .size(11.0)
                                    .color(FAINT),
                                );
                            });
                        }
                        ui.add_space(6.0);
                    });
                });
                if refresh {
                    self.begin_volumes(ctx);
                }
                if let Some(mount) = scan_now {
                    self.path = pretty_path(&mount);
                    self.begin_scan(ctx);
                }
            });
    }
    fn show_tree(&mut self, ctx: &egui::Context) {
        let tree = self.tree.take().expect("tree is present");
        let mut navigate = None;
        egui::SidePanel::left("tree-panel")
            .resizable(true)
            .default_width(290.0)
            .frame(
                egui::Frame::new()
                    .fill(crate::theme::PANEL)
                    .inner_margin(Margin::symmetric(10, 10)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(t("Directory tree", "目录树"))
                            .size(13.5)
                            .strong()
                            .color(crate::theme::TEXT),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let (logical, _) = self.marked_sizes(&tree);
                        if !self.marked.is_empty() {
                            crate::theme::badge(
                                ui,
                                crate::t_format!(
                                    "{} marked · {}",
                                    "已标记 {} · {}",
                                    self.marked.len(),
                                    format_bytes(logical)
                                ),
                                DANGER,
                            );
                        }
                        crate::theme::badge(
                            ui,
                            crate::t_format!("{} files", "{} 个文件", tree.nodes[0].files),
                            ACCENT,
                        );
                    });
                });
                ui.add_space(4.0);
                let full = ui.available_width();
                if ui
                    .add_sized(
                        [full, 28.0],
                        egui::Button::new(
                            RichText::new(t("Search entire tree", "搜索整棵树")).size(12.0),
                        )
                        .fill(SURFACE)
                        .stroke(Stroke::new(1.0_f32, STROKE))
                        .corner_radius(CornerRadius::same(9)),
                    )
                    .clicked()
                {
                    self.search_window_open = true;
                }

                ui.horizontal(|ui| {
                    let half = (ui.available_width() - 8.0) / 2.0;
                    if self.duplicate_rx.is_none() {
                        let width = if self.duplicates.is_some() {
                            half
                        } else {
                            ui.available_width()
                        };
                        if ui
                            .add_sized(
                                [width, 28.0],
                                egui::Button::new(
                                    RichText::new(t("Find duplicates", "查找重复文件")).size(12.0),
                                )
                                .fill(SURFACE)
                                .stroke(Stroke::new(1.0_f32, STROKE))
                                .corner_radius(CornerRadius::same(9)),
                            )
                            .clicked()
                        {
                            self.begin_duplicates(Arc::clone(&tree), ctx);
                        }
                        if self.duplicates.is_some()
                            && ui
                                .add_sized(
                                    [half, 28.0],
                                    egui::Button::new(
                                        RichText::new(t("Show duplicates", "显示重复文件"))
                                            .size(12.0),
                                    )
                                    .fill(SURFACE)
                                    .stroke(Stroke::new(1.0_f32, STROKE))
                                    .corner_radius(CornerRadius::same(9)),
                                )
                                .clicked()
                        {
                            self.duplicate_window_open = true;
                        }
                    } else {
                        crate::theme::badge(
                            ui,
                            crate::t_format!(
                                "Hashing: {}",
                                "哈希: {}",
                                self.duplicate_progress.load(Ordering::Relaxed)
                            ),
                            AMBER,
                        );
                        if ui
                            .add_sized(
                                [80.0, 24.0],
                                egui::Button::new(t("Cancel search", "取消搜索")),
                            )
                            .clicked()
                        {
                            self.duplicate_cancelled.store(true, Ordering::Relaxed);
                        }
                    }
                });
                ui.horizontal(|ui| {
                    let half = (ui.available_width() - 8.0) / 2.0;
                    if self.recommendation_rx.is_none() {
                        let width = if self.recommendations.is_some() {
                            half
                        } else {
                            ui.available_width()
                        };
                        if ui
                            .add_sized(
                                [width, 28.0],
                                egui::Button::new(
                                    RichText::new(t("Suggest cleanup", "清理建议")).size(12.0),
                                )
                                .fill(SURFACE)
                                .stroke(Stroke::new(1.0_f32, STROKE))
                                .corner_radius(CornerRadius::same(9)),
                            )
                            .clicked()
                        {
                            self.begin_recommendations(Arc::clone(&tree), ctx);
                        }
                        if self.recommendations.is_some()
                            && ui
                                .add_sized(
                                    [half, 28.0],
                                    egui::Button::new(
                                        RichText::new(t("Show suggestions", "显示建议")).size(12.0),
                                    )
                                    .fill(SURFACE)
                                    .stroke(Stroke::new(1.0_f32, STROKE))
                                    .corner_radius(CornerRadius::same(9)),
                                )
                                .clicked()
                        {
                            self.recommendation_window_open = true;
                        }
                    } else {
                        ui.label(
                            RichText::new(t("Analyzing suggestions...", "正在分析清理建议…"))
                                .color(AMBER)
                                .size(12.0),
                        );
                    }
                });
                ui.add_space(4.0);
                ui.separator();
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
                egui::ScrollArea::vertical().id_salt("tree-rows").show_rows(
                    ui,
                    26.0,
                    rows.len(),
                    |ui, range| {
                        for index in range {
                            let (id, depth) = rows[index];
                            let node = &tree.nodes[id as usize];
                            let row = Rect::from_min_size(
                                ui.cursor().min,
                                Vec2::new(ui.available_width(), 26.0),
                            );
                            let response = ui.allocate_rect(row, Sense::click());
                            let selected = id == self.tree_current;
                            let hovered = response.hovered();
                            let painter =
                                ui.painter().with_clip_rect(ui.clip_rect().intersect(row));
                            if selected {
                                painter.rect_filled(
                                    row.shrink2(Vec2::new(1.0, 1.0)),
                                    CornerRadius::same(6),
                                    ACCENT.gamma_multiply(0.16),
                                );
                            } else if hovered {
                                painter.rect_filled(
                                    row.shrink2(Vec2::new(1.0, 1.0)),
                                    CornerRadius::same(6),
                                    SURFACE,
                                );
                            }
                            let mut x = row.min.x + 8.0 + (depth.min(20) as f32) * 14.0;
                            let mut toggle = false;
                            if node.is_dir {
                                let tri = Rect::from_center_size(
                                    pos2(x + 5.0, row.center().y),
                                    Vec2::splat(14.0),
                                );
                                let center = tri.center();
                                let (points, color) = if self.expanded.contains(&id) {
                                    (
                                        vec![
                                            pos2(center.x - 4.0, center.y - 2.5),
                                            pos2(center.x + 4.0, center.y - 2.5),
                                            pos2(center.x, center.y + 2.5),
                                        ],
                                        if hovered { crate::theme::TEXT } else { MUTED },
                                    )
                                } else {
                                    (
                                        vec![
                                            pos2(center.x - 2.5, center.y - 4.0),
                                            pos2(center.x - 2.5, center.y + 4.0),
                                            pos2(center.x + 2.5, center.y),
                                        ],
                                        if hovered { crate::theme::TEXT } else { MUTED },
                                    )
                                };
                                painter.add(egui::Shape::convex_polygon(
                                    points,
                                    color,
                                    Stroke::NONE,
                                ));
                                toggle = response.clicked()
                                    && response
                                        .interact_pointer_pos()
                                        .is_some_and(|p| tri.expand(2.0).contains(p));
                            }
                            x += 16.0;
                            let icon = Rect::from_center_size(
                                pos2(x + 6.0, row.center().y),
                                Vec2::new(12.0, 11.0),
                            );
                            if node.is_dir {
                                paint_folder(
                                    &painter,
                                    icon,
                                    if selected {
                                        ACCENT_STRONG
                                    } else {
                                        ACCENT.gamma_multiply(0.85)
                                    },
                                );
                            } else {
                                paint_file(&painter, icon, FAINT);
                            }
                            x += 18.0;
                            let label = if id == 0 {
                                pretty_path(&tree.root)
                            } else {
                                node.name.to_string_lossy().into_owned()
                            };
                            let mut label_job = egui::text::LayoutJob::single_section(
                                label.clone(),
                                egui::TextFormat {
                                    font_id: egui::FontId::proportional(13.0),
                                    color: if selected || node.is_dir {
                                        crate::theme::TEXT
                                    } else {
                                        MUTED
                                    },
                                    ..Default::default()
                                },
                            );
                            label_job.wrap = egui::text::TextWrapping::truncate_at_width(
                                (row.max.x - 58.0 - x).max(0.0),
                            );
                            let label_galley = painter.layout_job(label_job);
                            painter
                                .with_clip_rect(Rect::from_min_max(
                                    pos2(x, row.top()),
                                    pos2((row.max.x - 58.0).max(x), row.bottom()),
                                ))
                                .galley(
                                    pos2(x, row.center().y - label_galley.size().y / 2.0),
                                    label_galley,
                                    crate::theme::TEXT,
                                );
                            let size_color = if selected { MUTED } else { FAINT };
                            let size = painter.layout_no_wrap(
                                format_bytes(node.logical),
                                egui::FontId::proportional(10.5),
                                size_color,
                            );
                            let size_left = row.max.x - 8.0 - size.size().x;
                            if !node.complete {
                                painter.circle_filled(
                                    pos2(size_left - 7.0, row.center().y),
                                    2.5,
                                    DANGER,
                                );
                            }
                            painter.galley(
                                pos2(size_left, row.center().y - size.size().y / 2.0),
                                size,
                                size_color,
                            );
                            if toggle {
                                if !self.expanded.remove(&id) {
                                    self.expanded.insert(id);
                                }
                                self.visible_dirty = true;
                            } else if response.clicked() && node.is_dir {
                                navigate = Some(id);
                            }
                            let response = response.on_hover_text(pretty_path(&tree.path(id)));
                            if !node.complete {
                                response.on_hover_text(node_error(
                                    node,
                                    t("Contains incomplete entries", "包含不完整条目"),
                                ));
                            }
                        }
                    },
                );
                self.visible_rows = rows;
            });
        self.handle_keys(ctx, &tree);
        self.show_info_panel(ctx, &tree);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(crate::theme::PANEL)
                    .inner_margin(Margin::symmetric(12, 8)),
            )
            .show(ctx, |ui| {
                self.treemap_toolbar(ui, &tree, ctx);
                let current = self.tree_current;
                let list_height = 250.0_f32;
                let size = Vec2::new(
                    ui.available_width(),
                    (ui.available_height() - list_height).max(200.0),
                );
                let (area, _) = ui.allocate_exact_size(size, Sense::hover());
                ui.painter()
                    .rect_filled(area, CornerRadius::same(10), crate::theme::BG);
                ui.painter().rect_stroke(
                    area,
                    CornerRadius::same(10),
                    Stroke::new(1.0_f32, STROKE),
                    egui::StrokeKind::Inside,
                );
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
                        Color32::from_rgb(62, 72, 84)
                    } else {
                        crate::classify::shade(
                            crate::classify::classify(&node.name, node.is_dir).color(),
                            tile.depth,
                        )
                    };
                    let mut fill = if marked {
                        DANGER
                    } else if covered {
                        DANGER.linear_multiply(0.7)
                    } else {
                        base
                    };
                    let tile_hovered = ctx.pointer_hover_pos().is_some_and(|p| rect.contains(p));
                    if tile_hovered && !marked && !covered {
                        fill = fill.linear_multiply(1.16);
                    }
                    let body = rect.shrink(1.2);
                    ui.painter().rect_filled(body, CornerRadius::same(3), fill);
                    if tile_hovered && !marked && !covered {
                        ui.painter().rect_stroke(
                            body,
                            CornerRadius::same(3),
                            Stroke::new(1.2_f32, ACCENT_STRONG.gamma_multiply(0.8)),
                            egui::StrokeKind::Inside,
                        );
                    }
                    if reclaim && !marked && !covered {
                        paint_hatch(ui, body);
                    }
                    let label = if tile.remainder {
                        t("other entries", "其他条目").to_owned()
                    } else if id == 0 {
                        pretty_path(&tree.root)
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
                        let text_painter = ui.painter().with_clip_rect(body);
                        let font =
                            egui::FontId::proportional(if tile.depth == 0 { 13.0 } else { 11.0 });
                        let at = body.min + Vec2::new(5.0, 4.0);
                        text_painter.text(
                            at + Vec2::new(0.0, 1.0),
                            egui::Align2::LEFT_TOP,
                            &text,
                            font.clone(),
                            Color32::from_black_alpha(130),
                        );
                        text_painter.text(
                            at,
                            egui::Align2::LEFT_TOP,
                            text,
                            font,
                            Color32::from_gray(242),
                        );
                    }
                    if self.focus == Some(id) {
                        ui.painter().rect_stroke(
                            body,
                            CornerRadius::same(3),
                            egui::Stroke::new(2.0_f32, AMBER),
                            egui::StrokeKind::Inside,
                        );
                    } else if marked {
                        ui.painter().rect_stroke(
                            body,
                            CornerRadius::same(3),
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
                            ui.label(pretty_path(&tree.path(id)));
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
                                    DANGER,
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
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(crate::t_format!(
                            "{} entries here",
                            "此处 {} 项",
                            tree.child_ids(current).count()
                        ))
                        .size(12.0)
                        .color(MUTED),
                    );
                    ui.label(
                        RichText::new(t("logical · on disk · files", "逻辑 · 磁盘 · 文件"))
                            .size(11.0)
                            .color(FAINT),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
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
                    .max_height((ui.available_height() - 6.0).max(60.0))
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
                                ui.label(
                                    RichText::new(format!(
                                        "{:>9}  {:>9}  {:>7}",
                                        format_bytes(node.logical),
                                        format_bytes(node.allocated),
                                        node.files
                                    ))
                                    .monospace()
                                    .size(12.0)
                                    .color(MUTED),
                                );
                                if node.is_dir {
                                    if ui
                                        .link(
                                            RichText::new(node.name.to_string_lossy())
                                                .color(ACCENT_STRONG),
                                        )
                                        .clicked()
                                    {
                                        navigate = Some(id);
                                    }
                                } else {
                                    ui.label(node.name.to_string_lossy());
                                }
                                if marked {
                                    ui.colored_label(DANGER, t("marked", "已标记"));
                                } else if covered {
                                    ui.colored_label(
                                        DANGER,
                                        t("in marked folder", "在已标记目录内"),
                                    )
                                    .on_hover_text(t(
                                        "A parent directory is already marked",
                                        "上级目录已被标记",
                                    ));
                                }
                                if self.reclaim_dirs.contains(&id) {
                                    ui.colored_label(AMBER, t("reclaimable", "可能回收"));
                                }
                                if !node.complete {
                                    ui.colored_label(DANGER, t("incomplete", "不完整"))
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
            self.path = pretty_path(parent);
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
                pretty_path(&tree.path(id)),
                pretty_path(&tree.path(covered))
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
        if self.scan_root.to_str().is_some() {
            self.path = pretty_path(&self.scan_root);
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
        ui.horizontal(|ui| {
            egui::Frame::new()
                .fill(SURFACE)
                .stroke(Stroke::new(1.0_f32, STROKE))
                .corner_radius(CornerRadius::same(9))
                .inner_margin(Margin::symmetric(10, 5))
                .show(ui, |ui| {
                    egui::ScrollArea::horizontal()
                        .id_salt("breadcrumb")
                        .auto_shrink([true, false])
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let mut chain = vec![self.tree_current];
                                chain.extend(
                                    ancestor_ids(tree, self.tree_current).into_iter().rev(),
                                );
                                for (index, id) in chain.iter().copied().enumerate() {
                                    if index > 0 {
                                        ui.label(RichText::new("/").color(FAINT));
                                    }
                                    let label = if id == 0 {
                                        pretty_path(&tree.root)
                                    } else {
                                        tree.nodes[id as usize].name.to_string_lossy().into_owned()
                                    };
                                    let text = if id == self.tree_current {
                                        RichText::new(label).strong().color(ACCENT_STRONG)
                                    } else {
                                        RichText::new(label).color(MUTED)
                                    };
                                    if ui.selectable_label(id == self.tree_current, text).clicked()
                                    {
                                        self.open_node(tree, id);
                                    }
                                }
                            });
                        });
                });
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            egui::Frame::new()
                .fill(BG)
                .stroke(Stroke::new(1.0_f32, STROKE))
                .corner_radius(CornerRadius::same(9))
                .inner_margin(Margin::same(3))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for metric in [Metric::Logical, Metric::Allocated, Metric::Files] {
                            let selected = self.metric == metric;
                            let button = egui::Button::new(
                                RichText::new(metric.label()).size(12.0).color(if selected {
                                    crate::theme::ACCENT_INK
                                } else {
                                    MUTED
                                }),
                            )
                            .fill(if selected {
                                ACCENT
                            } else {
                                Color32::TRANSPARENT
                            })
                            .stroke(Stroke::NONE)
                            .corner_radius(CornerRadius::same(7))
                            .min_size(Vec2::new(0.0, 24.0));
                            if ui.add(button).clicked() {
                                self.metric = metric;
                            }
                        }
                    });
                });
            ui.label(RichText::new(t("Depth", "层数")).size(12.0).color(MUTED));
            ui.add(egui::DragValue::new(&mut self.depth).range(1..=6));
            if crate::theme::toggle_chip(ui, self.hide_dot, t("Hide dotfiles", "隐藏点号文件"))
                .clicked()
            {
                self.hide_dot = !self.hide_dot;
            }
            if !self.marked.is_empty() {
                let (logical, _) = self.marked_sizes(tree);
                let review = egui::Button::new(
                    RichText::new(crate::t_format!(
                        "Review {} ({})",
                        "核对 {}（{}）",
                        self.marked.len(),
                        format_bytes(logical)
                    ))
                    .strong()
                    .color(crate::theme::ACCENT_INK),
                )
                .fill(ACCENT)
                .corner_radius(CornerRadius::same(9))
                .min_size(Vec2::new(0.0, 28.0));
                if ui.add(review).clicked() {
                    self.review_open = true;
                }
                if ui
                    .add(
                        egui::Button::new(RichText::new(t("Clear marks", "清除标记")).size(12.0))
                            .fill(Color32::TRANSPARENT)
                            .stroke(Stroke::NONE),
                    )
                    .clicked()
                {
                    self.marked.clear();
                    self.forget_plan();
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .add_enabled(
                        self.rx.is_none(),
                        egui::Button::new(RichText::new(t("Rescan", "重新扫描")).size(12.0))
                            .fill(SURFACE)
                            .stroke(Stroke::new(1.0_f32, STROKE))
                            .corner_radius(CornerRadius::same(9))
                            .min_size(Vec2::new(0.0, 28.0)),
                    )
                    .clicked()
                {
                    self.rescan(ctx);
                }
            });
        });
        ui.add_space(2.0);
    }
    fn show_info_panel(&mut self, ctx: &egui::Context, tree: &crate::tree::Snapshot) {
        egui::SidePanel::right("details")
            .resizable(true)
            .default_width(310.0)
            .min_width(240.0)
            .frame(
                egui::Frame::new()
                    .fill(crate::theme::PANEL)
                    .inner_margin(Margin::symmetric(10, 10)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("inspector")
                    .show(ui, |ui| {
                        section(ui, t("Volume", "卷"), |ui| self.disk_panel(ui, tree));
                        section(ui, t("Entry", "条目"), |ui| {
                            match self.hover_tile.or(self.focus) {
                                Some(id) => self.node_panel(ui, tree, id),
                                None => {
                                    ui.label(
                                    RichText::new(t(
                                        "Hover a tile or press an arrow key to inspect an entry.",
                                        "悬停方块或按方向键查看条目。",
                                    ))
                                    .size(12.0)
                                    .color(MUTED),
                                );
                                }
                            }
                        });
                        section(ui, t("Marks", "标记清单"), |ui| {
                            self.marks_panel(ui, tree)
                        });
                        section(ui, t("Worth a look", "值得一看"), |ui| {
                            self.suggestion_panel(ui, tree)
                        });
                    });
            });
    }
    fn disk_panel(&mut self, ui: &mut egui::Ui, tree: &crate::tree::Snapshot) {
        match self.disk {
            Some(space) if space.total > 0 => {
                let used = space.total.saturating_sub(space.available);
                let fraction = used as f32 / space.total as f32;
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .desired_height(10.0)
                        .corner_radius(CornerRadius::same(5))
                        .fill(ACCENT)
                        .text(crate::t_format!(
                            "{:.0}% used",
                            "{:.0}% 已用",
                            fraction * 100.0
                        )),
                );
                ui.add_space(4.0);
                ui.label(
                    RichText::new(crate::t_format!(
                        "{} available",
                        "{} 可用",
                        format_bytes(space.available)
                    ))
                    .strong(),
                );
                ui.label(
                    RichText::new(crate::t_format!(
                        "{} free · {} total",
                        "{} 空闲 · 共 {}",
                        format_bytes(space.free),
                        format_bytes(space.total)
                    ))
                    .size(12.0)
                    .color(MUTED),
                );
                let (logical, allocated) = self.marked_sizes(tree);
                if logical > 0 {
                    let projected = space.available.saturating_add(logical).min(space.total);
                    ui.add_space(4.0);
                    egui::Frame::new()
                        .fill(DANGER.gamma_multiply(0.12))
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(Margin::same(6))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(crate::t_format!(
                                    "{} marked — free space could become {}",
                                    "已标记 {} — 可用空间可变为 {}",
                                    format_bytes(logical),
                                    format_bytes(projected)
                                ))
                                .size(12.0),
                            );
                            ui.label(
                                RichText::new(crate::t_format!(
                                    "{} attributed on disk, not guaranteed reclaimed space",
                                    "{} 归属磁盘占用，不保证可回收",
                                    format_bytes(allocated)
                                ))
                                .size(11.0)
                                .color(MUTED),
                            );
                        });
                }
            }
            Some(_) => {
                ui.label(t(
                    "This volume did not report a capacity.",
                    "该卷未报告容量。",
                ));
            }
            None => {
                ui.label(
                    RichText::new(t(
                        "Reading the capacity of the scanned volume...",
                        "正在读取扫描卷的容量…",
                    ))
                    .color(MUTED),
                );
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
            pretty_path(&tree.root)
        } else {
            node.name.to_string_lossy().into_owned()
        };
        ui.label(RichText::new(name).size(16.0).strong());
        ui.label(
            RichText::new(pretty_path(&tree.path(id)))
                .size(11.0)
                .color(FAINT),
        );
        let kind = crate::classify::classify(&node.name, node.is_dir);
        ui.add_space(2.0);
        crate::theme::badge(ui, format!("{kind:?}"), kind.color());
        ui.add_space(4.0);
        crate::theme::inset().show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format_bytes(node.logical))
                        .size(15.0)
                        .strong(),
                );
                ui.label(RichText::new(t("logical", "逻辑")).size(11.0).color(MUTED));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(crate::t_format!(
                            "{} files · {} dirs",
                            "{} 文件 · {} 目录",
                            node.files,
                            node.directories
                        ))
                        .size(11.0)
                        .color(MUTED),
                    );
                });
            });
            ui.label(
                RichText::new(crate::t_format!(
                    "{} on disk",
                    "磁盘 {}",
                    format_bytes(node.allocated)
                ))
                .size(11.0)
                .color(MUTED),
            );
        });
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
                    pretty_path(&tree.path(covered))
                ),
            );
        }
        if !node.complete {
            ui.colored_label(
                DANGER,
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
                            .unwrap_or_else(|| pretty_path(path));
                        ui.label(format!("{} · {}", format_bytes(logical), label))
                            .on_hover_text(pretty_path(path));
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
                        .on_hover_text(pretty_path(&candidate.path));
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
                                ui.label(pretty_path(path));
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
                    ui.label(RichText::new(&self.plan_details).color(DANGER));
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
            if !report.complete { ui.colored_label(DANGER,crate::t_format!("Incomplete: {} read errors or incomplete tree", "不完整: {} 个读取错误或树不完整",report.error_count)); }
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
                for error in &report.errors {ui.colored_label(DANGER,error);}
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
            if !report.complete { ui.colored_label(DANGER, t("Scan incomplete; suggestions may be missing or sizes inaccurate.", "扫描不完整；建议可能缺失或大小不准确。")); }
            if report.total_candidates > report.candidates.len() as u64 { ui.colored_label(DANGER, crate::t_format!("Showing {} of {} suggestions", "显示 {} / {} 条建议",report.candidates.len(),report.total_candidates)); }
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
                    ui.colored_label(AMBER, c.risk);
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
                        ui.label(crate::t_format!("{} bytes", "{} 字节",hit.logical));ui.label(hit.path.to_string_lossy());if !hit.complete {ui.colored_label(DANGER,"incomplete");}
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
                .map(|t| t.id)
                .unwrap()
        };
        let marked_id = file_tile(&app);
        let expected = app.tree.as_ref().unwrap().path(marked_id);
        let tree = app.tree.clone().unwrap();
        // Synthetic pointer positions depend on the headless panel geometry, which
        // differs between platforms; mark through the same entry point the tile
        // click handler uses.
        app.toggle_mark(&tree, marked_id);
        assert!(app.marked.contains(&marked_id));
        assert!(app.plan_inputs(&tree).contains(&expected));
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
