use eframe::egui::{self, Color32, RichText, Vec2};
use egui_plot::{Legend, Line, Plot, PlotPoint, PlotPoints, Points};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::{Duration, Instant};

#[path = "engine.rs"]
mod engine;

const BLUE: Color32 = Color32::from_rgb(0, 112, 199);
const GRAY: Color32 = Color32::from_rgb(225, 225, 231);
const WORKSPACE: Color32 = Color32::from_rgb(224, 224, 229);
const TOOLBAR: Color32 = Color32::from_rgb(238, 239, 248);
const HEADER: Color32 = Color32::from_rgb(216, 228, 239);
const METADATA: Color32 = Color32::from_rgb(255, 255, 207);
const MODELS: [(&str, &str); 4] = [
    ("linear", "线性拟合"),
    ("quadratic", "二次多项式"),
    ("exponential", "指数衰减"),
    ("gaussian", "高斯单峰"),
];

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Column {
    name: String,
    unit: String,
    #[serde(default)]
    comments: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Workbook {
    name: String,
    columns: Vec<Column>,
    rows: Vec<Vec<String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct PlotStyle {
    title: String,
    x_label: String,
    y_label: String,
    kind: String,
    color: [u8; 3],
    width: f32,
    marker_size: f32,
    grid: bool,
    legend: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Parameter {
    name: String,
    value: f64,
    stderr: Option<f64>,
    low: Option<f64>,
    high: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FitResult {
    model: String,
    equation: String,
    params: Vec<Parameter>,
    points: Vec<[f64; 2]>,
    residuals: Vec<[f64; 2]>,
    r2: Option<f64>,
    rmse: f64,
    n: usize,
    dropped: usize,
    weighted: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Statistics {
    count: usize,
    dropped: usize,
    mean: f64,
    median: f64,
    sd: Option<f64>,
    minimum: f64,
    maximum: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Project {
    version: u32,
    books: Vec<Workbook>,
    active: usize,
    x_col: usize,
    y_col: usize,
    error_col: Option<usize>,
    model: String,
    style: PlotStyle,
    #[serde(default)]
    range: Option<[f64; 2]>,
    fit: Option<FitResult>,
    statistics: Option<Statistics>,
}

fn column_label(c: &Column) -> String {
    if c.unit.is_empty() {
        c.name.clone()
    } else {
        format!("{} ({})", c.name, c.unit)
    }
}

fn column_code(index: usize) -> String {
    if index < 26 {
        ((b'A' + index as u8) as char).to_string()
    } else {
        format!("A{}", (b'A' + (index - 26) as u8) as char)
    }
}

impl Project {
    fn blank() -> Self {
        let mut p = Self::demo("linear");
        p.books[0].columns.truncate(2);
        for column in &mut p.books[0].columns {
            column.name.clear();
            column.unit.clear();
        }
        p.books[0].rows = vec![vec![String::new(); 2]; 32];
        p.style.title.clear();
        p.style.x_label = "X".into();
        p.style.y_label = "Y".into();
        p.style.kind = "line".into();
        p
    }
    fn demo(kind: &str) -> Self {
        let (columns, model, title) = match kind {
            "linear" => (
                [
                    ("Concentration", "mg/L"),
                    ("Response", "a.u."),
                    ("Reference", "a.u."),
                    ("SD", "a.u."),
                ],
                "linear",
                "校准曲线",
            ),
            "gaussian" => (
                [
                    ("Raman shift", "cm⁻¹"),
                    ("Intensity", "a.u."),
                    ("Reference", "a.u."),
                    ("SD", "a.u."),
                ],
                "gaussian",
                "单峰光谱",
            ),
            _ => (
                [
                    ("Time", "s"),
                    ("Signal", "mV"),
                    ("Reference", "mV"),
                    ("SD", "mV"),
                ],
                "exponential",
                "指数衰减实验",
            ),
        };
        let columns: Vec<Column> = columns
            .iter()
            .map(|(n, u)| Column {
                name: n.to_string(),
                unit: u.to_string(),
                comments: String::new(),
            })
            .collect();
        let rows = (0..96)
            .map(|i| {
                let t = i as f64 / 8.0;
                let noise = 0.7 * (i as f64 * 2.13).sin() + 0.38 * (i as f64 * 0.77).cos();
                let (x, y, reference) = match kind {
                    "linear" => (t, 2.8 * t + 1.5 + noise, 2.8 * t + 1.5),
                    "gaussian" => {
                        let x = 800.0 + t * 65.0;
                        let y = 8.0 + 62.0 * (-0.5 * ((x - 1200.0) / 72.0).powi(2)).exp();
                        (x, y + noise, y)
                    }
                    _ => {
                        let y = 5.0 + 60.0 * (-t / 3.1).exp();
                        (t, y + noise, y)
                    }
                };
                vec![
                    format!("{x:.5}"),
                    format!("{y:.5}"),
                    format!("{reference:.5}"),
                    "0.8".into(),
                ]
            })
            .collect();
        let style = PlotStyle {
            title: title.into(),
            x_label: column_label(&columns[0]),
            y_label: column_label(&columns[1]),
            kind: "line_points".into(),
            color: [42, 94, 165],
            width: 1.5,
            marker_size: 4.0,
            grid: false,
            legend: true,
        };
        Self {
            version: 1,
            books: vec![Workbook {
                name: "Book1".into(),
                columns,
                rows,
            }],
            active: 0,
            x_col: 0,
            y_col: 1,
            error_col: None,
            model: model.into(),
            style,
            range: None,
            fit: None,
            statistics: None,
        }
    }

    fn book(&self) -> &Workbook {
        &self.books[self.active]
    }

    fn points(&self) -> Vec<[f64; 2]> {
        let mut points: Vec<_> = self
            .book()
            .rows
            .iter()
            .filter_map(|r| {
                let x = r[self.x_col].parse::<f64>().ok()?;
                let y = r[self.y_col].parse::<f64>().ok()?;
                (x.is_finite() && y.is_finite()).then_some([x, y])
            })
            .collect();
        points.sort_by(|a, b| a[0].total_cmp(&b[0]));
        points
    }

    fn sync_labels(&mut self) {
        self.style.x_label = column_label(&self.book().columns[self.x_col]);
        self.style.y_label = column_label(&self.book().columns[self.y_col]);
    }

    fn request(&self, action: &str) -> Value {
        let range = if action == "fit" || action == "statistics" {
            self.range
        } else {
            None
        };
        json!({"action":action, "workbook":self.book(), "x_col":self.x_col,
            "y_col":self.y_col, "error_col":self.error_col, "model":self.model,
            "x_min":range.map(|r|r[0]),"x_max":range.map(|r|r[1]),
            "style":self.style, "fit":self.fit})
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err("工程版本暂未支持。".into());
        }
        if self.books.is_empty() || self.books.len() > 16 || self.active >= self.books.len() {
            return Err("工程中的工作簿数量或索引有误。".into());
        }
        for book in &self.books {
            if book.name.chars().count() > 512
                || book.columns.is_empty()
                || book.columns.len() > 32
                || book.rows.len() > 50_000
            {
                return Err("工作表超出本原型的容量。".into());
            }
            if book.columns.iter().any(|c| {
                c.name.chars().count() > 512
                    || c.unit.chars().count() > 128
                    || c.comments.chars().count() > 4096
            }) || book.rows.iter().any(|r| {
                r.len() != book.columns.len() || r.iter().any(|c| c.chars().count() > 4096)
            }) {
                return Err("工作表的列结构或文本长度有误。".into());
            }
        }
        if [Some(self.x_col), Some(self.y_col), self.error_col]
            .into_iter()
            .flatten()
            .any(|i| i >= self.book().columns.len())
        {
            return Err("绘图数据列的索引有误。".into());
        }
        if !MODELS.iter().any(|(k, _)| *k == self.model) {
            return Err("工程中的拟合模型有误。".into());
        }
        if !["line", "scatter", "line_points"].contains(&self.style.kind.as_str())
            || !self.style.width.is_finite()
            || !(0.5..=5.0).contains(&self.style.width)
            || !self.style.marker_size.is_finite()
            || !(1.0..=10.0).contains(&self.style.marker_size)
            || [&self.style.title, &self.style.x_label, &self.style.y_label]
                .iter()
                .any(|s| s.chars().count() > 512)
        {
            return Err("工程的图形样式有误。".into());
        }
        if self
            .range
            .is_some_and(|r| !r[0].is_finite() || !r[1].is_finite() || r[0] >= r[1])
        {
            return Err("工程的分析区间有误。".into());
        }
        if let Some(fit) = &self.fit {
            if fit.points.len() > 5000
                || fit.residuals.len() > 50_000
                || fit.params.len() > 16
                || fit.n != fit.residuals.len()
                || !fit.rmse.is_finite()
                || fit
                    .points
                    .iter()
                    .chain(&fit.residuals)
                    .flatten()
                    .any(|n| !n.is_finite())
            {
                return Err("工程中的分析结果有误。".into());
            }
        }
        Ok(())
    }
}

fn app_directory() -> PathBuf {
    if cfg!(test) {
        return PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    }
    let exe = std::env::current_exe().unwrap_or_default();
    exe.parent().unwrap_or(Path::new(".")).to_path_buf()
}

fn atomic_save(path: &Path, project: &Project) -> Result<(), String> {
    project.validate()?;
    let bytes = serde_json::to_vec_pretty(project).map_err(|e| e.to_string())?;
    let temporary = path.with_file_name(format!(".plotlab-{}.tmp", std::process::id()));
    std::fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    Ok(())
}

fn load_project(path: &Path) -> Result<Project, String> {
    if std::fs::metadata(path).map_err(|e| e.to_string())?.len() > 64 * 1024 * 1024 {
        return Err("工程文件超过 64 MB。".into());
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let project: Project =
        serde_json::from_slice(&bytes).map_err(|e| format!("工程格式有误：{e}"))?;
    project.validate()?;
    Ok(project)
}

fn run_task(request: Value, cancel: Arc<AtomicBool>) -> Result<Value, String> {
    engine::dispatch(request, cancel)
}

struct Task {
    receiver: mpsc::Receiver<Result<Value, String>>,
    cancel: Arc<AtomicBool>,
    revision: u64,
    action: String,
}
impl Drop for Task {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub struct PlotLab {
    egui_ctx: egui::Context,
    window_title: String,
    plot_points: Vec<PlotPoint>,
    plot_key: Option<(usize, usize, usize, u64)>,
    project: Project,
    path: Option<PathBuf>,
    dirty: bool,
    undo: Vec<Project>,
    redo: Vec<Project>,
    revision: u64,
    task: Option<Task>,
    status: String,
    messages: Vec<String>,
    auto_fit: bool,
    needs_fit: bool,
    last_edit: Instant,
    last_save: Instant,
    edit_id: Option<egui::Id>,
    reset_plot: bool,
    show_graph: bool,
    show_report: bool,
    active_window: usize,
    tiled: bool,
    show_project: bool,
    show_objects: bool,
    show_apps: bool,
    fit_open: bool,
    format_open: bool,
    analysis_action: String,
    selected_col: usize,
    search: String,
    zoom_factor: f32,
    reveal_results: bool,
    show_residuals: bool,
    report_tab: usize,
    range_enabled: bool,
    range_start: String,
    range_end: String,
    help_open: bool,
    screenshot: Option<String>,
    screenshot_requested: bool,
    frames: u32,
}

impl PlotLab {
    pub fn new(cc: &eframe::CreationContext<'_>, screenshot: Option<String>, demo: bool) -> Self {
        configure_style(&cc.egui_ctx);
        let restore = if screenshot.is_none() && !demo {
            load_project(&app_directory().join("last-session.plab")).ok()
        } else {
            None
        };
        let project = restore.unwrap_or_else(|| {
            if demo {
                Project::demo("exponential")
            } else {
                Project::blank()
            }
        });
        let has_results = project.fit.is_some() || project.statistics.is_some();
        let range_enabled = project.range.is_some();
        let range = project.range.unwrap_or([0.0, 12.0]);
        let selected_col = project.y_col;
        let mut app = Self {
            egui_ctx: cc.egui_ctx.clone(),
            window_title: String::new(),
            plot_points: vec![],
            plot_key: None,
            project,
            path: None,
            dirty: false,
            undo: vec![],
            redo: vec![],
            revision: 0,
            task: None,
            status: "就绪".into(),
            messages: vec![],
            auto_fit: true,
            needs_fit: false,
            last_edit: Instant::now(),
            last_save: Instant::now(),
            edit_id: None,
            reset_plot: false,
            show_graph: has_results || demo,
            show_report: has_results || demo,
            active_window: 0,
            tiled: demo,
            show_project: true,
            show_objects: true,
            show_apps: true,
            fit_open: false,
            format_open: false,
            analysis_action: "fit".into(),
            selected_col,
            search: String::new(),
            zoom_factor: 1.0,
            reveal_results: false,
            show_residuals: true,
            report_tab: 0,
            range_enabled,
            range_start: range[0].to_string(),
            range_end: range[1].to_string(),
            help_open: false,
            screenshot,
            screenshot_requested: false,
            frames: 0,
        };
        if demo && app.project.fit.is_none() && app.project.points().len() >= 5 {
            app.start_analysis("fit");
        }
        app
    }

    fn log(&mut self, message: impl Into<String>) {
        self.status = message.into();
        self.messages.push(self.status.clone());
        if self.messages.len() > 24 {
            self.messages.remove(0);
        }
    }

    fn remember(&mut self) {
        self.remember_project(self.project.clone());
    }
    fn remember_project(&mut self, project: Project) {
        // ponytail: twenty full snapshots; use cell-level edits for very large projects.
        fn footprint(p: &Project) -> usize {
            p.books
                .iter()
                .flat_map(|b| &b.rows)
                .flat_map(|r| r)
                .map(|s| s.len() + std::mem::size_of::<String>())
                .sum()
        }
        let incoming = footprint(&project);
        while !self.undo.is_empty()
            && (self.undo.len() >= 20
                || incoming + self.undo.iter().map(footprint).sum::<usize>() > 64 * 1024 * 1024)
        {
            self.undo.remove(0);
        }
        self.undo.push(project);
        self.redo.clear();
    }

    fn data_changed(&mut self) {
        self.dirty = true;
        self.revision += 1;
        self.needs_fit = self.auto_fit
            && (self.needs_fit
                || self.project.fit.is_some()
                || self.task.as_ref().is_some_and(|t| t.action == "fit"));
        self.project.fit = None;
        self.project.statistics = None;
        self.last_edit = Instant::now();
        if self.task.is_some() {
            self.task = None;
            self.log("数据已更新，原分析任务已取消。");
        }
    }

    fn switch_book(&mut self, index: usize) {
        if index == self.project.active {
            return;
        }
        self.remember();
        self.data_changed();
        self.project.active = index;
        self.project.x_col = 0;
        self.project.y_col = usize::from(self.project.book().columns.len() > 1);
        self.project.error_col = None;
        self.project.sync_labels();
        self.needs_fit = false;
        self.reset_plot = true;
        self.project.range = None;
        self.range_enabled = false;
        self.selected_col = self.project.y_col;
    }

    fn start_task(&mut self, request: Value) {
        if self.task.is_some() {
            return;
        }
        let action = request["action"].as_str().unwrap_or("").to_string();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let (sender, receiver) = mpsc::channel();
        let ctx = self.egui_ctx.clone();
        std::thread::spawn(move || {
            let _ = sender.send(run_task(request, worker_cancel));
            ctx.request_repaint();
        });
        self.task = Some(Task {
            receiver,
            cancel,
            revision: self.revision,
            action,
        });
        self.log("正在处理，请稍候…");
    }

    fn start_analysis(&mut self, action: &str) {
        self.reveal_results = !self.needs_fit;
        self.project.range = if self.range_enabled {
            match (
                self.range_start.parse::<f64>(),
                self.range_end.parse::<f64>(),
            ) {
                (Ok(a), Ok(b)) if a.is_finite() && b.is_finite() && a < b => Some([a, b]),
                _ => {
                    self.log("请输入有效的 X 区间：起点小于终点。");
                    return;
                }
            }
        } else {
            None
        };
        let request = self.project.request(action);
        self.report_tab = usize::from(action == "statistics");
        self.needs_fit = false;
        self.start_task(request);
    }

    fn poll_task(&mut self) {
        let reply = self.task.as_ref().and_then(|t| t.receiver.try_recv().ok());
        if let Some(reply) = reply {
            let task = self.task.take().unwrap();
            if task.revision != self.revision {
                return;
            }
            match reply {
                Err(error) => self.log(error),
                Ok(result) => {
                    let applied: Result<(), String> = (|| {
                        match task.action.as_str() {
                            "fit" => {
                                let fit: FitResult =
                                    serde_json::from_value(result).map_err(|e| e.to_string())?;
                                self.log(format!(
                                    "拟合完成：{} 个有效点，RMSE = {:.5}",
                                    fit.n, fit.rmse
                                ));
                                self.project.fit = Some(fit);
                                self.show_graph = true;
                                self.show_report = true;
                                if self.reveal_results && !self.tiled {
                                    self.active_window = 2;
                                }
                                self.dirty = true;
                                self.report_tab = 0;
                            }
                            "statistics" => {
                                self.project.statistics = Some(
                                    serde_json::from_value(result).map_err(|e| e.to_string())?,
                                );
                                self.dirty = true;
                                self.report_tab = 1;
                                self.show_report = true;
                                if self.reveal_results && !self.tiled {
                                    self.active_window = 2;
                                }
                                self.log("描述统计已完成。");
                            }
                            "import" => {
                                let mut book: Workbook =
                                    serde_json::from_value(result).map_err(|e| e.to_string())?;
                                if self.project.books.len() >= 16 {
                                    return Err("本原型支持最多 16 个工作簿。".into());
                                }
                                let base = book.name.clone();
                                let mut n = 2;
                                while self.project.books.iter().any(|b| b.name == book.name) {
                                    book.name = format!("{base}_{n}");
                                    n += 1;
                                }
                                self.remember();
                                self.data_changed();
                                self.project.books.push(book);
                                self.project.active = self.project.books.len() - 1;
                                self.project.x_col = 0;
                                self.project.y_col =
                                    usize::from(self.project.book().columns.len() > 1);
                                self.project.error_col = None;
                                self.project.sync_labels();
                                self.project.style.title = self.project.book().name.clone();
                                self.needs_fit = false;
                                self.reset_plot = true;
                                self.active_window = 0;
                                self.selected_col = self.project.y_col;
                                self.tiled = false;
                                self.range_enabled = false;
                                self.project.range = None;
                                self.log("导入完成");
                            }
                            _ => self.log(format!(
                                "已导出：{}",
                                result["path"].as_str().unwrap_or("文件")
                            )),
                        }
                        Ok(())
                    })();
                    if let Err(e) = applied {
                        self.log(e);
                    }
                }
            }
        }
    }

    fn save(&mut self, save_as: bool) {
        let path = if !save_as { self.path.clone() } else { None };
        let path = path.or_else(|| {
            rfd::FileDialog::new()
                .add_filter("PlotLab 工程", &["plab"])
                .set_file_name("实验分析.plab")
                .save_file()
        });
        if let Some(mut path) = path {
            if path.extension().is_none() {
                path.set_extension("plab");
            }
            if path
                .extension()
                .is_none_or(|e| !e.eq_ignore_ascii_case("plab"))
            {
                self.log("请使用 .plab 后缀保存工程。");
                return;
            }
            match atomic_save(&path, &self.project) {
                Ok(()) => {
                    self.path = Some(path);
                    self.dirty = false;
                    self.log("工程已保存。");
                }
                Err(error) => self.log(format!("保存失败：{error}")),
            }
        }
    }

    fn backup(&mut self) {
        if self.dirty {
            if let Err(e) = atomic_save(&app_directory().join("last-session.plab"), &self.project) {
                self.log(format!("自动恢复文件保存失败：{e}"));
            }
            self.last_save = Instant::now();
        }
    }

    fn open(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("PlotLab 工程", &["plab"])
            .pick_file()
        {
            match load_project(&path) {
                Ok(project) => {
                    self.backup();
                    self.remember();
                    self.data_changed();
                    self.project = project;
                    self.path = Some(path);
                    self.dirty = false;
                    self.needs_fit = false;
                    self.range_enabled = self.project.range.is_some();
                    if let Some(range) = self.project.range {
                        self.range_start = range[0].to_string();
                        self.range_end = range[1].to_string();
                    }
                    self.reset_plot = true;
                    self.log("工程已打开。");
                    self.active_window = 0;
                    self.selected_col = self.project.y_col;
                    self.tiled = false;
                    self.show_graph = self.project.fit.is_some();
                    self.show_report =
                        self.project.fit.is_some() || self.project.statistics.is_some();
                }
                Err(error) => self.log(error),
            }
        }
    }

    fn import(&mut self) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter("数据文件", &["csv", "tsv", "txt", "dat", "xlsx"])
            .pick_file()
        {
            self.start_task(json!({"action":"import","path":path}));
        }
    }

    fn export(&mut self, csv: bool) {
        let dialog = rfd::FileDialog::new();
        let path = if csv {
            dialog
                .add_filter("CSV", &["csv"])
                .set_file_name("数据.csv")
                .save_file()
        } else {
            dialog
                .add_filter("矢量图 SVG", &["svg"])
                .add_filter("PDF", &["pdf"])
                .add_filter("PNG", &["png"])
                .set_file_name("Graph1.svg")
                .save_file()
        };
        if let Some(mut path) = path {
            if path.extension().is_none() {
                path.set_extension(if csv { "csv" } else { "svg" });
            }
            let mut request = self
                .project
                .request(if csv { "export_csv" } else { "export" });
            request["path"] = json!(path);
            self.start_task(request);
        }
    }

    fn demo(&mut self, kind: &str) {
        self.backup();
        self.remember();
        self.data_changed();
        self.project = Project::demo(kind);
        self.path = None;
        self.needs_fit = false;
        self.range_enabled = false;
        self.reset_plot = true;
        self.start_analysis("fit");
        self.tiled = true;
        self.active_window = 0;
        self.show_graph = true;
        self.show_report = true;
        self.selected_col = 1;
    }

    fn new_book(&mut self) {
        if self.project.books.len() >= 16 {
            self.log("本原型支持最多 16 个工作簿。");
            return;
        }
        self.remember();
        self.data_changed();
        let mut n = self.project.books.len() + 1;
        while self
            .project
            .books
            .iter()
            .any(|b| b.name == format!("Book{n}"))
        {
            n += 1;
        }
        self.project.books.push(Workbook {
            name: format!("Book{n}"),
            columns: vec![
                Column {
                    name: String::new(),
                    unit: String::new(),
                    comments: String::new(),
                },
                Column {
                    name: String::new(),
                    unit: String::new(),
                    comments: String::new(),
                },
            ],
            rows: vec![vec![String::new(); 2]; 32],
        });
        self.project.active = self.project.books.len() - 1;
        self.project.x_col = 0;
        self.project.y_col = 1;
        self.project.error_col = None;
        self.project.sync_labels();
        self.project.style.title = self.project.book().name.clone();
        self.needs_fit = false;
        self.reset_plot = true;
        self.project.range = None;
        self.range_enabled = false;
        self.selected_col = 1;
        self.activate(0);
    }

    fn undo(&mut self, redo: bool) {
        let project = if redo {
            self.redo.pop()
        } else {
            self.undo.pop()
        };
        if let Some(project) = project {
            if redo {
                self.undo.push(self.project.clone());
            } else {
                self.redo.push(self.project.clone());
            }
            self.task = None;
            self.project = project;
            self.selected_col = self.selected_col.min(self.project.book().columns.len() - 1);
            self.dirty = true;
            self.revision += 1;
            self.needs_fit = false;
            self.reset_plot = true;
            self.log(if redo { "已重做。" } else { "已撤销。" });
        }
    }
}

fn configure_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    for (name, path) in [
        ("cjk", "C:/Windows/Fonts/msyh.ttc"),
        ("arial", "C:/Windows/Fonts/arial.ttf"),
    ] {
        if let Ok(data) = std::fs::read(path) {
            fonts
                .font_data
                .insert(name.into(), egui::FontData::from_owned(data).into());
            fonts
                .families
                .get_mut(&egui::FontFamily::Proportional)
                .unwrap()
                .insert(0, name.into());
            if name == "cjk" {
                fonts
                    .families
                    .get_mut(&egui::FontFamily::Monospace)
                    .unwrap()
                    .push(name.into());
            }
        }
    }
    ctx.set_fonts(fonts);
    let mut style = egui::Style::default();
    style.visuals = egui::Visuals::light();
    style.visuals.panel_fill = TOOLBAR;
    style.visuals.window_fill = Color32::from_rgb(246, 246, 248);
    style.visuals.override_text_color = Some(Color32::from_rgb(23, 23, 27));
    style.visuals.selection.bg_fill = BLUE;
    style.visuals.selection.stroke = egui::Stroke::new(1.0_f32, Color32::WHITE);
    style.visuals.widgets.noninteractive.bg_stroke =
        egui::Stroke::new(1.0_f32, Color32::from_rgb(181, 184, 191));
    style.visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(241, 241, 245);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(217, 233, 249);
    style.visuals.widgets.active.weak_bg_fill = Color32::from_rgb(193, 219, 246);
    style.visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::ZERO;
    style.visuals.widgets.inactive.corner_radius = egui::CornerRadius::ZERO;
    style.visuals.widgets.hovered.corner_radius = egui::CornerRadius::ZERO;
    style.visuals.widgets.active.corner_radius = egui::CornerRadius::ZERO;
    style.spacing.item_spacing = Vec2::new(4.0, 3.0);
    style.spacing.button_padding = Vec2::new(5.0, 3.0);
    for kind in [egui::TextStyle::Body, egui::TextStyle::Button] {
        style
            .text_styles
            .insert(kind, egui::FontId::proportional(13.5));
    }
    style
        .text_styles
        .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
    style
        .text_styles
        .insert(egui::TextStyle::Monospace, egui::FontId::proportional(13.0));
    ctx.set_style(style);
}

fn paint_icon(p: &egui::Painter, rect: egui::Rect, kind: &str, enabled: bool) {
    let r = rect.shrink(3.0);
    let point = |x: f32, y: f32| egui::pos2(r.left() + r.width() * x, r.top() + r.height() * y);
    let ink = if enabled {
        Color32::from_rgb(38, 39, 43)
    } else {
        Color32::from_gray(170)
    };
    let blue = if enabled {
        Color32::from_rgb(41, 103, 177)
    } else {
        Color32::from_gray(165)
    };
    let red = if enabled {
        Color32::from_rgb(206, 54, 52)
    } else {
        Color32::from_gray(170)
    };
    let green = if enabled {
        Color32::from_rgb(36, 147, 73)
    } else {
        Color32::from_gray(170)
    };
    let stroke = egui::Stroke::new(1.2_f32, ink);
    let segment = |a: (f32, f32), b: (f32, f32), color| {
        p.line_segment(
            [point(a.0, a.1), point(b.0, b.1)],
            egui::Stroke::new(1.5_f32, color),
        );
    };
    match kind {
        "new" => {
            p.rect(
                egui::Rect::from_min_max(point(0.2, 0.04), point(0.8, 0.96)),
                0.0,
                Color32::WHITE,
                stroke,
                egui::StrokeKind::Inside,
            );
            segment((0.6, 0.05), (0.79, 0.26), ink);
        }
        "open" | "folder" => {
            p.add(egui::Shape::convex_polygon(
                vec![
                    point(0.04, 0.3),
                    point(0.38, 0.3),
                    point(0.44, 0.14),
                    point(0.91, 0.14),
                    point(0.96, 0.84),
                    point(0.04, 0.84),
                ],
                Color32::from_rgb(238, 196, 75),
                stroke,
            ));
            segment((0.07, 0.38), (0.93, 0.38), Color32::from_rgb(171, 133, 36));
        }
        "save" => {
            p.rect(
                egui::Rect::from_min_max(point(0.07, 0.03), point(0.91, 0.96)),
                0.0,
                blue,
                stroke,
                egui::StrokeKind::Inside,
            );
            p.rect_filled(
                egui::Rect::from_min_max(point(0.25, 0.03), point(0.7, 0.4)),
                0.0,
                Color32::WHITE,
            );
            p.rect_filled(
                egui::Rect::from_min_max(point(0.2, 0.58), point(0.78, 0.92)),
                0.0,
                Color32::from_rgb(224, 230, 244),
            );
        }
        "book" | "import" | "add_col" | "add_row" => {
            p.rect(
                egui::Rect::from_min_max(point(0.07, 0.12), point(0.9, 0.94)),
                0.0,
                Color32::WHITE,
                stroke,
                egui::StrokeKind::Inside,
            );
            p.rect_filled(
                egui::Rect::from_min_max(point(0.08, 0.12), point(0.89, 0.3)),
                0.0,
                if kind == "book" { green } else { blue },
            );
            for x in [0.35, 0.63] {
                segment((x, 0.3), (x, 0.94), ink);
            }
            for y in [0.52, 0.74] {
                segment((0.07, y), (0.9, y), ink);
            }
            if kind == "import" {
                segment((0.75, 0.0), (0.75, 0.6), green);
                p.add(egui::Shape::convex_polygon(
                    vec![point(0.55, 0.45), point(0.95, 0.45), point(0.75, 0.72)],
                    green,
                    egui::Stroke::NONE,
                ));
            }
            if kind.starts_with("add") {
                segment((0.62, 0.64), (1.0, 0.64), green);
                segment((0.82, 0.45), (0.82, 0.84), green);
            }
        }
        "undo" | "redo" => {
            let flip = if kind == "undo" { 1.0 } else { -1.0 };
            let q = |x: f32, y: f32| point(0.5 + (x - 0.5) * flip, y);
            p.add(egui::Shape::line(
                vec![
                    q(0.8, 0.86),
                    q(0.93, 0.55),
                    q(0.8, 0.22),
                    q(0.52, 0.12),
                    q(0.16, 0.31),
                ],
                egui::Stroke::new(2.0_f32, blue),
            ));
            p.add(egui::Shape::convex_polygon(
                vec![q(0.01, 0.31), q(0.35, 0.12), q(0.35, 0.49)],
                blue,
                egui::Stroke::NONE,
            ));
        }
        "line" | "scatter" | "line_points" | "graph" | "fit" => {
            segment((0.05, 0.08), (0.05, 0.92), ink);
            segment((0.05, 0.92), (0.97, 0.92), ink);
            let pts = [
                point(0.2, 0.76),
                point(0.44, 0.6),
                point(0.67, 0.67),
                point(0.9, 0.2),
            ];
            if kind != "scatter" {
                p.add(egui::Shape::line(
                    pts.to_vec(),
                    egui::Stroke::new(1.8_f32, if kind == "fit" { red } else { blue }),
                ));
            }
            if kind != "line" {
                for pos in pts {
                    p.circle_filled(pos, 1.7, blue);
                }
            }
        }
        "export" => {
            segment((0.1, 0.42), (0.1, 0.93), ink);
            segment((0.1, 0.93), (0.9, 0.93), ink);
            segment((0.9, 0.93), (0.9, 0.42), ink);
            segment((0.5, 0.76), (0.5, 0.07), green);
            p.add(egui::Shape::convex_polygon(
                vec![point(0.25, 0.3), point(0.75, 0.3), point(0.5, 0.0)],
                green,
                egui::Stroke::NONE,
            ));
        }
        "zoom_in" | "zoom_out" | "search" => {
            p.circle_stroke(point(0.4, 0.4), r.width() * 0.31, stroke);
            segment((0.65, 0.65), (0.97, 0.97), blue);
            if kind != "search" {
                segment((0.2, 0.4), (0.6, 0.4), ink);
                if kind == "zoom_in" {
                    segment((0.4, 0.2), (0.4, 0.6), ink);
                }
            }
        }
        "bounds" => {
            p.rect_stroke(
                egui::Rect::from_min_max(point(0.1, 0.1), point(0.9, 0.9)),
                0.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            segment((0.1, 0.5), (0.9, 0.5), blue);
            segment((0.5, 0.1), (0.5, 0.9), blue);
        }
        "statistics" => {
            p.text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                "Σ",
                egui::FontId::proportional(23.0),
                ink,
            );
        }
        "format" => {
            p.text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                "T",
                egui::FontId::proportional(20.0),
                ink,
            );
            segment((0.12, 0.94), (0.94, 0.94), blue);
        }
        "grid" => {
            for t in [0.1, 0.38, 0.66, 0.94] {
                segment((t, 0.1), (t, 0.94), ink);
                segment((0.1, t), (0.94, t), ink);
            }
        }
        "legend" => {
            for (y, c) in [(0.22, blue), (0.5, red), (0.78, green)] {
                p.circle_filled(point(0.12, y), 1.6, c);
                segment((0.32, y), (0.92, y), ink);
            }
        }
        "tile" => {
            for (x, y) in [(0.05, 0.05), (0.53, 0.05), (0.05, 0.53), (0.53, 0.53)] {
                p.rect_stroke(
                    egui::Rect::from_min_max(point(x, y), point(x + 0.41, y + 0.41)),
                    0.0,
                    stroke,
                    egui::StrokeKind::Inside,
                );
            }
        }
        "stop" => {
            p.rect_filled(
                egui::Rect::from_min_max(point(0.2, 0.2), point(0.8, 0.8)),
                0.0,
                red,
            );
        }
        "pointer" => {
            p.add(egui::Shape::convex_polygon(
                vec![
                    point(0.13, 0.02),
                    point(0.87, 0.59),
                    point(0.53, 0.58),
                    point(0.6, 0.93),
                    point(0.43, 0.97),
                    point(0.33, 0.6),
                    point(0.12, 0.78),
                ],
                Color32::WHITE,
                stroke,
            ));
        }
        _ => {}
    }
}

fn icon_button(
    ui: &mut egui::Ui,
    kind: &str,
    tip: &str,
    enabled: bool,
    selected: bool,
) -> egui::Response {
    let response = ui.add_enabled(
        enabled,
        egui::Button::new("")
            .min_size(Vec2::splat(25.0))
            .selected(selected)
            .frame(selected),
    );
    paint_icon(ui.painter(), response.rect, kind, enabled);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, tip));
    response.on_hover_text(tip)
}

fn small_icon(ui: &mut egui::Ui, kind: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), egui::Sense::hover());
    paint_icon(ui.painter(), rect, kind, true);
}

