use crate::{
    ScanEngine, delete_checked, plan_description, read_checked_plan, save_plan, scan_dispatch,
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
enum ScanResult {
    Summary(serde_json::Value),
    Tree(crate::tree::Snapshot),
}
struct App {
    path: String,
    filter: String,
    threads: u8,
    engine: ScanEngine,
    full_tree: bool,
    scan_is_tree: bool,
    treemap_tiles: Vec<crate::treemap::Tile>,
    treemap_current: u32,
    treemap_rect: egui::Rect,
    tree: Option<crate::tree::Snapshot>,
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
            threads: 4,
            engine: ScanEngine::Walk,
            full_tree: true,
            scan_is_tree: false,
            treemap_tiles: Vec::new(),
            treemap_current: u32::MAX,
            treemap_rect: egui::Rect::NOTHING,
            tree: None,
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
            plan_data: None,
            plan_details: String::new(),
            token: String::new(),
            confirm: String::new(),
        }
    }
}
impl App {
    fn begin_scan(&mut self, ctx: &egui::Context) {
        if self.rx.is_some() {
            return;
        }
        let path = PathBuf::from(&self.path);
        let threads = self.threads;
        let engine = self.engine;
        let tree_enabled = self.full_tree && engine == ScanEngine::Walk;
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
                    crate::tree::scan(&path, threads, &cancelled, &progress).map(ScanResult::Tree)
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
        if let Some(rx) = &self.rx
            && let Ok(result) = rx.try_recv()
        {
            match result {
                Ok(ScanResult::Tree(tree)) => {
                    self.path = tree.root.display().to_string();
                    self.status = format!(
                        "Tree: {} entries, logical {} bytes, allocated {} bytes{}",
                        tree.nodes.len().saturating_sub(1),
                        tree.nodes[0].logical,
                        tree.nodes[0].allocated,
                        if tree.nodes[0].complete {
                            ""
                        } else {
                            " (incomplete)"
                        }
                    );
                    self.tree = Some(tree);
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
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.heading("filetree233");
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
                            self.path = tree.path(parent).display().to_string();
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
                ui.add_enabled(
                    self.engine == ScanEngine::Walk,
                    egui::Checkbox::new(&mut self.full_tree, "Full tree (walk)"),
                );
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
                    if ui.button("Create cleanup plan").clicked() {
                        match save_plan(&self.selected, &PathBuf::from(&self.plan_path)) {
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
                        if ui.button("Permanently delete").clicked() {
                            if self.confirm == self.token {
                                let data = self.plan_data.take().unwrap();
                                match delete_checked(&PathBuf::from(&self.plan_path), &data) {
                                    Ok(v) => {
                                        self.status = format!("{v}");
                                        self.entries.clear();
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
                    ui.horizontal(|ui| {
                        {
                            let p = PathBuf::from(path);
                            let mut selected = self.selected.contains(&p);
                            if ui.checkbox(&mut selected, "").changed() {
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
                        if is_dir && ui.link(path).clicked() {
                            navigate = Some(path.to_string());
                        } else if !is_dir {
                            ui.label(path);
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
            self.path = tree.path(id).display().to_string();
        }
        self.tree = Some(tree);
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
            tree: Some(tree),
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
}
