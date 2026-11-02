use crate::{delete_checked, plan_description, read_checked_plan, save_plan, scan_with_threads};
use eframe::egui::{self, Color32, RichText, Sense, Vec2};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver},
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
struct App {
    path: String,
    filter: String,
    threads: u8,
    entries: Vec<serde_json::Value>,
    selected: Vec<PathBuf>,
    status: String,
    rx: Option<Receiver<Result<serde_json::Value, String>>>,
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
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        self.status = "Scanning...".into();
        self.entries.clear();
        self.selected.clear();
        self.plan_data = None;
        self.rx = Some(rx);
        thread::spawn(move || {
            let _ = tx.send(scan_with_threads(&path, threads));
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
                Ok(v) => {
                    self.entries = v["entries"].as_array().cloned().unwrap_or_default();
                    self.status = format!(
                        "{} entries. Attributed allocation: {} bytes{}",
                        self.entries.len(),
                        v["total_allocated_bytes"].as_u64().unwrap_or(0),
                        if v["complete"] == false {
                            " (incomplete)"
                        } else {
                            ""
                        }
                    );
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
                if ui.button("Up").clicked()
                    && let Some(p) = std::path::Path::new(&self.path).parent()
                {
                    self.path = p.display().to_string();
                    self.begin_scan(ctx);
                }
            });
            ui.horizontal(|ui| {
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
                            ui.monospace(format!("{n:>14}"));
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