fn panel_title(ui: &mut egui::Ui, title: &str, closable: bool) -> bool {
    egui::Frame::new()
        .fill(HEADER)
        .inner_margin(egui::Margin::symmetric(5, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(title);
                if closable {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.small_button("×").clicked()
                    })
                    .inner
                } else {
                    false
                }
            })
            .inner
        })
        .inner
}

fn frame() -> egui::Frame {
    egui::Frame::new()
        .fill(Color32::WHITE)
        .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(181, 184, 192)))
}

impl PlotLab {
    fn activate(&mut self, window: usize) {
        self.active_window = window;
        self.tiled = false;
        if window == 1 {
            self.show_graph = true;
        }
        if window == 2 {
            self.show_report = true;
        }
    }
    fn plot_kind(&mut self, kind: &str) {
        self.remember();
        self.project.style.kind = kind.into();
        if self.selected_col < self.project.book().columns.len()
            && self.selected_col != self.project.x_col
            && self.selected_col != self.project.y_col
        {
            self.data_changed();
            self.project.y_col = self.selected_col;
            self.project.sync_labels();
        }
        self.dirty = true;
        self.reset_plot = true;
        self.activate(1);
    }
    fn open_analysis(&mut self, action: &str, model: Option<&str>) {
        self.analysis_action = action.into();
        self.fit_open = true;
        if let Some(model) = model {
            if self.project.model != model {
                self.remember();
                self.data_changed();
                self.project.model = model.into();
                self.needs_fit = false;
            }
        }
    }
    fn add_column(&mut self) {
        if self.project.book().columns.len() >= 32 {
            return;
        }
        self.remember();
        self.data_changed();
        let book = &mut self.project.books[self.project.active];
        book.columns.push(Column {
            name: String::new(),
            unit: String::new(),
            comments: String::new(),
        });
        for row in &mut book.rows {
            row.push(String::new());
        }
        self.activate(0);
    }
    fn add_row(&mut self) {
        if self.project.book().rows.len() >= 50_000 {
            return;
        }
        self.remember();
        self.data_changed();
        let n = self.project.book().columns.len();
        self.project.books[self.project.active]
            .rows
            .push(vec![String::new(); n]);
        self.activate(0);
    }
    fn set_role(&mut self, col: usize, role: &str) {
        self.remember();
        self.data_changed();
        match role {
            "X" => self.project.x_col = col,
            "YErr" => self.project.error_col = Some(col),
            _ => self.project.y_col = col,
        };
        self.project.sync_labels();
        self.selected_col = col;
        self.reset_plot = true;
    }
    fn tool(&mut self, key: &str) {
        match key {
            "new" => self.new_book(),
            "open" => self.open(),
            "import" => self.import(),
            "save" => self.save(false),
            "undo" => self.undo(false),
            "redo" => self.undo(true),
            "line" | "scatter" | "line_points" => self.plot_kind(key),
            "fit" => self.open_analysis("fit", None),
            "statistics" => self.open_analysis("statistics", None),
            "export" => self.export(false),
            "format" => self.format_open = true,
            "add_col" => self.add_column(),
            "add_row" => self.add_row(),
            "bounds" => {
                self.reset_plot = true;
                self.activate(1);
            }
            "zoom_in" => {
                self.zoom_factor = 1.4;
                self.activate(1);
            }
            "zoom_out" => {
                self.zoom_factor = 1.0 / 1.4;
                self.activate(1);
            }
            "grid" => {
                self.project.style.grid = !self.project.style.grid;
                self.dirty = true;
            }
            "legend" => {
                self.project.style.legend = !self.project.style.legend;
                self.dirty = true;
            }
            "tile" => {
                self.tiled = true;
            }
            "pointer" => self.activate(0),
            "stop" => {
                self.task = None;
                self.log("已停止");
            }
            _ => {}
        }
    }

