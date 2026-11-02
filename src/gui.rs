use crate::{
    CleanupMode, ScanEngine, delete_checked, plan_description, read_checked_plan, save_plan_mode,
    scan_dispatch,
};
use eframe::egui::{self, Color32, RichText, Sense, Vec2};
use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
};

pub fn run() -> Result<(), String> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([700.0, 450.0]),
        ..Default::default()
    };
    eframe::run_native(
        "filetree233",
        options,
        Box::new(|cc| {
            install_system_font(&cc.egui_ctx);
            Ok(Box::new(App::default()))
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
enum ScanResult {
    Summary(serde_json::Value),
    Tree(crate::tree::Snapshot),
}
struct App {
    path: String,
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
    treemap_current: u32,
    treemap_rect: egui::Rect,
    tree: Option<Arc<crate::tree::Snapshot>>,
    duplicates: Option<crate::duplicates::Report>,
    duplicate_rx: Option<Receiver<Result<crate::duplicates::Report, String>>>,
    duplicate_cancelled: Arc<AtomicBool>,
    duplicate_progress: Arc<AtomicU64>,
    duplicate_window_open: bool,
    recommendations: Option<crate::recommend::Report>,
    recommendation_rx: Option<Receiver<Result<crate::recommend::Report, String>>>,
    recommendation_window_open: bool,
    #[cfg(test)]
    suggestion_rects: Vec<(PathBuf, egui::Rect)>,
    tree_current: u32,
    expanded: HashSet<u32>,
    visible_rows: Vec<(u32, usize)>,
    visible_dirty: bool,
    list_rows: Vec<u32>,
    list_current: u32,
    list_filter: String,
    cancelled: Arc<AtomicBool>,
    progress: Arc<AtomicU64>,
    entries: Vec<serde_json::Value>,
    selected: Vec<PathBuf>,
    status: String,
    rx: Option<Receiver<Result<ScanResult, String>>>,
    cleanup_mode: CleanupMode,
    plan_path: String,
    plan_data: Option<Vec<u8>>,
    plan_details: String,
    token: String,
    confirm: String,
}
impl Default for App {
    fn default() -> Self {
        Self {
            path: std::env::current_dir()
                .unwrap_or_default()
                .display()
                .to_string(),
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
            treemap_current: u32::MAX,
            treemap_rect: egui::Rect::NOTHING,
            tree: None,
            duplicates: None,
            duplicate_rx: None,
            duplicate_cancelled: Arc::new(AtomicBool::new(false)),
            duplicate_progress: Arc::new(AtomicU64::new(0)),
            duplicate_window_open: false,
            recommendations: None,
            recommendation_rx: None,
            recommendation_window_open: false,
            #[cfg(test)]
            suggestion_rects: Vec::new(),
            tree_current: 0,
            expanded: HashSet::new(),
            visible_rows: Vec::new(),
            visible_dirty: true,
            list_rows: Vec::new(),
            list_current: u32::MAX,
            list_filter: String::new(),
            cancelled: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(AtomicU64::new(0)),
            entries: vec![],
            selected: vec![],
            status: "Choose a directory and scan. Sizes are logical bytes.".into(),
            rx: None,
            plan_path: std::env::temp_dir()
                .join("filetree233-plan.json")
                .display()
                .to_string(),
            cleanup_mode: CleanupMode::Recycle,
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
                .selected_text("Choose volume")
                .width(240.0)
                .show_ui(ui, |ui| {
                    if self.volumes.is_empty() {
                        ui.label("No volumes loaded");
                    }
                    for volume in &self.volumes {
                        let label = format!(
                            "{} {} {}{}{} available {} / {}",
                            volume.mount.display(),
                            volume.name,
                            volume.filesystem,
                            if volume.removable { " removable" } else { "" },
                            if volume.read_only { " read-only" } else { "" },
                            if volume.total == 0 {
                                "unknown".into()
                            } else {
                                format_bytes(volume.available)
                            },
                            if volume.total == 0 {
                                "unknown".into()
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
                    egui::Button::new("Refresh volumes"),
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
            self.status = "This path cannot be represented in the text field; navigation remains in the current snapshot".into();
        }
    }
    fn begin_volumes(&mut self, ctx: &egui::Context) {
        if self.volume_rx.is_some() {
            return;
        }
        self.volumes_requested = true;
        self.volume_status = "Loading volumes...".into();
        let (tx, rx) = mpsc::channel();
        self.volume_rx = Some(rx);
        let ctx = ctx.clone();
        thread::spawn(move || {
            let result = std::panic::catch_unwind(crate::volumes::list)
                .unwrap_or_else(|_| Err("volume discovery failed unexpectedly".into()));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
    fn begin_scan(&mut self, ctx: &egui::Context) {
        if self.rx.is_some() {
            return;
        }
        let path = PathBuf::from(&self.path);
        let threads = self.threads;
        let engine = self.engine;
        let tree_enabled = self.full_tree;
        self.scan_is_tree = tree_enabled;
        self.treemap_current = u32::MAX;
        self.treemap_tiles.clear();
        let (cancelled, progress) = crate::tree::new_controls();
        self.cancelled = cancelled.clone();
        self.progress = progress.clone();
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        self.status = "Scanning...".into();
        self.entries.clear();
        self.tree = None;
        self.duplicate_cancelled.store(true, Ordering::Relaxed);
        self.duplicate_rx = None;
        self.duplicates = None;
        self.duplicate_window_open = false;
        self.recommendations = None;
        self.recommendation_rx = None;
        self.recommendation_window_open = false;
        self.tree_current = 0;
        self.expanded.clear();
        self.expanded.insert(0);
        self.visible_rows.clear();
        self.visible_dirty = true;
        self.list_rows.clear();
        self.list_current = u32::MAX;
        self.selected.clear();
        self.plan_data = None;
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
            .unwrap_or_else(|_| Err("scan worker failed unexpectedly".into()));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }
}
impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.volumes_requested {
            self.begin_volumes(ctx);
        }
        if let Some(rx) = &self.volume_rx
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(volumes) => {
                    self.volume_status = if volumes.is_empty() {
                        "No volumes detected".into()
                    } else {
                        String::new()
                    };
                    self.volumes = volumes;
                }
                Err(error) => self.volume_status = format!("Volume discovery: {error}"),
            }
            self.volume_rx = None;
        }
        if self.volume_rx.is_some() {
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
                    self.status = format!(
                        "{} tree: {} entries, logical {} bytes, allocated {} bytes{}",
                        tree.engine,
                        tree.nodes.len().saturating_sub(1),
                        tree.nodes[0].logical,
                        tree.nodes[0].allocated,
                        if tree.nodes[0].complete {
                            ""
                        } else {
                            " (incomplete)"
                        }
                    );
                    if !root_representable {
                        self.status
                            .push_str("; root path cannot be represented in the text field");
                    }
                    if let Some(reason) = &tree.fallback_reason {
                        self.status.push_str(&format!("; fallback: {reason}"));
                    }
                    self.tree = Some(Arc::new(tree));
                }
                Ok(ScanResult::Summary(v)) => {
                    self.entries = v["entries"].as_array().cloned().unwrap_or_default();
                    self.status = format!(
                        "{}: {} entries. Attributed allocation: {} bytes{}",
                        v["engine"].as_str().unwrap_or("unknown"),
                        self.entries.len(),
                        v["total_allocated_bytes"].as_u64().unwrap_or(0),
                        if v["complete"] == false {
                            " (incomplete)"
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
                    self.status = format!(
                        "Duplicate search: {} groups{}",
                        report.groups.len(),
                        if report.complete { "" } else { " (incomplete)" }
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
                    self.status = format!(
                        "Cleanup suggestions: {}{}",
                        report.total_candidates,
                        if report.complete {
                            ""
                        } else {
                            " (incomplete scan)"
                        }
                    );
                    self.recommendations = Some(report);
                    self.recommendation_window_open = true;
                }
                Err(error) => self.status = error,
            }
            self.recommendation_rx = None;
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
                ui.label("Directory");
                ui.add(egui::TextEdit::singleline(&mut self.path).desired_width(600.0));
                if ui.button("Scan").clicked() {
                    self.begin_scan(ctx);
                }
                if ui.button("Up").clicked() {
                    if let Some(tree) = &self.tree {
                        if let Some(parent) = tree.parent(self.tree_current) {
                            self.tree_current = parent;
                            let path = tree.path(parent);
                            self.set_tree_path(&path);
                        }
                    } else if let Some(p) = std::path::Path::new(&self.path).parent() {
                        self.path = p.display().to_string();
                        self.begin_scan(ctx);
                    }
                }
                if self.rx.is_some() && self.scan_is_tree && ui.button("Cancel").clicked() {
                    self.cancelled.store(true, Ordering::Relaxed);
                }
            });
            ui.horizontal(|ui| {
                egui::ComboBox::from_label("Engine")
                    .selected_text(format!("{:?}", self.engine))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.engine, ScanEngine::Walk, "Walk");
                        ui.selectable_value(
                            &mut self.engine,
                            ScanEngine::Mft,
                            "MFT (experimental)",
                        );
                        ui.selectable_value(
                            &mut self.engine,
                            ScanEngine::Auto,
                            "Auto (experimental)",
                        );
                    });
                ui.checkbox(&mut self.full_tree, "Full tree");
                if self.rx.is_some() && self.scan_is_tree {
                    ui.label(format!("{} entries", self.progress.load(Ordering::Relaxed)));
                    ctx.request_repaint_after(std::time::Duration::from_millis(250));
                }
                ui.label("Workers");
                ui.add(egui::DragValue::new(&mut self.threads).range(1..=32));
                ui.label("Filter");
                ui.text_edit_singleline(&mut self.filter);
                ui.label(&self.status);
            });
        });
        egui::TopBottomPanel::bottom("bottom")
            .resizable(true)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Plan file");
                    ui.add(egui::TextEdit::singleline(&mut self.plan_path).desired_width(420.0));
                    egui::ComboBox::from_id_salt("cleanup_mode")
                        .selected_text(match self.cleanup_mode {
                            CleanupMode::Recycle => "Recycle bin",
                            CleanupMode::Permanent => "Permanent",
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.cleanup_mode,
                                CleanupMode::Recycle,
                                "Recycle bin",
                            );
                            ui.selectable_value(
                                &mut self.cleanup_mode,
                                CleanupMode::Permanent,
                                "Permanent",
                            );
                        });
                    if ui.button("Create cleanup plan").clicked() {
                        match save_plan_mode(
                            &self.selected,
                            &PathBuf::from(&self.plan_path),
                            self.cleanup_mode,
                        ) {
                            Ok(_) => match read_checked_plan(&PathBuf::from(&self.plan_path)) {
                                Ok((data, plan)) => {
                                    let (message, token) = plan_description(&data, &plan);
                                    self.plan_details = message;
                                    self.token = token;
                                    self.confirm.clear();
                                    self.plan_data = Some(data);
                                }
                                Err(e) => self.status = e,
                            },
                            Err(e) => self.status = e,
                        }
                    }
                });
                if self.plan_data.is_some() {
                    ui.label(RichText::new(&self.plan_details).color(Color32::LIGHT_RED));
                    ui.horizontal(|ui| {
                        ui.label("Confirmation");
                        ui.text_edit_singleline(&mut self.confirm);
                        if ui.button("Apply confirmed plan").clicked() {
                            if self.confirm == self.token {
                                let data = self.plan_data.take().unwrap();
                                match delete_checked(&PathBuf::from(&self.plan_path), &data) {
                                    Ok(v) => {
                                        self.status = format!("{v}");
                                        self.entries.clear();
                                        self.tree = None;
                                        self.duplicates = None;
                                        self.duplicate_window_open = false;
                                        self.recommendations = None;
                                        self.recommendation_rx = None;
                                        self.recommendation_window_open = false;
                                        self.selected.clear();
                                    }
                                    Err(e) => self.status = e,
                                }
                            } else {
                                self.status = "Confirmation did not match; nothing deleted".into();
                            }
                        }
                    });
                }
            });
        if self.tree.is_some() {
            self.show_tree(ctx);
            self.show_duplicates(ctx);
            self.show_recommendations(ctx);
            return;
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label(
                "Logical bytes | allocated bytes* | path (*hard links attributed once across scan)",
            );
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
                    response.on_hover_text(format!(
                        "{}: {} bytes",
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
                            ui.colored_label(Color32::LIGHT_RED, "ERROR");
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
                                    "raw path; see exact code units in plan",
                                );
                            }
                        }
                        if entry["complete"] == false {
                            ui.colored_label(
                                Color32::LIGHT_RED,
                                format!(
                                    "Incomplete: {}",
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
                ui.heading("Directory tree");
                ui.horizontal(|ui| {
                    if self.duplicate_rx.is_none() {
                        if ui.button("Find duplicates").clicked() {
                            self.begin_duplicates(Arc::clone(&tree), ctx);
                        }
                        if self.duplicates.is_some() && ui.button("Show duplicates").clicked() {
                            self.duplicate_window_open = true;
                        }
                    } else {
                        ui.label(format!(
                            "Hashing: {}",
                            self.duplicate_progress.load(Ordering::Relaxed)
                        ));
                        if ui.button("Cancel search").clicked() {
                            self.duplicate_cancelled.store(true, Ordering::Relaxed);
                        }
                    }
                });
                ui.horizontal(|ui| {
                    if self.recommendation_rx.is_none() {
                        if ui.button("Suggest cleanup").clicked() {
                            self.begin_recommendations(Arc::clone(&tree), ctx);
                        }
                        if self.recommendations.is_some() && ui.button("Show suggestions").clicked()
                        {
                            self.recommendation_window_open = true;
                        }
                    } else {
                        ui.label("Analyzing suggestions...");
                    }
                });
                if self.visible_dirty {
                    self.visible_rows.clear();
                    let mut pending = vec![(0u32, 0usize)];
                    while let Some((id, depth)) = pending.pop() {
                        self.visible_rows.push((id, depth));
                        if tree.nodes[id as usize].is_dir && self.expanded.contains(&id) {
                            for child in tree.children(id).into_iter().rev() {
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
                                    node.error
                                        .as_deref()
                                        .unwrap_or("Contains incomplete entries"),
                                );
                            }
                        });
                    }
                });
                self.visible_rows = rows;
            });
        egui::CentralPanel::default().show(ctx, |ui| {
            let current = self.tree_current;
            ui.label(format!("{} — {} logical bytes", tree.path(current).display(), tree.nodes[current as usize].logical));
            ui.label("Treemap: click a directory to enter it, or a file to select it. Small regions can represent a directory.");
            let size = Vec2::new(ui.available_width(), 220.0);
            let (area, _) = ui.allocate_exact_size(size, Sense::hover());
            if self.treemap_current != current || self.treemap_rect != area {
                self.treemap_tiles = crate::treemap::layout(&tree, current, area, 2048);
                self.treemap_current = current; self.treemap_rect = area;
            }
            for tile in self.treemap_tiles.iter().copied() {
                let id = tile.id; let rect = tile.rect;
                let n = &tree.nodes[id as usize];
                let color = Color32::from_rgb(50 + ((id.wrapping_mul(37) % 120) as u8), 65 + ((id.wrapping_mul(71) % 110) as u8), 105 + ((id.wrapping_mul(29) % 100) as u8));
                ui.painter().rect_filled(rect.shrink(0.5), 0.0, color);
                if rect.width() > 70.0 && rect.height() > 20.0 {
                    let label = if tile.remainder { "Other entries".to_string() } else if id == 0 { tree.root.display().to_string() } else { n.name.to_string_lossy().into_owned() };
                    ui.painter().with_clip_rect(rect).text(rect.min + Vec2::new(3.0,3.0), egui::Align2::LEFT_TOP, label, egui::FontId::proportional(12.0), Color32::WHITE);
                }
                if !tile.remainder && !self.selected.is_empty() && self.selected.contains(&tree.path(id)) {
                    ui.painter().rect_stroke(rect.shrink(1.0), 0.0, egui::Stroke::new(2.0_f32, Color32::YELLOW), egui::StrokeKind::Inside);
                }
                let response=ui.interact(rect,ui.id().with(("tile",id)),Sense::click()).on_hover_ui(|ui| {
                    if tile.remainder { ui.label(format!("Other entries\n{} logical bytes", tile.logical)); }
                    else { ui.label(format!("{}\n{} logical bytes; {} allocated bytes",tree.path(id).display(),n.logical,n.allocated)); }
                });
                if response.clicked() && !tile.remainder {
                    if n.is_dir && id != current { navigate=Some(id); }
                    else if !n.is_dir { let path=tree.path(id); if self.selected.contains(&path) { self.selected.retain(|p|p!=&path); } else { self.selected.push(path); } self.plan_data=None; }
                }
            }
            ui.separator();
            ui.label("Logical bytes | allocated bytes | name");
            if self.list_current != current || self.list_filter != self.filter {
                let filter = self.filter.to_lowercase();
                self.list_rows = tree.child_ids(current).filter(|id| tree.nodes[*id as usize].name.to_string_lossy().to_lowercase().contains(&filter)).collect();
                self.list_current = current; self.list_filter = self.filter.clone();
            }
            let children = std::mem::take(&mut self.list_rows);
            egui::ScrollArea::vertical().show_rows(ui, 23.0, children.len(), |ui,range| {
                for row in range {
                    let id=children[row]; let n=&tree.nodes[id as usize]; let path=tree.path(id);
                    ui.horizontal(|ui| {
                        let mut selected=self.selected.contains(&path);
                        if ui.checkbox(&mut selected, "").changed() { if selected { self.selected.push(path.clone()); } else { self.selected.retain(|p|p!=&path); } self.plan_data=None; }
                        ui.monospace(format!("{:>14} | {:>14}",n.logical,n.allocated));
                        if n.is_dir { if ui.link(n.name.to_string_lossy()).clicked() {navigate=Some(id);} } else {ui.label(n.name.to_string_lossy());}
                        if !n.complete {ui.colored_label(Color32::LIGHT_RED,"incomplete").on_hover_text(n.error.as_deref().unwrap_or("Contains incomplete entries"));}
                    });
                }
            });
            self.list_rows = children;
        });
        if let Some(id) = navigate {
            self.tree_current = id;
            if self.expanded.insert(id) {
                self.visible_dirty = true;
            }
            self.set_tree_path(&tree.path(id));
        }
        self.tree = Some(tree);
    }
}