    fn toolbar(&mut self, ctx: &egui::Context) {
        let busy = self.task.is_some();
        egui::TopBottomPanel::top("menus")
            .frame(
                egui::Frame::new()
                    .fill(TOOLBAR)
                    .inner_margin(egui::Margin::symmetric(4, 1)),
            )
            .show(ctx, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    small_icon(ui, "book");
                    ui.menu_button("文件", |ui| {
                        for (key, label) in [
                            ("new", "新建工作簿    Ctrl+N"),
                            ("open", "打开工程…    Ctrl+O"),
                            ("save", "保存工程    Ctrl+S"),
                        ] {
                            if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                                self.tool(key);
                                ui.close();
                            }
                        }
                        if ui.button("工程另存为…").clicked() {
                            self.save(true);
                            ui.close();
                        }
                        ui.separator();
                        if ui
                            .add_enabled(!busy && self.show_graph, egui::Button::new("导出图形…"))
                            .clicked()
                        {
                            self.export(false);
                            ui.close();
                        }
                    });
                    ui.menu_button("编辑", |ui| {
                        if ui
                            .add_enabled(!self.undo.is_empty(), egui::Button::new("撤销    Ctrl+Z"))
                            .clicked()
                        {
                            self.undo(false);
                            ui.close();
                        }
                        if ui
                            .add_enabled(!self.redo.is_empty(), egui::Button::new("重做    Ctrl+Y"))
                            .clicked()
                        {
                            self.undo(true);
                            ui.close();
                        }
                    });
                    ui.menu_button("查看", |ui| {
                        ui.checkbox(&mut self.show_project, "项目管理器");
                        ui.checkbox(&mut self.show_objects, "对象管理器");
                        ui.checkbox(&mut self.show_apps, "Apps");
                        ui.checkbox(&mut self.show_residuals, "残差图");
                        if ui.button("消息日志").clicked() {
                            self.report_tab = 2;
                            self.activate(2);
                            ui.close();
                        }
                    });
                    ui.menu_button("数据", |ui| {
                        if ui
                            .add_enabled(!busy, egui::Button::new("从文件导入…"))
                            .clicked()
                        {
                            self.import();
                            ui.close();
                        }
                        if ui
                            .add_enabled(!busy, egui::Button::new("导出数据…"))
                            .clicked()
                        {
                            self.export(true);
                            ui.close();
                        }
                    });
                    ui.menu_button("绘图", |ui| {
                        for (key, label) in [
                            ("line", "折线图"),
                            ("scatter", "散点图"),
                            ("line_points", "点线图"),
                        ] {
                            if ui.button(label).clicked() {
                                self.plot_kind(key);
                                ui.close();
                            }
                        }
                    });
                    ui.menu_button("列", |ui| {
                        ui.menu_button("设置为", |ui| {
                            for role in ["X", "Y", "YErr"] {
                                if ui.button(role).clicked() {
                                    self.set_role(self.selected_col, role);
                                    ui.close();
                                }
                            }
                        });
                        if ui.button("添加列").clicked() {
                            self.add_column();
                            ui.close();
                        }
                    });
                    ui.menu_button("工作表", |ui| {
                        if ui.button("添加行").clicked() {
                            self.add_row();
                            ui.close();
                        }
                        if ui.button("新建工作簿").clicked() {
                            self.new_book();
                            ui.close();
                        }
                    });
                    ui.menu_button("分析", |ui| {
                        ui.menu_button("拟合", |ui| {
                            for (key, label) in MODELS {
                                if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                                    self.open_analysis("fit", Some(key));
                                    ui.close();
                                }
                            }
                        });
                    });
                    ui.menu_button("统计", |ui| {
                        if ui
                            .add_enabled(!busy, egui::Button::new("描述统计…"))
                            .clicked()
                        {
                            self.open_analysis("statistics", None);
                            ui.close();
                        }
                    });
                    ui.menu_button("格式", |ui| {
                        if ui.button("绘图细节…").clicked() {
                            self.format_open = true;
                            ui.close();
                        }
                    });
                    ui.menu_button("工具", |ui| {
                        ui.checkbox(&mut self.auto_fit, "自动重算");
                        if ui.button("调整刻度以显示全部    Ctrl+R").clicked() {
                            self.reset_plot = true;
                            ui.close();
                        }
                    });
                    ui.menu_button("窗口", |ui| {
                        if ui.button("平铺").clicked() {
                            self.tiled = true;
                            ui.close();
                        }
                        ui.separator();
                        let names: Vec<_> =
                            self.project.books.iter().map(|b| b.name.clone()).collect();
                        for (i, name) in names.iter().enumerate() {
                            if ui.button(name).clicked() {
                                self.switch_book(i);
                                self.activate(0);
                                ui.close();
                            }
                        }
                        if self.show_graph && ui.button("Graph1").clicked() {
                            self.activate(1);
                            ui.close();
                        }
                        if self.show_report && ui.button("FitReport1").clicked() {
                            self.activate(2);
                            ui.close();
                        }
                    });
                    ui.menu_button("帮助", |ui| {
                        ui.menu_button("示例项目", |ui| {
                            for (key, label) in [
                                ("linear", "校准曲线"),
                                ("exponential", "指数衰减"),
                                ("gaussian", "单峰光谱"),
                            ] {
                                if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                                    self.demo(key);
                                    ui.close();
                                }
                            }
                        });
                        if ui.button("使用说明").clicked() {
                            self.help_open = true;
                            ui.close();
                        }
                    });
                });
                ui.horizontal(|ui| {
                    small_icon(ui, "search");
                    let response = ui.add_sized(
                        [275.0, 22.0],
                        egui::TextEdit::singleline(&mut self.search)
                            .hint_text("搜索")
                            .char_limit(80),
                    );
                    if response.has_focus() && !self.search.is_empty() {
                        let query = self.search.to_lowercase();
                        let choices = [
                            ("导入数据", "import"),
                            ("线性拟合", "linear"),
                            ("非线性拟合", "fit"),
                            ("描述统计", "statistics"),
                            ("绘图细节", "format"),
                            ("导出图形", "export"),
                        ];
                        egui::Area::new(egui::Id::new("search_results"))
                            .order(egui::Order::Foreground)
                            .fixed_pos(response.rect.left_bottom())
                            .show(ctx, |ui| {
                                egui::Frame::popup(ui.style()).show(ui, |ui| {
                                    ui.set_min_width(270.0);
                                    for (label, key) in choices {
                                        if label.contains(&query) || key.contains(&query) {
                                            if ui.button(label).clicked() {
                                                if key == "linear" {
                                                    self.open_analysis("fit", Some("linear"));
                                                } else {
                                                    self.tool(key);
                                                }
                                                self.search.clear();
                                            }
                                        }
                                    }
                                });
                            });
                    }
                });
                ui.separator();
                ui.horizontal(|ui| {
                    for (key, tip) in [
                        ("new", "新建工作簿"),
                        ("open", "打开工程"),
                        ("import", "导入数据"),
                        ("save", "保存"),
                        ("export", "导出图形"),
                    ] {
                        if icon_button(ui, key, tip, !busy, false).clicked() {
                            self.tool(key);
                        }
                    }
                    ui.separator();
                    if icon_button(ui, "undo", "撤销", !self.undo.is_empty(), false).clicked() {
                        self.undo(false);
                    }
                    if icon_button(ui, "redo", "重做", !self.redo.is_empty(), false).clicked() {
                        self.undo(true);
                    }
                    ui.separator();
                    for (key, tip) in [
                        ("add_col", "添加列"),
                        ("add_row", "添加行"),
                        ("book", "工作簿"),
                        ("graph", "图形窗口"),
                        ("tile", "平铺窗口"),
                    ] {
                        if icon_button(ui, key, tip, !busy, false).clicked() {
                            match key {
                                "book" => self.activate(0),
                                "graph" => self.activate(1),
                                _ => self.tool(key),
                            }
                        }
                    }
                    ui.separator();
                    for (key, tip) in [
                        ("zoom_in", "放大"),
                        ("zoom_out", "缩小"),
                        ("bounds", "重新调整刻度"),
                        ("statistics", "描述统计"),
                        ("fit", "曲线拟合"),
                    ] {
                        if icon_button(ui, key, tip, !busy, false).clicked() {
                            self.tool(key);
                        }
                    }
                    if busy {
                        ui.separator();
                        ui.spinner();
                        if icon_button(ui, "stop", "停止", true, false).clicked() {
                            self.tool("stop");
                        }
                    }
                });
                ui.horizontal(|ui| {
                    for (key, tip) in [
                        ("format", "绘图细节"),
                        ("grid", "网格线"),
                        ("legend", "图例"),
                    ] {
                        let selected = key == "grid" && self.project.style.grid
                            || key == "legend" && self.project.style.legend;
                        if icon_button(ui, key, tip, true, selected).clicked() {
                            self.tool(key);
                        }
                    }
                    ui.separator();
                    let old = self.project.style.clone();
                    ui.color_edit_button_srgb(&mut self.project.style.color);
                    ui.add(
                        egui::DragValue::new(&mut self.project.style.width)
                            .range(0.5..=5.0)
                            .speed(0.1)
                            .prefix("线宽 "),
                    );
                    ui.add(
                        egui::DragValue::new(&mut self.project.style.marker_size)
                            .range(1.0..=10.0)
                            .speed(0.2)
                            .prefix("符号 "),
                    );
                    if old.color != self.project.style.color
                        || old.width != self.project.style.width
                        || old.marker_size != self.project.style.marker_size
                    {
                        self.dirty = true;
                    }
                });
            });
    }

    fn project_tree(&mut self, ctx: &egui::Context) {
        if !self.show_project {
            return;
        }
        egui::SidePanel::left("project")
            .default_width(345.0)
            .width_range(250.0..=420.0)
            .frame(egui::Frame::new().fill(Color32::WHITE).inner_margin(2))
            .resizable(true)
            .show(ctx, |ui| {
                if panel_title(ui, "项目管理器", true) {
                    self.show_project = false;
                }
                let name = self
                    .path
                    .as_ref()
                    .and_then(|p| p.file_stem())
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "UNTITLED".into());
                let tree_height = 140.0;
                ui.allocate_ui_with_layout(
                    Vec2::new(ui.available_width(), tree_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        egui::CollapsingHeader::new(name)
                            .default_open(true)
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    small_icon(ui, "folder");
                                    if ui.selectable_label(true, "Folder1").clicked() {
                                        self.activate(0);
                                    }
                                });
                            });
                    },
                );
                ui.separator();
                ui.horizontal(|ui| {
                    ui.add_sized([130.0, 21.0], egui::Label::new("名称"));
                    ui.add_sized([55.0, 21.0], egui::Label::new("大小"));
                    ui.label("备注");
                });
                ui.separator();
                let books: Vec<_> = self
                    .project
                    .books
                    .iter()
                    .enumerate()
                    .map(|(i, b)| {
                        (
                            i,
                            b.name.clone(),
                            b.rows.iter().flatten().map(|s| s.len() + 2).sum::<usize>(),
                        )
                    })
                    .collect();
                for (i, name, bytes) in books {
                    ui.horizontal(|ui| {
                        small_icon(ui, "book");
                        if ui
                            .add_sized(
                                [112.0, 23.0],
                                egui::Button::new(&name)
                                    .selected(self.active_window == 0 && self.project.active == i)
                                    .frame(false),
                            )
                            .clicked()
                        {
                            self.switch_book(i);
                            self.activate(0);
                        }
                        ui.label(format!("{}KB", (bytes + 1023) / 1024));
                    });
                }
                if self.show_graph {
                    ui.horizontal(|ui| {
                        small_icon(ui, "graph");
                        if ui
                            .selectable_label(self.active_window == 1, "Graph1")
                            .clicked()
                        {
                            self.activate(1);
                        }
                    });
                }
                if self.show_report {
                    ui.horizontal(|ui| {
                        small_icon(ui, "book");
                        if ui
                            .selectable_label(self.active_window == 2, "FitReport1")
                            .clicked()
                        {
                            self.activate(2);
                        }
                    });
                }
            });
    }

    fn object_manager(&mut self, ctx: &egui::Context) {
        if !self.show_objects {
            return;
        }
        egui::SidePanel::right("objects")
            .default_width(207.0)
            .width_range(185.0..=270.0)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(249, 249, 250))
                    .inner_margin(2),
            )
            .show(ctx, |ui| {
                if panel_title(ui, "对象管理器", true) {
                    self.show_objects = false;
                }
                ui.allocate_ui_with_layout(
                    Vec2::new(ui.available_width(), 132.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        egui::CollapsingHeader::new(self.project.book().name.clone())
                            .default_open(true)
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    small_icon(ui, "book");
                                    if ui
                                        .selectable_label(self.active_window == 0, "Sheet1")
                                        .clicked()
                                    {
                                        self.activate(0);
                                    }
                                });
                            });
                        if self.show_graph {
                            egui::CollapsingHeader::new("Graph1")
                                .default_open(true)
                                .show(ui, |ui| {
                                    if ui
                                        .selectable_label(self.active_window == 1, "Layer1")
                                        .clicked()
                                    {
                                        self.activate(1);
                                    }
                                    let name =
                                        &self.project.book().columns[self.project.y_col].name;
                                    if !name.is_empty() {
                                        ui.label(name);
                                    }
                                });
                        }
                    },
                );
                if !self.show_apps {
                    return;
                }
                ui.separator();
                if panel_title(ui, "Apps", true) {
                    self.show_apps = false;
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (key, label) in [
                        ("linear", "Simple Fit"),
                        ("fit", "Nonlinear Fit"),
                        ("statistics", "Statistics"),
                        ("export", "Export Graph"),
                    ] {
                        ui.vertical_centered(|ui| {
                            let response = ui.add_enabled(
                                self.task.is_none(),
                                egui::Button::new("")
                                    .min_size(Vec2::splat(45.0))
                                    .frame(false),
                            );
                            paint_icon(
                                ui.painter(),
                                response.rect.shrink(3.0),
                                if key == "linear" { "fit" } else { key },
                                self.task.is_none(),
                            );
                            if response.on_hover_text(label).clicked() {
                                if key == "linear" {
                                    self.open_analysis("fit", Some("linear"));
                                } else {
                                    self.tool(key);
                                }
                            }
                            ui.label(label);
                            ui.add_space(14.0);
                        });
                    }
                });
            });
    }

    fn worksheet(&mut self, ui: &mut egui::Ui, height: f32) {
        let row_height = 22.0;
        let label_width = 109.0;
        let cell_width = 91.0;
        let rows = self.project.book().rows.len();
        let cols = self.project.book().columns.len();
        let mut change: Option<(egui::Id, usize, usize, String, u8)> = None;
        let mut finished = false;
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        egui::ScrollArea::horizontal()
            .id_salt("horizontal_table")
            .auto_shrink([false, true])
            .max_height((height - 24.0).max(150.0))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                ui.horizontal(|ui| {
                    cell_label(ui, "", label_width, GRAY);
                    for c in 0..cols {
                        let role = if c == self.project.x_col {
                            "X"
                        } else if Some(c) == self.project.error_col {
                            "YErr"
                        } else {
                            "Y"
                        };
                        let response = cell_label(
                            ui,
                            &format!("{}({role})", column_code(c)),
                            cell_width,
                            if c == self.selected_col {
                                Color32::from_rgb(208, 221, 239)
                            } else {
                                GRAY
                            },
                        );
                        if response.clicked() {
                            self.selected_col = c;
                        }
                        response.context_menu(|ui| {
                            for role in ["X", "Y", "YErr"] {
                                if ui.button(format!("设置为 {role}")).clicked() {
                                    self.set_role(c, role);
                                    ui.close();
                                }
                            }
                        });
                    }
                });
                for metadata in 0..3 {
                    ui.horizontal(|ui| {
                        cell_label(
                            ui,
                            ["Long Name", "Units", "Comments"][metadata],
                            label_width,
                            GRAY,
                        );
                        for c in 0..cols {
                            let column = &mut self.project.books[self.project.active].columns[c];
                            let value = match metadata {
                                0 => &mut column.name,
                                1 => &mut column.unit,
                                _ => &mut column.comments,
                            };
                            let original = value.clone();
                            let id = egui::Id::new(("metadata", self.project.active, metadata, c));
                            let response = table_edit(ui, value, cell_width, METADATA, id, false);
                            if response.changed() {
                                change = Some((id, 0, c, original, metadata as u8 + 1));
                            }
                            finished |= response.lost_focus();
                        }
                    });
                }
                // ponytail: formula row is read-only; add column calculations as a separate feature.
                ui.horizontal(|ui| {
                    cell_label(ui, "F(x)=", label_width, GRAY);
                    for _ in 0..cols {
                        cell_label(ui, "", cell_width, METADATA);
                    }
                });
                egui::ScrollArea::vertical()
                    .id_salt("vertical_table")
                    .max_height((height - 151.0).max(160.0))
                    .auto_shrink([false, false])
                    .show_rows(ui, row_height, rows, |ui, range| {
                        for r in range {
                            ui.horizontal(|ui| {
                                cell_label(ui, &(r + 1).to_string(), label_width, GRAY);
                                for c in 0..cols {
                                    let id = egui::Id::new(("data", self.project.active, r, c));
                                    let value =
                                        &mut self.project.books[self.project.active].rows[r][c];
                                    let original = value.clone();
                                    let response = table_edit(
                                        ui,
                                        value,
                                        cell_width,
                                        Color32::from_rgb(245, 245, 245),
                                        id,
                                        true,
                                    );
                                    if response.changed() {
                                        change = Some((id, r, c, original, 0));
                                    }
                                    finished |= response.lost_focus();
                                }
                            });
                        }
                    });
            });
        if let Some((id, r, c, original, kind)) = change {
            if self.edit_id != Some(id) {
                let mut before = self.project.clone();
                let book = &mut before.books[before.active];
                match kind {
                    0 => book.rows[r][c] = original,
                    1 => book.columns[c].name = original,
                    2 => book.columns[c].unit = original,
                    _ => book.columns[c].comments = original,
                };
                self.remember_project(before);
                self.edit_id = Some(id);
            }
            self.data_changed();
            if kind == 1 || kind == 2 {
                self.project.sync_labels();
            }
        }
        if finished {
            self.edit_id = None;
        }
        ui.spacing_mut().item_spacing = Vec2::new(4.0, 3.0);
        ui.horizontal(|ui| {
            if ui.small_button("+").clicked() {
                self.new_book();
            }
            if ui.selectable_label(true, "Sheet1").clicked() {
                self.activate(0);
            }
        });
    }

    fn graph(&mut self, ui: &mut egui::Ui, height: f32) {
        let key = (
            self.project.active,
            self.project.x_col,
            self.project.y_col,
            self.revision,
        );
        if self.plot_key != Some(key) {
            self.plot_points = self
                .project
                .points()
                .into_iter()
                .map(PlotPoint::from)
                .collect();
            self.plot_key = Some(key);
        }
        frame().inner_margin(14).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            if !self.project.style.title.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new(&self.project.style.title).size(17.0));
                });
            }
            let style = &self.project.style;
            let color = Color32::from_rgb(style.color[0], style.color[1], style.color[2]);
            let name = self.project.book().columns[self.project.y_col].name.clone();
            let mut plot = Plot::new((
                "main_plot",
                self.project.active,
                self.project.x_col,
                self.project.y_col,
            ))
            .height((height - 65.0).max(180.0))
            .x_axis_label(&style.x_label)
            .y_axis_label(&style.y_label)
            .show_grid([style.grid, style.grid])
            .auto_bounds([true, true]);
            if style.legend {
                plot = plot.legend(Legend::default().follow_insertion_order(true));
            }
            if self.reset_plot {
                plot = plot.reset();
                self.reset_plot = false;
            }
            let zoom = self.zoom_factor;
            self.zoom_factor = 1.0;
            plot.show(ui, |plot_ui| {
                if zoom != 1.0 {
                    let center = plot_ui.plot_bounds().center();
                    plot_ui.zoom_bounds(Vec2::splat(zoom), center);
                }
                if style.kind != "scatter" {
                    plot_ui.line(
                        Line::new(&name, PlotPoints::Borrowed(&self.plot_points))
                            .color(color)
                            .width(style.width),
                    );
                }
                if style.kind != "line" {
                    plot_ui.points(
                        Points::new(
                            if style.kind == "scatter" {
                                name.as_str()
                            } else {
                                ""
                            },
                            PlotPoints::Borrowed(&self.plot_points),
                        )
                        .color(color)
                        .radius(style.marker_size / 2.0),
                    );
                }
                if let Some(ec) = self.project.error_col {
                    for (i, row) in self.project.book().rows.iter().enumerate() {
                        if let (Ok(x), Ok(y), Ok(e)) = (
                            row[self.project.x_col].parse::<f64>(),
                            row[self.project.y_col].parse::<f64>(),
                            row[ec].parse::<f64>(),
                        ) {
                            if x.is_finite() && y.is_finite() && e.is_finite() && e > 0.0 {
                                plot_ui.line(
                                    Line::new("", vec![[x, y - e], [x, y + e]])
                                        .id(egui::Id::new(("error", i)))
                                        .color(color.gamma_multiply(0.55))
                                        .width(0.8_f32)
                                        .allow_hover(false),
                                );
                            }
                        }
                    }
                }
                if let Some(fit) = &self.project.fit {
                    plot_ui.line(
                        Line::new("Fit", fit.points.clone())
                            .color(Color32::from_rgb(206, 54, 52))
                            .width(2.0_f32),
                    );
                }
            });
        });
    }

    fn report(&mut self, ui: &mut egui::Ui, height: f32) {
        frame().inner_margin(8).show(ui, |ui| {
            ui.set_min_size(Vec2::new(ui.available_width(), height - 20.0));
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.report_tab, 0, "FitReport1");
                ui.selectable_value(&mut self.report_tab, 1, "Statistics1");
                ui.selectable_value(&mut self.report_tab, 2, "消息日志");
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height((height - 66.0).max(130.0))
                .show(ui, |ui| match self.report_tab {
                    0 => {
                        if let Some(fit) = &self.project.fit {
                            ui.columns(if self.show_residuals { 2 } else { 1 }, |columns| {
                                let ui = &mut columns[0];
                                ui.label(RichText::new(&fit.equation).strong());
                                egui::ScrollArea::horizontal().show(ui, |ui| {
                                    egui::Grid::new("fit_report")
                                        .striped(true)
                                        .spacing([12.0, 7.0])
                                        .show(ui, |ui| {
                                            for label in [
                                                "参数",
                                                "数值",
                                                "标准误",
                                                "95% CI 下限",
                                                "95% CI 上限",
                                            ] {
                                                ui.label(RichText::new(label).strong());
                                            }
                                            ui.end_row();
                                            for p in &fit.params {
                                                for value in [
                                                    p.name.clone(),
                                                    number(Some(p.value)),
                                                    number(p.stderr),
                                                    number(p.low),
                                                    number(p.high),
                                                ] {
                                                    ui.label(value);
                                                }
                                                ui.end_row();
                                            }
                                        });
                                });
                                ui.separator();
                                ui.label(format!(
                                    "N = {}    R² = {}    RMSE = {:.5}",
                                    fit.n,
                                    number(fit.r2),
                                    fit.rmse
                                ));
                                ui.label(format!(
                                    "{}    排除：{}",
                                    if fit.weighted {
                                        "加权"
                                    } else {
                                        "最小二乘"
                                    },
                                    fit.dropped
                                ));
                                if self.show_residuals {
                                    let ui = &mut columns[1];
                                    ui.label("残差");
                                    Plot::new("residual_plot")
                                        .height((height - 100.0).clamp(130.0, 340.0))
                                        .show_grid([true, true])
                                        .show(ui, |p| {
                                            p.points(
                                                Points::new("Residual", fit.residuals.clone())
                                                    .color(Color32::from_rgb(41, 103, 177))
                                                    .radius(2.0_f32),
                                            );
                                            p.hline(
                                                egui_plot::HLine::new("", 0.0).color(Color32::GRAY),
                                            );
                                        });
                                }
                            });
                        }
                    }
                    1 => {
                        if let Some(s) = &self.project.statistics {
                            egui::Grid::new("statistics_report")
                                .striped(true)
                                .spacing([30.0, 7.0])
                                .show(ui, |ui| {
                                    for (label, value) in [
                                        ("N", s.count.to_string()),
                                        ("均值", number(Some(s.mean))),
                                        ("中位数", number(Some(s.median))),
                                        ("标准差", number(s.sd)),
                                        ("最小值", number(Some(s.minimum))),
                                        ("最大值", number(Some(s.maximum))),
                                        ("排除", s.dropped.to_string()),
                                    ] {
                                        ui.label(label);
                                        ui.label(value);
                                        ui.end_row();
                                    }
                                });
                        }
                    }
                    _ => {
                        for msg in self.messages.iter().rev() {
                            ui.label(msg);
                        }
                    }
                });
        });
    }

    fn analysis_dialog(&mut self, ctx: &egui::Context) {
        if !self.fit_open {
            return;
        }
        let mut open = true;
        let mut run = false;
        let mut close = false;
        let action = self.analysis_action.clone();
        let old = (
            self.project.x_col,
            self.project.y_col,
            self.project.error_col,
            self.project.model.clone(),
        );
        let title = if action == "statistics" {
            "描述统计"
        } else {
            "曲线拟合 - NLFit"
        };
        egui::Window::new(title)
            .open(&mut open)
            .default_width(470.0)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                let names: Vec<_> = self
                    .project
                    .book()
                    .columns
                    .iter()
                    .enumerate()
                    .map(|(i, c)| format!("{}  {}", column_code(i), c.name))
                    .collect();
                egui::Grid::new("analysis_fields")
                    .spacing([12.0, 9.0])
                    .show(ui, |ui| {
                        ui.label("工作表");
                        ui.label(format!("[{}]Sheet1", self.project.book().name));
                        ui.end_row();
                        for (label, id, col) in [
                            ("X", "dialog_x", &mut self.project.x_col),
                            ("Y", "dialog_y", &mut self.project.y_col),
                        ] {
                            ui.label(label);
                            egui::ComboBox::from_id_salt(id)
                                .width(300.0)
                                .selected_text(&names[*col])
                                .show_ui(ui, |ui| {
                                    for (i, name) in names.iter().enumerate() {
                                        ui.selectable_value(col, i, name);
                                    }
                                });
                            ui.end_row();
                        }
                        if action == "fit" {
                            ui.label("Y 误差");
                            egui::ComboBox::from_id_salt("dialog_error")
                                .width(300.0)
                                .selected_text(
                                    self.project
                                        .error_col
                                        .map(|i| names[i].as_str())
                                        .unwrap_or("无"),
                                )
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut self.project.error_col, None, "无");
                                    for (i, name) in names.iter().enumerate() {
                                        ui.selectable_value(
                                            &mut self.project.error_col,
                                            Some(i),
                                            name,
                                        );
                                    }
                                });
                            ui.end_row();
                            ui.label("函数");
                            egui::ComboBox::from_id_salt("dialog_function")
                                .width(300.0)
                                .selected_text(
                                    MODELS
                                        .iter()
                                        .find(|(k, _)| *k == self.project.model)
                                        .unwrap()
                                        .1,
                                )
                                .show_ui(ui, |ui| {
                                    for (key, label) in MODELS {
                                        ui.selectable_value(
                                            &mut self.project.model,
                                            key.into(),
                                            label,
                                        );
                                    }
                                });
                            ui.end_row();
                        }
                    });
                ui.separator();
                if ui.checkbox(&mut self.range_enabled, "X 范围").changed() {
                    self.data_changed();
                    self.needs_fit = false;
                }
                if self.range_enabled {
                    ui.horizontal(|ui| {
                        let a = ui
                            .add_sized(
                                [130.0, 23.0],
                                egui::TextEdit::singleline(&mut self.range_start),
                            )
                            .changed();
                        ui.label("—");
                        let b = ui
                            .add_sized(
                                [130.0, 23.0],
                                egui::TextEdit::singleline(&mut self.range_end),
                            )
                            .changed();
                        if a || b {
                            self.data_changed();
                            self.needs_fit = false;
                        }
                    });
                }
                ui.checkbox(&mut self.auto_fit, "自动重算");
                ui.separator();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("取消").clicked() {
                        close = true;
                    }
                    if ui
                        .add_enabled(
                            self.task.is_none(),
                            egui::Button::new(if action == "fit" { "拟合" } else { "确定" }),
                        )
                        .clicked()
                    {
                        run = true;
                    }
                });
            });
        if old
            != (
                self.project.x_col,
                self.project.y_col,
                self.project.error_col,
                self.project.model.clone(),
            )
        {
            self.data_changed();
            self.needs_fit = false;
            self.project.sync_labels();
            self.reset_plot = true;
        }
        self.fit_open = open && !close;
        if run {
            self.needs_fit = false;
            self.start_analysis(&action);
            self.fit_open = false;
        }
    }

    fn format_dialog(&mut self, ctx: &egui::Context) {
        if !self.format_open {
            return;
        }
        let mut open = true;
        let mut close = false;
        let before = self.project.style.clone();
        egui::Window::new("绘图细节")
            .open(&mut open)
            .default_width(465.0)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                egui::Grid::new("format_fields")
                    .spacing([12.0, 9.0])
                    .show(ui, |ui| {
                        for (label, value) in [
                            ("标题", &mut self.project.style.title),
                            ("X 轴", &mut self.project.style.x_label),
                            ("Y 轴", &mut self.project.style.y_label),
                        ] {
                            ui.label(label);
                            ui.add_sized(
                                [300.0, 23.0],
                                egui::TextEdit::singleline(value).char_limit(512),
                            );
                            ui.end_row();
                        }
                        ui.label("颜色");
                        ui.color_edit_button_srgb(&mut self.project.style.color);
                        ui.end_row();
                        ui.label("线宽");
                        ui.add(egui::Slider::new(&mut self.project.style.width, 0.5..=5.0));
                        ui.end_row();
                        ui.label("符号");
                        ui.add(egui::Slider::new(
                            &mut self.project.style.marker_size,
                            1.0..=10.0,
                        ));
                        ui.end_row();
                    });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.project.style.grid, "网格线");
                    ui.checkbox(&mut self.project.style.legend, "图例");
                });
                ui.separator();
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("确定").clicked() {
                        close = true;
                    }
                });
            });
        if serde_json::to_value(before).ok() != serde_json::to_value(&self.project.style).ok() {
            self.dirty = true;
        }
        self.format_open = open && !close;
    }

    fn render(&mut self, ctx: &egui::Context) {
        let file = self
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "UNTITLED".into());
        let active = match self.active_window {
            1 => "Graph1".into(),
            2 => "FitReport1".into(),
            _ => self.project.book().name.clone(),
        };
        let title = format!(
            "PlotLab - {file}{} - [{active}]",
            if self.dirty { " *" } else { "" }
        );
        if self.window_title != title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
        self.toolbar(ctx);
        egui::TopBottomPanel::bottom("status")
            .frame(egui::Frame::new().fill(TOOLBAR).inner_margin(3))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(RichText::new(&self.status).small()).truncate());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(format!("[{}]Sheet1", self.project.book().name));
                        ui.separator();
                        ui.label(format!(
                            "({}×{})",
                            self.project.book().columns.len(),
                            self.project.book().rows.len()
                        ));
                        ui.separator();
                        ui.label(if self.auto_fit { "AU : ON" } else { "AU : OFF" });
                    });
                });
            });
        egui::TopBottomPanel::bottom("plot_toolbar")
            .frame(
                egui::Frame::new()
                    .fill(TOOLBAR)
                    .inner_margin(egui::Margin::symmetric(7, 2)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    for (key, tip) in [
                        ("line", "折线图"),
                        ("scatter", "散点图"),
                        ("line_points", "点线图"),
                    ] {
                        if icon_button(
                            ui,
                            key,
                            tip,
                            true,
                            self.project.style.kind == key && self.active_window == 1,
                        )
                        .clicked()
                        {
                            self.plot_kind(key);
                        }
                    }
                    ui.separator();
                    for (key, tip) in [
                        ("fit", "曲线拟合"),
                        ("statistics", "描述统计"),
                        ("bounds", "调整刻度"),
                        ("grid", "网格线"),
                        ("legend", "图例"),
                        ("tile", "平铺"),
                    ] {
                        if icon_button(ui, key, tip, true, false).clicked() {
                            self.tool(key);
                        }
                    }
                });
            });
        egui::TopBottomPanel::bottom("window_tabs")
            .frame(egui::Frame::new().fill(WORKSPACE).inner_margin(3))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let names: Vec<_> = self.project.books.iter().map(|b| b.name.clone()).collect();
                    for (i, name) in names.iter().enumerate() {
                        if ui
                            .selectable_label(
                                self.active_window == 0 && i == self.project.active,
                                name,
                            )
                            .clicked()
                        {
                            self.switch_book(i);
                            self.activate(0);
                        }
                    }
                    if self.show_graph
                        && ui
                            .selectable_label(self.active_window == 1, "Graph1")
                            .clicked()
                    {
                        self.activate(1);
                    }
                    if self.show_report
                        && ui
                            .selectable_label(self.active_window == 2, "FitReport1")
                            .clicked()
                    {
                        self.activate(2);
                    }
                });
            });
        egui::SidePanel::left("vertical_tools")
            .exact_width(29.0)
            .frame(egui::Frame::new().fill(TOOLBAR).inner_margin(2))
            .resizable(false)
            .show(ctx, |ui| {
                for (key, tip) in [
                    ("pointer", "选择"),
                    ("zoom_in", "放大"),
                    ("zoom_out", "缩小"),
                    ("bounds", "调整刻度"),
                    ("format", "绘图细节"),
                    ("grid", "网格线"),
                    ("legend", "图例"),
                    ("export", "导出图形"),
                ] {
                    if icon_button(ui, key, tip, true, false).clicked() {
                        self.tool(key);
                    }
                }
            });
        self.project_tree(ctx);
        self.object_manager(ctx);
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(WORKSPACE)
                    .inner_margin(if self.tiled { 5 } else { 0 }),
            )
            .show(ctx, |ui| {
                let size = ui.available_size();
                if self.tiled {
                    let h = if self.show_report {
                        size.y * 0.64
                    } else {
                        size.y
                    };
                    ui.columns(2, |cols| {
                        panel_title(&mut cols[0], &self.project.book().name, false);
                        self.worksheet(&mut cols[0], h - 25.0);
                        panel_title(&mut cols[1], "Graph1", false);
                        self.graph(&mut cols[1], h - 25.0);
                    });
                    if self.show_report {
                        ui.separator();
                        self.report(ui, ui.available_height().max(180.0));
                    }
                } else {
                    match self.active_window {
                        1 => self.graph(ui, size.y),
                        2 => self.report(ui, size.y),
                        _ => self.worksheet(ui, size.y),
                    }
                }
            });
        self.analysis_dialog(ctx);
        self.format_dialog(ctx);
        if self.help_open {
            egui::Window::new("使用说明").open(&mut self.help_open).default_width(460.0).show(ctx,|ui|{ui.label("数据 → 从文件导入\n绘图 → 折线 / 散点 / 点线\n分析 → 拟合\n统计 → 描述统计\n格式 → 绘图细节\n文件 → 保存 / 导出\n帮助 → 示例项目");ui.separator();ui.label("PlotLab 0.3\nF(x) 列计算、Origin 工程格式和插件支持属于后续范围。");});
        }
    }
}

fn cell_label(ui: &mut egui::Ui, text: &str, width: f32, color: Color32) -> egui::Response {
    egui::Frame::new()
        .fill(color)
        .stroke(egui::Stroke::new(0.5_f32, Color32::from_rgb(190, 190, 198)))
        .show(ui, |ui| {
            ui.add_sized(
                [width, 21.0],
                egui::Label::new(RichText::new(text).size(13.0)).sense(egui::Sense::click()),
            )
        })
        .inner
}

fn table_edit(
    ui: &mut egui::Ui,
    text: &mut String,
    width: f32,
    color: Color32,
    id: egui::Id,
    numeric: bool,
) -> egui::Response {
    egui::Frame::new()
        .fill(color)
        .stroke(egui::Stroke::new(0.5_f32, Color32::from_rgb(190, 190, 198)))
        .inner_margin(egui::Margin::symmetric(3, 1))
        .show(ui, |ui| {
            ui.add_sized(
                [width - 6.0, 18.0],
                egui::TextEdit::singleline(text)
                    .id(id)
                    .frame(false)
                    .font(egui::TextStyle::Monospace)
                    .horizontal_align(if numeric {
                        egui::Align::RIGHT
                    } else {
                        egui::Align::Center
                    })
                    .char_limit(4096),
            )
        })
        .inner
}