impl App {
    fn begin_duplicates(&mut self, tree: Arc<crate::tree::Snapshot>, ctx: &egui::Context) {
        if self.duplicate_rx.is_some() {
            return;
        }
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
            .unwrap_or_else(|_| Err("duplicate search failed unexpectedly".into()));
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
        egui::Window::new("Content-verified duplicates").open(&mut open).resizable(true).default_size([700.0,500.0]).show(ctx,|ui| {
            ui.label("Select files explicitly, then create a cleanup plan. Hard links are aliases of the same file; actual reclaimed space may differ.");
            if !report.complete { ui.colored_label(Color32::LIGHT_RED,format!("Incomplete: {} read errors or incomplete tree",report.error_count)); }
            egui::ScrollArea::vertical().show(ui,|ui| {
                for group in &report.groups {
                    ui.separator();ui.strong(format!("{} bytes — {} independent files — SHA-256 {}",group.size,group.objects.len(),group.sha256));
                    for object in &group.objects {
                        for (path,alias) in std::iter::once((&object.path,false)).chain(object.aliases.iter().map(|p|(p,true))) {
                            ui.horizontal(|ui| {
                                let mut selected=self.selected.contains(path);
                                if ui.checkbox(&mut selected,"").changed() { if selected {self.selected.push(path.clone());} else {self.selected.retain(|p|p!=path);}self.plan_data=None; }
                                ui.label(path.to_string_lossy());if alias {ui.label("(hard-link alias)");}
                            });
                        }
                    }
                }
                if report.groups.is_empty() {ui.label("No content-verified duplicate files found.");}
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
            .map_err(|_| "suggestion worker failed unexpectedly".to_string());
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
        egui::Window::new("Cleanup suggestions").open(&mut open).resizable(true).default_size([760.0, 520.0]).show(ctx, |ui| {
            ui.label("Heuristics only. Review each path and risk; no files are selected automatically. Create a cleanup plan to verify and confirm.");
            if !report.complete { ui.colored_label(Color32::LIGHT_RED, "Scan incomplete; suggestions may be missing or sizes inaccurate."); }
            if report.total_candidates > report.candidates.len() as u64 { ui.colored_label(Color32::LIGHT_RED, format!("Showing {} of {} suggestions",report.candidates.len(),report.total_candidates)); }
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
                    ui.label(format!("{} | {} logical bytes | {} attributed allocated bytes | {} files{}",c.rule,c.logical,c.allocated,c.files,if c.complete {""} else {" | INCOMPLETE"}));
                    ui.label(c.reason);
                    ui.colored_label(Color32::YELLOW, c.risk);
                }
                if report.candidates.is_empty() { ui.label("No matching suggestions."); }
            });
        });
        self.recommendation_window_open = open;
        self.recommendations = Some(report);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let tile = app
            .treemap_tiles
            .iter()
            .find(|t| !t.remainder && !app.tree.as_ref().unwrap().nodes[t.id as usize].is_dir)
            .unwrap();
        let position = tile.rect.center();
        let expected = app.tree.as_ref().unwrap().path(tile.id);
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
        assert!(app.selected.contains(&expected));
        std::fs::remove_dir_all(root).unwrap();
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
}