fn number(value: Option<f64>) -> String {
    value
        .map(|v| format!("{v:.5}"))
        .unwrap_or_else(|| "—".into())
}

impl eframe::App for PlotLab {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frames += 1;
        self.poll_task();
        if self.needs_fit
            && self.auto_fit
            && self.task.is_none()
            && self.last_edit.elapsed() > Duration::from_millis(800)
        {
            self.start_analysis("fit");
        }
        if self.dirty && self.last_save.elapsed() > Duration::from_secs(15) {
            self.backup();
        }
        let keyboard = ctx.wants_keyboard_input();
        let shortcuts = ctx.input(|i| {
            (
                i.modifiers.command && i.key_pressed(egui::Key::S),
                i.modifiers.command && i.key_pressed(egui::Key::O),
                i.modifiers.command && i.key_pressed(egui::Key::N),
                i.modifiers.command && i.key_pressed(egui::Key::Z),
                i.modifiers.command && i.key_pressed(egui::Key::Y),
                i.modifiers.command && i.key_pressed(egui::Key::R),
            )
        });
        if shortcuts.0 {
            self.save(false);
        }
        if !keyboard {
            if shortcuts.1 {
                self.open();
            }
            if shortcuts.2 {
                self.new_book();
            }
            if shortcuts.3 {
                self.undo(false);
            }
            if shortcuts.4 {
                self.undo(true);
            }
            if shortcuts.5 {
                self.reset_plot = true;
            }
        }
        self.render(ctx);
        if self.screenshot.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        if self.needs_fit && self.auto_fit && self.task.is_none() {
            ctx.request_repaint_after(
                Duration::from_millis(800).saturating_sub(self.last_edit.elapsed()),
            );
        }
        if self.dirty {
            ctx.request_repaint_after(
                Duration::from_secs(15).saturating_sub(self.last_save.elapsed()),
            );
        }
        if self.screenshot.is_some()
            && !self.screenshot_requested
            && self.task.is_none()
            && self.frames > 20
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            self.screenshot_requested = true;
        }
        if let Some(path) = &self.screenshot {
            let capture = ctx.input(|i| {
                i.events.iter().find_map(|e| {
                    if let egui::Event::Screenshot { image, .. } = e {
                        Some(image.clone())
                    } else {
                        None
                    }
                })
            });
            if let Some(image) = capture {
                let bytes: Vec<u8> = image.pixels.iter().flat_map(|c| c.to_array()).collect();
                let _ = ::image::save_buffer(
                    path,
                    &bytes,
                    image.size[0] as u32,
                    image.size[1] as u32,
                    ::image::ColorType::Rgba8,
                );
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.task = None;
        if self.screenshot.is_none() {
            self.backup();
        }
    }
}

pub fn smoke_test() -> Result<(), String> {
    let root = app_directory();
    let output = root.join("verification");
    std::fs::create_dir_all(&output).map_err(|e| e.to_string())?;
    let mut project = Project::demo("exponential");
    let reply = run_task(project.request("fit"), Arc::new(AtomicBool::new(false)))?;
    project.fit = Some(serde_json::from_value(reply).map_err(|e| e.to_string())?);
    let fit = project.fit.as_ref().unwrap();
    if fit.rmse > 1.0 || (fit.params[2].value - 3.1).abs() > 0.12 {
        return Err("指数拟合结果超出验收范围。".into());
    }
    atomic_save(&output.join("example.plab"), &project)?;
    let restored = load_project(&output.join("example.plab"))?;
    if serde_json::to_value(&restored).unwrap() != serde_json::to_value(&project).unwrap() {
        return Err("工程往返保存不一致。".into());
    }
    atomic_save(&output.join("example.plab"), &project)?;
    for extension in ["svg", "pdf", "png"] {
        let mut req = project.request("export");
        req["path"] = json!(output.join(format!("example.{extension}")));
        run_task(req, Arc::new(AtomicBool::new(false)))?;
    }
    let mut req = project.request("export_csv");
    req["path"] = json!(output.join("example.csv"));
    run_task(req, Arc::new(AtomicBool::new(false)))?;
    let imported = run_task(
        json!({"action":"import","path":output.join("example.csv")}),
        Arc::new(AtomicBool::new(false)),
    )?;
    let book: Workbook = serde_json::from_value(imported).map_err(|e| e.to_string())?;
    if book.rows != project.book().rows
        || column_label(&book.columns[0]) != column_label(&project.book().columns[0])
    {
        return Err("CSV 往返保存不一致。".into());
    }
    std::fs::write(output.join("passed.txt"),"PASS: native Rust fit; project round trip and overwrite; SVG/PDF/PNG export; CSV round trip.\n").map_err(|e|e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> PlotLab {
        PlotLab {
            egui_ctx: egui::Context::default(),
            window_title: String::new(),
            plot_points: vec![],
            plot_key: None,
            project: Project::demo("linear"),
            path: None,
            dirty: false,
            undo: vec![],
            redo: vec![],
            revision: 0,
            task: None,
            status: "就绪".into(),
            messages: vec![],
            auto_fit: true,
            needs_fit: false,
            last_edit: Instant::now(),
            last_save: Instant::now(),
            edit_id: None,
            reset_plot: false,
            show_graph: true,
            show_report: true,
            active_window: 0,
            tiled: false,
            show_project: true,
            show_objects: true,
            show_apps: true,
            fit_open: false,
            format_open: false,
            analysis_action: "fit".into(),
            selected_col: 1,
            search: String::new(),
            zoom_factor: 1.0,
            reveal_results: false,
            show_residuals: true,
            report_tab: 0,
            range_enabled: false,
            range_start: "0".into(),
            range_end: "12".into(),
            help_open: false,
            screenshot: None,
            screenshot_requested: false,
            frames: 0,
        }
    }
    fn fitted() -> FitResult {
        FitResult {
            model: "linear".into(),
            equation: "y = 2.8*x + 1.5".into(),
            params: vec![],
            points: vec![[0.0, 1.5], [12.0, 35.1]],
            residuals: vec![[0.0, 0.0]],
            r2: Some(1.0),
            rmse: 0.0,
            n: 1,
            dropped: 0,
            weighted: false,
        }
    }
    #[test]
    fn idle_ui_stops_requesting_repaints_and_updates_title_only_on_change() {
        let ctx = egui::Context::default();
        configure_style(&ctx);
        let mut app = state();
        let input = |time| egui::RawInput {
            time: Some(time),
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1536.0, 980.0),
            )),
            ..Default::default()
        };
        for i in 0..16 {
            let _ = ctx.run(input(i as f64 * 0.1), |ctx| app.render(ctx));
        }
        let output = ctx.run(input(2.0), |ctx| app.render(ctx));
        let root = &output.viewport_output[&egui::ViewportId::ROOT];
        assert!(
            !root
                .commands
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::Title(_)))
        );
        assert!(
            root.repaint_delay > Duration::from_secs(1),
            "idle UI is repainting: {:?}",
            root.repaint_delay
        );
        app.activate(1);
        let output = ctx.run(input(2.1), |ctx| app.render(ctx));
        assert!(
            output.viewport_output[&egui::ViewportId::ROOT]
                .commands
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::Title(_)))
        );
    }
    #[test]
    fn plot_cache_is_invalidated_by_data_edits() {
        let ctx = egui::Context::default();
        let mut app = state();
        app.activate(1);
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1536.0, 980.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(input(), |ctx| app.render(ctx));
        let old = app.plot_points[0].y;
        let key = app.plot_key;
        let _ = ctx.run(input(), |ctx| app.render(ctx));
        assert_eq!(app.plot_key, key);
        app.project.books[0].rows[0][1] = "99.125".into();
        app.data_changed();
        let _ = ctx.run(input(), |ctx| app.render(ctx));
        assert_ne!(app.plot_points[0].y, old);
        assert_eq!(app.plot_points[0].y, 99.125);
        assert_ne!(app.plot_key, key);
    }
    #[test]
    fn project_rejects_bad_column_and_version() {
        let mut p = Project::demo("linear");
        p.x_col = 100;
        assert!(p.validate().is_err());
        p.x_col = 0;
        p.version = 99;
        assert!(p.validate().is_err());
    }
    #[test]
    fn project_rejects_ragged_and_invalid_style() {
        let mut p = Project::demo("linear");
        p.books[0].rows[0].pop();
        assert!(p.validate().is_err());
        let mut p = Project::demo("linear");
        p.style.width = -1.0;
        assert!(p.validate().is_err());
    }
    #[test]
    fn graph_skips_nonfinite_and_sorts_x() {
        let mut p = Project::demo("linear");
        p.books[0].rows = vec![
            vec!["2".into(), "5".into(), "".into(), "".into()],
            vec!["NaN".into(), "5".into(), "".into(), "".into()],
            vec!["1".into(), "3".into(), "".into(), "".into()],
        ];
        assert_eq!(p.points(), vec![[1.0, 3.0], [2.0, 5.0]]);
    }
    #[test]
    fn repeated_edits_keep_auto_fit_pending_and_undo_restores_data() {
        let mut app = state();
        app.project.fit = Some(fitted());
        let old = app.project.books[0].rows[0][1].clone();
        app.remember();
        app.project.books[0].rows[0][1] = "99.125".into();
        app.data_changed();
        app.data_changed();
        assert!(app.needs_fit);
        assert!(app.project.fit.is_none());
        app.undo(false);
        assert_eq!(app.project.books[0].rows[0][1], old);
        assert!(app.project.fit.is_some());
        app.undo(true);
        assert_eq!(app.project.books[0].rows[0][1], "99.125");
        assert!(app.project.fit.is_none());
    }
    #[test]
    fn analysis_range_is_saved_but_export_keeps_all_points() {
        let mut p = Project::demo("linear");
        p.range = Some([2.0, 7.0]);
        p.validate().unwrap();
        let roundtrip: Project = serde_json::from_value(serde_json::to_value(&p).unwrap()).unwrap();
        assert_eq!(roundtrip.range, Some([2.0, 7.0]));
        assert_eq!(p.request("fit")["x_min"], 2.0);
        assert!(p.request("export")["x_min"].is_null());
    }
    #[test]
    fn worksheet_keyboard_edit_updates_model_and_invalidates_fit() {
        let ctx = egui::Context::default();
        configure_style(&ctx);
        let mut app = state();
        app.project.fit = Some(fitted());
        let input = |events| egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1536.0, 980.0),
            )),
            events,
            ..Default::default()
        };
        let _ = ctx.run(input(vec![]), |ctx| app.render(ctx));
        let id = egui::Id::new(("data", 0_usize, 0_usize, 1_usize));
        let pos = ctx
            .read_response(id)
            .expect("data cell is rendered")
            .rect
            .center();
        let _ = ctx.run(
            input(vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ]),
            |ctx| app.render(ctx),
        );
        let _ = ctx.run(
            input(vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }]),
            |ctx| app.render(ctx),
        );
        let mods = egui::Modifiers {
            ctrl: true,
            command: true,
            ..Default::default()
        };
        let _ = ctx.run(
            input(vec![egui::Event::Key {
                key: egui::Key::A,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: mods,
            }]),
            |ctx| app.render(ctx),
        );
        let _ = ctx.run(input(vec![egui::Event::Text("99.125".into())]), |ctx| {
            app.render(ctx)
        });
        assert_eq!(app.project.books[0].rows[0][1], "99.125");
        assert!(app.project.fit.is_none());
        assert!(app.needs_fit);
    }
}
