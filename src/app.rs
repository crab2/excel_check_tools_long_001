use eframe::egui::{
    self, Align, Align2, Color32, ComboBox, FontData, FontDefinitions, FontFamily, FontId, Frame,
    Layout, Margin, RichText, ScrollArea, Stroke, TextEdit, TextStyle, Vec2,
};
use industry_excel_checker::model::normalize_rule_code;
use industry_excel_checker::{
    AnalysisReport, AppSettings, CategoryRepository, CategorySet, CheckOptions, CheckRow,
    HierarchyIndex, IndustryCategory, IndustryRule, MetricKind, MetricRule, RowStatus,
    RuleRepository, RuleSet, SettingsRepository, analyze_workbook, default_output_path,
    export_checked_workbook, load_categories_from_path, load_default_categories,
    load_default_rules, load_rules_from_path,
};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    mpsc::{self, Receiver, TryRecvError},
};
use std::time::{Duration, Instant};

const PAGE_BG: Color32 = Color32::from_rgb(244, 246, 247);
const SURFACE: Color32 = Color32::from_rgb(255, 255, 255);
const INK: Color32 = Color32::from_rgb(28, 39, 45);
const MUTED: Color32 = Color32::from_rgb(91, 106, 113);
const BORDER: Color32 = Color32::from_rgb(216, 222, 225);
const ACCENT: Color32 = Color32::from_rgb(16, 118, 111);
const ACCENT_SOFT: Color32 = Color32::from_rgb(226, 242, 239);
const DANGER: Color32 = Color32::from_rgb(174, 55, 55);
const WARNING: Color32 = Color32::from_rgb(172, 111, 22);

type BackgroundResult = std::result::Result<AnalysisReport, String>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    DataCheck,
    Configuration,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConfigurationTab {
    Rules,
    Categories,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RowFilter {
    All,
    Changed,
    Unchanged,
    Skipped,
}

impl RowFilter {
    const ALL: [Self; 4] = [Self::All, Self::Changed, Self::Unchanged, Self::Skipped];

    fn label(self) -> &'static str {
        match self {
            Self::All => "全部",
            Self::Changed => "待修改",
            Self::Unchanged => "一致",
            Self::Skipped => "已跳过",
        }
    }

    fn matches(self, status: RowStatus) -> bool {
        match self {
            Self::All => true,
            Self::Changed => status == RowStatus::Changed,
            Self::Unchanged => status == RowStatus::Unchanged,
            Self::Skipped => status == RowStatus::Skipped,
        }
    }
}

#[derive(Clone, Copy)]
enum NoticeLevel {
    Info,
    Success,
    Error,
}

struct Notice {
    level: NoticeLevel,
    message: String,
}

struct RuleEditor {
    original_id: Option<u64>,
    draft: IndustryRule,
    error: Option<String>,
}

struct CategoryEditor {
    original_id: Option<u64>,
    draft: IndustryCategory,
    error: Option<String>,
}

#[derive(Clone, Copy)]
enum ConfirmAction {
    DeleteRule(u64),
    DeleteCategory(u64),
    RestoreRules,
    RestoreCategories,
}

pub struct IndustryCheckApp {
    page: Page,
    configuration_tab: ConfigurationTab,
    rule_repository: RuleRepository,
    category_repository: CategoryRepository,
    settings_repository: SettingsRepository,
    rules: RuleSet,
    categories: CategorySet,
    settings: AppSettings,
    source_path: String,
    output_path: String,
    analysis_receiver: Option<Receiver<BackgroundResult>>,
    analysis_started_at: Option<Instant>,
    report: Option<AnalysisReport>,
    preview_search: String,
    row_filter: RowFilter,
    rule_search: String,
    category_search: String,
    rule_editor: Option<RuleEditor>,
    category_editor: Option<CategoryEditor>,
    confirmation: Option<ConfirmAction>,
    notice: Option<Notice>,
}

impl IndustryCheckApp {
    pub fn new(creation_context: &eframe::CreationContext<'_>) -> anyhow::Result<Self> {
        let mut startup_warnings = Vec::new();
        if let Err(message) = install_cjk_font(&creation_context.egui_ctx) {
            startup_warnings.push(message);
        }
        configure_style(&creation_context.egui_ctx);

        let rule_repository = RuleRepository::discover()?;
        let category_repository = CategoryRepository::discover()?;
        let settings_repository = SettingsRepository::discover()?;

        let rules = match rule_repository.load_or_default() {
            Ok(rules) => rules,
            Err(error) => {
                startup_warnings.push(format!("自定义划型标准载入失败，已使用内置标准：{error:#}"));
                load_default_rules()?
            }
        };
        let categories = match category_repository.load_or_default() {
            Ok(categories) => categories,
            Err(error) => {
                startup_warnings.push(format!("自定义行业细类载入失败，已使用内置细类：{error:#}"));
                load_default_categories()?
            }
        };
        let settings = match settings_repository.load() {
            Ok(settings) => settings,
            Err(error) => {
                startup_warnings.push(format!("应用设置载入失败，已使用默认设置：{error:#}"));
                AppSettings::default()
            }
        };

        let source_path = settings
            .last_source_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let output_path = settings
            .last_output_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default();
        let notice = (!startup_warnings.is_empty()).then(|| Notice {
            level: NoticeLevel::Error,
            message: startup_warnings.join("\n"),
        });

        Ok(Self {
            page: Page::DataCheck,
            configuration_tab: ConfigurationTab::Rules,
            rule_repository,
            category_repository,
            settings_repository,
            rules,
            categories,
            settings,
            source_path,
            output_path,
            analysis_receiver: None,
            analysis_started_at: None,
            report: None,
            preview_search: String::new(),
            row_filter: RowFilter::All,
            rule_search: String::new(),
            category_search: String::new(),
            rule_editor: None,
            category_editor: None,
            confirmation: None,
            notice,
        })
    }

    fn is_analyzing(&self) -> bool {
        self.analysis_receiver.is_some()
    }

    fn set_notice(&mut self, level: NoticeLevel, message: impl Into<String>) {
        self.notice = Some(Notice {
            level,
            message: message.into(),
        });
    }

    fn sync_settings_paths(&mut self) {
        self.settings.last_source_path = non_empty_path(&self.source_path);
        self.settings.last_output_path = non_empty_path(&self.output_path);
    }

    fn save_settings(&mut self) {
        self.sync_settings_paths();
        if let Err(error) = self.settings_repository.save(&self.settings) {
            self.set_notice(NoticeLevel::Error, format!("无法保存应用设置：{error:#}"));
        }
    }

    fn poll_analysis(&mut self) {
        let received =
            self.analysis_receiver
                .as_ref()
                .and_then(|receiver| match receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => Some(Err("后台核对任务异常结束".to_owned())),
                });

        if let Some(result) = received {
            self.analysis_receiver = None;
            self.analysis_started_at = None;
            match result {
                Ok(report) => {
                    let changed = report.summary.changed_rows;
                    let skipped = report.summary.skipped_rows;
                    self.report = Some(report);
                    self.set_notice(
                        NoticeLevel::Success,
                        format!("核对完成：{changed} 行待修改，{skipped} 行已跳过"),
                    );
                }
                Err(error) => {
                    self.report = None;
                    self.set_notice(NoticeLevel::Error, error);
                }
            }
        }
    }

    fn start_analysis(&mut self, context: &egui::Context) {
        if self.is_analyzing() {
            return;
        }

        self.settings.annotation_column =
            self.settings.annotation_column.trim().to_ascii_uppercase();
        let options = CheckOptions {
            source_path: PathBuf::from(self.source_path.trim()),
            annotation_column: self.settings.annotation_column.clone(),
            annotate_skipped_rows: self.settings.annotate_skipped_rows,
        };
        if let Err(error) = options.validate() {
            self.set_notice(NoticeLevel::Error, format!("无法开始核对：{error:#}"));
            return;
        }

        self.sync_settings_paths();
        if let Err(error) = self.settings_repository.save(&self.settings) {
            self.set_notice(NoticeLevel::Error, format!("设置保存失败：{error:#}"));
            return;
        }

        let categories = self.categories.clone();
        let rules = self.rules.clone();
        let (sender, receiver) = mpsc::channel();
        let repaint_context = context.clone();
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<AnalysisReport> {
                let hierarchy = HierarchyIndex::from_categories(&categories)?;
                analyze_workbook(&options, &hierarchy, &rules)
            })()
            .map_err(|error| format!("核对失败：{error:#}"));
            let _ = sender.send(result);
            repaint_context.request_repaint();
        });

        self.report = None;
        self.analysis_receiver = Some(receiver);
        self.analysis_started_at = Some(Instant::now());
        self.set_notice(NoticeLevel::Info, "正在读取并核对源数据，请稍候");
    }

    fn export_report(&mut self) {
        let Some(report) = self.report.as_ref() else {
            self.set_notice(NoticeLevel::Error, "请先完成一次核对");
            return;
        };
        let output = PathBuf::from(self.output_path.trim());
        if output.as_os_str().is_empty() {
            self.set_notice(NoticeLevel::Error, "请选择输出文件");
            return;
        }
        match export_checked_workbook(report, &output) {
            Ok(()) => {
                self.save_settings();
                self.set_notice(
                    NoticeLevel::Success,
                    format!("结果已导出到 {}", output.display()),
                );
            }
            Err(error) => {
                self.set_notice(NoticeLevel::Error, format!("导出失败：{error:#}"));
            }
        }
    }

    fn pick_source(&mut self) {
        let mut dialog = rfd::FileDialog::new()
            .set_title("选择源数据工作簿")
            .add_filter("Excel 工作簿", &["xlsx"]);
        if let Some(directory) = existing_parent(&self.source_path) {
            dialog = dialog.set_directory(directory);
        }
        if let Some(path) = dialog.pick_file() {
            self.source_path = path.to_string_lossy().into_owned();
            self.output_path = default_output_path(&path).to_string_lossy().into_owned();
            self.report = None;
            self.save_settings();
        }
    }

    fn pick_output(&mut self) {
        let source = PathBuf::from(self.source_path.trim());
        let suggested = if source.as_os_str().is_empty() {
            PathBuf::from("核对结果.xlsx")
        } else {
            default_output_path(&source)
        };
        let mut dialog = rfd::FileDialog::new()
            .set_title("选择输出文件")
            .add_filter("Excel 工作簿", &["xlsx"])
            .set_file_name(
                suggested
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("核对结果.xlsx"),
            );
        if let Some(directory) = suggested.parent().filter(|path| path.exists()) {
            dialog = dialog.set_directory(directory);
        }
        if let Some(path) = dialog.save_file() {
            self.output_path = ensure_extension(path, "xlsx")
                .to_string_lossy()
                .into_owned();
            self.save_settings();
        }
    }

    fn replace_rules(&mut self, rules: RuleSet, success_message: &str) -> Result<(), String> {
        rules.validate()?;
        self.rule_repository
            .save(&rules)
            .map_err(|error| format!("规则保存失败：{error:#}"))?;
        self.rules = rules;
        self.report = None;
        self.set_notice(NoticeLevel::Success, success_message);
        Ok(())
    }

    fn replace_categories(
        &mut self,
        categories: CategorySet,
        success_message: &str,
    ) -> Result<(), String> {
        categories.validate()?;
        self.category_repository
            .save(&categories)
            .map_err(|error| format!("行业细类保存失败：{error:#}"))?;
        self.categories = categories;
        self.report = None;
        self.set_notice(NoticeLevel::Success, success_message);
        Ok(())
    }

    fn show_navigation(&mut self, context: &egui::Context) {
        egui::TopBottomPanel::top("primary_navigation")
            .exact_height(56.0)
            .frame(
                Frame::new()
                    .fill(INK)
                    .inner_margin(Margin::symmetric(18, 10)),
            )
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("行业划型核对工具")
                            .color(Color32::WHITE)
                            .strong()
                            .size(18.0),
                    );
                    ui.add_space(24.0);

                    let data_button =
                        egui::Button::new(RichText::new("数据核对").color(Color32::WHITE))
                            .selected(self.page == Page::DataCheck)
                            .fill(if self.page == Page::DataCheck {
                                ACCENT
                            } else {
                                Color32::TRANSPARENT
                            });
                    if ui.add(data_button).clicked() {
                        self.page = Page::DataCheck;
                    }

                    let configuration_button =
                        egui::Button::new(RichText::new("配置中心").color(Color32::WHITE))
                            .selected(self.page == Page::Configuration)
                            .fill(if self.page == Page::Configuration {
                                ACCENT
                            } else {
                                Color32::TRANSPARENT
                            });
                    if ui.add(configuration_button).clicked() {
                        self.page = Page::Configuration;
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(format!(
                                "{} 条标准  |  {} 条行业细类",
                                self.rules.rules.len(),
                                self.categories.categories.len()
                            ))
                            .color(Color32::from_rgb(192, 204, 209))
                            .size(12.0),
                        );
                    });
                });
            });
    }

    fn show_notice(&mut self, ui: &mut egui::Ui) {
        let Some(notice) = self.notice.as_ref() else {
            return;
        };
        let (fill, stroke, text_color) = match notice.level {
            NoticeLevel::Info => (
                Color32::from_rgb(232, 240, 244),
                Color32::from_rgb(151, 178, 190),
                Color32::from_rgb(45, 79, 94),
            ),
            NoticeLevel::Success => (
                ACCENT_SOFT,
                Color32::from_rgb(143, 194, 186),
                Color32::from_rgb(25, 92, 84),
            ),
            NoticeLevel::Error => (
                Color32::from_rgb(251, 236, 235),
                Color32::from_rgb(220, 166, 162),
                DANGER,
            ),
        };
        let message = notice.message.clone();
        let mut dismiss = false;
        Frame::new()
            .fill(fill)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(4)
            .inner_margin(Margin::symmetric(12, 9))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(message).color(text_color));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(RichText::new("×").color(text_color))
                                    .frame(false),
                            )
                            .on_hover_text("关闭提示")
                            .clicked()
                        {
                            dismiss = true;
                        }
                    });
                });
            });
        ui.add_space(12.0);
        if dismiss {
            self.notice = None;
        }
    }

    fn show_data_page(&mut self, ui: &mut egui::Ui, context: &egui::Context) {
        ui.horizontal(|ui| {
            ui.heading("数据核对");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if let Some(started_at) = self.analysis_started_at {
                    ui.label(
                        RichText::new(format!(
                            "正在核对  {:.1} 秒",
                            started_at.elapsed().as_secs_f32()
                        ))
                        .color(ACCENT),
                    );
                }
            });
        });
        ui.add_space(8.0);

        let mut pick_source = false;
        let mut pick_output = false;
        let mut run_analysis = false;
        let mut export = false;
        let mut settings_changed = false;
        let mut source_changed = false;
        Frame::new()
            .fill(SURFACE)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(6)
            .inner_margin(Margin::same(14))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_sized([72.0, 30.0], egui::Label::new("源数据"));
                    let input_width = (ui.available_width() - 86.0).max(180.0);
                    let response = ui.add_sized(
                        [input_width, 32.0],
                        TextEdit::singleline(&mut self.source_path)
                            .hint_text("选择 .xlsx 源数据文件"),
                    );
                    source_changed = response.changed();
                    settings_changed |= response.lost_focus();
                    pick_source = ui.button("选择").clicked();
                });
                ui.horizontal(|ui| {
                    ui.add_sized([72.0, 30.0], egui::Label::new("输出文件"));
                    let input_width = (ui.available_width() - 86.0).max(180.0);
                    let response = ui.add_sized(
                        [input_width, 32.0],
                        TextEdit::singleline(&mut self.output_path)
                            .hint_text("选择核对结果保存位置"),
                    );
                    settings_changed |= response.lost_focus();
                    pick_output = ui.button("选择").clicked();
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("标注列");
                    let response = ui.add_sized(
                        [64.0, 32.0],
                        TextEdit::singleline(&mut self.settings.annotation_column)
                            .char_limit(3)
                            .hint_text("Q"),
                    );
                    if response.changed() {
                        self.settings.annotation_column = self
                            .settings
                            .annotation_column
                            .chars()
                            .filter(char::is_ascii_alphabetic)
                            .collect::<String>()
                            .to_ascii_uppercase();
                        self.report = None;
                    }
                    settings_changed |= response.lost_focus();
                    ui.label(
                        RichText::new("默认 Q，不能占用 A-I")
                            .color(MUTED)
                            .size(12.0),
                    );
                    ui.add_space(12.0);
                    if ui
                        .checkbox(&mut self.settings.annotate_skipped_rows, "标注跳过原因")
                        .changed()
                    {
                        settings_changed = true;
                        self.report = None;
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        export = ui
                            .add_enabled(
                                self.report.is_some() && !self.is_analyzing(),
                                egui::Button::new("导出结果"),
                            )
                            .clicked();
                        run_analysis = ui
                            .add_enabled(
                                !self.is_analyzing(),
                                egui::Button::new(
                                    RichText::new(if self.is_analyzing() {
                                        "核对中"
                                    } else {
                                        "开始核对"
                                    })
                                    .color(Color32::WHITE),
                                )
                                .fill(ACCENT),
                            )
                            .clicked();
                    });
                });
            });

        if source_changed {
            self.report = None;
        }
        if settings_changed {
            self.save_settings();
        }
        if pick_source {
            self.pick_source();
        }
        if pick_output {
            self.pick_output();
        }
        if run_analysis {
            self.start_analysis(context);
        }
        if export {
            self.export_report();
        }

        ui.add_space(14.0);
        if let Some(report) = &self.report {
            let summary = &report.summary;
            Frame::new()
                .fill(SURFACE)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(6)
                .inner_margin(Margin::symmetric(14, 10))
                .show(ui, |ui| {
                    ui.columns(4, |columns| {
                        summary_item(&mut columns[0], "数据行", summary.total_rows, INK);
                        summary_item(&mut columns[1], "待修改", summary.changed_rows, WARNING);
                        summary_item(&mut columns[2], "一致", summary.unchanged_rows, ACCENT);
                        summary_item(&mut columns[3], "已跳过", summary.skipped_rows, MUTED);
                    });
                });
            ui.add_space(12.0);

            Frame::new()
                .fill(SURFACE)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(6)
                .inner_margin(Margin::same(12))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("核对明细").strong());
                        ui.add_space(8.0);
                        ui.add_sized(
                            [260.0, 32.0],
                            TextEdit::singleline(&mut self.preview_search)
                                .hint_text("搜索账户、行业代码或说明"),
                        );
                        for filter in RowFilter::ALL {
                            let selected = self.row_filter == filter;
                            if ui
                                .add(
                                    egui::Button::new(
                                        RichText::new(filter.label()).color(if selected {
                                            Color32::WHITE
                                        } else {
                                            INK
                                        }),
                                    )
                                    .selected(selected)
                                    .fill(if selected {
                                        ACCENT
                                    } else {
                                        SURFACE
                                    }),
                                )
                                .clicked()
                            {
                                self.row_filter = filter;
                            }
                        }
                    });
                    ui.add_space(6.0);
                    self.show_preview_table(ui);
                });
        } else {
            Frame::new()
                .fill(SURFACE)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(6)
                .inner_margin(Margin::same(24))
                .show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(36.0);
                        ui.label(RichText::new("尚无核对结果").color(MUTED).size(16.0));
                        ui.add_space(36.0);
                    });
                });
        }
    }

    fn show_preview_table(&self, ui: &mut egui::Ui) {
        let Some(report) = self.report.as_ref() else {
            return;
        };
        let query = self.preview_search.trim().to_lowercase();
        let filtered = report
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| self.row_filter.matches(row.status))
            .filter(|(_, row)| row_matches_query(row, &query))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();

        let available = ui.available_width().max(900.0);
        let widths = [
            48.0,
            (available * 0.15).clamp(132.0, 190.0),
            76.0,
            (available * 0.19).clamp(170.0, 230.0),
            (available * 0.12).clamp(110.0, 150.0),
            62.0,
            62.0,
            68.0,
        ];
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.horizontal(|ui| {
                table_header(ui, widths[0], "行号");
                table_header(ui, widths[1], "客户 / 账户");
                table_header(ui, widths[2], "E列代码");
                table_header(ui, widths[3], "行业层级");
                table_header(ui, widths[4], "匹配标准");
                table_header(ui, widths[5], "原 C 列");
                table_header(ui, widths[6], "核对值");
                table_header(ui, widths[7], "状态");
                table_header(ui, ui.available_width().max(130.0), "修改说明");
            });
        });
        ui.separator();

        if filtered.is_empty() {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("没有符合当前筛选条件的记录").color(MUTED));
            });
            return;
        }

        let table_height = ui.available_height().max(190.0);
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), table_height),
            Layout::top_down(Align::Min),
            |ui| {
                ScrollArea::vertical()
                    .id_salt("analysis_preview")
                    .auto_shrink([false, false])
                    .show_rows(ui, 28.0, filtered.len(), |ui, visible_range| {
                        for visible_index in visible_range {
                            let row = &report.rows[filtered[visible_index]];
                            ui.scope(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                ui.horizontal(|ui| {
                                    table_cell(ui, widths[0], &row.row_number.to_string(), MUTED);
                                    let account = if row.customer_id.trim().is_empty() {
                                        row.account_name.clone()
                                    } else {
                                        format!("{}  {}", row.customer_id, row.account_name)
                                    };
                                    table_cell(ui, widths[1], &account, INK);
                                    table_cell(ui, widths[2], &row.trusted_code, INK);
                                    table_cell(ui, widths[3], &row.industry_path, MUTED);
                                    let matched_rule = match (
                                        row.matched_rule_code.as_deref(),
                                        row.matched_rule_name.as_deref(),
                                    ) {
                                        (Some(code), Some(name)) => format!("{code} {name}"),
                                        (Some(code), None) => code.to_owned(),
                                        _ => String::new(),
                                    };
                                    table_cell(ui, widths[4], &matched_rule, INK);
                                    table_cell(ui, widths[5], &row.original_value, INK);
                                    table_cell(
                                        ui,
                                        widths[6],
                                        row.calculated_size.map(|size| size.as_str()).unwrap_or(""),
                                        INK,
                                    );
                                    let status_color = match row.status {
                                        RowStatus::Changed => WARNING,
                                        RowStatus::Unchanged => ACCENT,
                                        RowStatus::Skipped => MUTED,
                                    };
                                    table_cell(ui, widths[7], row.status.label(), status_color);
                                    table_cell(
                                        ui,
                                        ui.available_width().max(130.0),
                                        &row.annotation,
                                        MUTED,
                                    );
                                });
                            });
                        }
                    });
            },
        );
    }

    fn show_configuration_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("配置中心");
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let rules_selected = self.configuration_tab == ConfigurationTab::Rules;
            if ui
                .add(
                    egui::Button::new(RichText::new("划型标准").color(if rules_selected {
                        Color32::WHITE
                    } else {
                        INK
                    }))
                    .selected(rules_selected)
                    .fill(if rules_selected { ACCENT } else { SURFACE }),
                )
                .clicked()
            {
                self.configuration_tab = ConfigurationTab::Rules;
            }
            let categories_selected = self.configuration_tab == ConfigurationTab::Categories;
            if ui
                .add(
                    egui::Button::new(RichText::new("行业细类").color(if categories_selected {
                        Color32::WHITE
                    } else {
                        INK
                    }))
                    .selected(categories_selected)
                    .fill(if categories_selected { ACCENT } else { SURFACE }),
                )
                .clicked()
            {
                self.configuration_tab = ConfigurationTab::Categories;
            }
        });
        ui.add_space(8.0);

        match self.configuration_tab {
            ConfigurationTab::Rules => self.show_rules_page(ui),
            ConfigurationTab::Categories => self.show_categories_page(ui),
        }
    }

    fn show_rules_page(&mut self, ui: &mut egui::Ui) {
        let mut add_rule = false;
        let mut import_rules = false;
        let mut restore_rules = false;
        Frame::new()
            .fill(SURFACE)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(6)
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [300.0, 32.0],
                        TextEdit::singleline(&mut self.rule_search)
                            .hint_text("搜索行业名称、代码或指标"),
                    );
                    ui.label(
                        RichText::new(format!("共 {} 条", self.rules.rules.len())).color(MUTED),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        restore_rules = ui.button("恢复内置").clicked();
                        import_rules = ui.button("导入 XLSX").clicked();
                        add_rule = ui
                            .add(
                                egui::Button::new(RichText::new("新增标准").color(Color32::WHITE))
                                    .fill(ACCENT),
                            )
                            .clicked();
                    });
                });
                ui.add_space(8.0);
                self.show_rules_table(ui);
            });

        if add_rule {
            let id = self.rules.next_id();
            self.rule_editor = Some(RuleEditor {
                original_id: None,
                draft: IndustryRule {
                    id,
                    industry_name: String::new(),
                    industry_code: String::new(),
                    metrics: vec![default_metric_rule(MetricKind::Employees)],
                },
                error: None,
            });
        }
        if import_rules {
            self.import_rules();
        }
        if restore_rules {
            self.confirmation = Some(ConfirmAction::RestoreRules);
        }
    }

    fn show_rules_table(&mut self, ui: &mut egui::Ui) {
        let query = self.rule_search.trim().to_lowercase();
        let rows = self
            .rules
            .rules
            .iter()
            .filter(|rule| rule_matches_query(rule, &query))
            .cloned()
            .collect::<Vec<_>>();
        ui.horizontal(|ui| {
            table_header(ui, 86.0, "行业代码");
            table_header(ui, 210.0, "行业名称");
            table_header(
                ui,
                ui.available_width() - 128.0,
                "指标与阈值（大型 / 中型 / 小型）",
            );
            table_header(ui, 110.0, "操作");
        });
        ui.separator();
        if rows.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(24.0);
                ui.label(RichText::new("没有符合条件的划型标准").color(MUTED));
            });
            return;
        }

        let mut edit_id = None;
        let mut delete_id = None;
        let height = ui.available_height().max(260.0);
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), height),
            Layout::top_down(Align::Min),
            |ui| {
                ScrollArea::vertical()
                    .id_salt("rules_table")
                    .auto_shrink([false, false])
                    .show_rows(ui, 34.0, rows.len(), |ui, range| {
                        for index in range {
                            let rule = &rows[index];
                            ui.horizontal(|ui| {
                                table_cell(ui, 86.0, &rule.industry_code, INK);
                                table_cell(ui, 210.0, &rule.industry_name, INK);
                                let metric_text = rule
                                    .metrics
                                    .iter()
                                    .map(|metric| {
                                        format!(
                                            "{}: {} / {} / {} {}",
                                            metric.metric.label(),
                                            format_number(metric.large_min),
                                            format_number(metric.medium_min),
                                            format_number(metric.small_min),
                                            metric.metric.unit_label()
                                        )
                                    })
                                    .collect::<Vec<_>>()
                                    .join("；");
                                table_cell(
                                    ui,
                                    (ui.available_width() - 128.0).max(240.0),
                                    &metric_text,
                                    MUTED,
                                );
                                if ui.small_button("编辑").clicked() {
                                    edit_id = Some(rule.id);
                                }
                                if ui
                                    .add(
                                        egui::Button::new(RichText::new("删除").color(DANGER))
                                            .small(),
                                    )
                                    .clicked()
                                {
                                    delete_id = Some(rule.id);
                                }
                            });
                        }
                    });
            },
        );
        if let Some(id) = edit_id
            && let Some(rule) = self.rules.rules.iter().find(|rule| rule.id == id)
        {
            self.rule_editor = Some(RuleEditor {
                original_id: Some(id),
                draft: rule.clone(),
                error: None,
            });
        }
        if let Some(id) = delete_id {
            self.confirmation = Some(ConfirmAction::DeleteRule(id));
        }
    }

    fn show_categories_page(&mut self, ui: &mut egui::Ui) {
        let mut add_category = false;
        let mut import_categories = false;
        let mut restore_categories = false;
        Frame::new()
            .fill(SURFACE)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(6)
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [300.0, 32.0],
                        TextEdit::singleline(&mut self.category_search)
                            .hint_text("搜索任一级代码或名称"),
                    );
                    ui.label(
                        RichText::new(format!("共 {} 条", self.categories.categories.len()))
                            .color(MUTED),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        restore_categories = ui.button("恢复内置").clicked();
                        import_categories = ui.button("导入 XLS").clicked();
                        add_category = ui
                            .add(
                                egui::Button::new(RichText::new("新增细类").color(Color32::WHITE))
                                    .fill(ACCENT),
                            )
                            .clicked();
                    });
                });
                ui.add_space(8.0);
                self.show_categories_table(ui);
            });

        if add_category {
            self.category_editor = Some(CategoryEditor {
                original_id: None,
                draft: IndustryCategory {
                    id: self.categories.next_id(),
                    level1_code: String::new(),
                    level1_name: String::new(),
                    level2_code: String::new(),
                    level2_name: String::new(),
                    level3_code: String::new(),
                    level3_name: String::new(),
                    level4_code: String::new(),
                    level4_name: String::new(),
                },
                error: None,
            });
        }
        if import_categories {
            self.import_categories();
        }
        if restore_categories {
            self.confirmation = Some(ConfirmAction::RestoreCategories);
        }
    }

    fn show_categories_table(&mut self, ui: &mut egui::Ui) {
        let query = self.category_search.trim().to_lowercase();
        let filtered = self
            .categories
            .categories
            .iter()
            .enumerate()
            .filter(|(_, category)| category_matches_query(category, &query))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let available = ui.available_width();
        let widths = [
            (available * 0.13).max(112.0),
            (available * 0.18).max(150.0),
            (available * 0.22).max(180.0),
            (available * 0.25).max(210.0),
        ];
        ui.horizontal(|ui| {
            table_header(ui, widths[0], "一级分类");
            table_header(ui, widths[1], "二级分类");
            table_header(ui, widths[2], "三级分类");
            table_header(ui, widths[3], "四级细类");
            table_header(ui, 110.0, "操作");
        });
        ui.separator();
        if filtered.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(24.0);
                ui.label(RichText::new("没有符合条件的行业细类").color(MUTED));
            });
            return;
        }

        let mut edit_id = None;
        let mut delete_id = None;
        let height = ui.available_height().max(260.0);
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), height),
            Layout::top_down(Align::Min),
            |ui| {
                ScrollArea::vertical()
                    .id_salt("categories_table")
                    .auto_shrink([false, false])
                    .show_rows(ui, 32.0, filtered.len(), |ui, range| {
                        for visible_index in range {
                            let category = &self.categories.categories[filtered[visible_index]];
                            ui.horizontal(|ui| {
                                table_cell(
                                    ui,
                                    widths[0],
                                    &format!("{} {}", category.level1_code, category.level1_name),
                                    INK,
                                );
                                table_cell(
                                    ui,
                                    widths[1],
                                    &format!("{} {}", category.level2_code, category.level2_name),
                                    INK,
                                );
                                table_cell(
                                    ui,
                                    widths[2],
                                    &format!("{} {}", category.level3_code, category.level3_name),
                                    INK,
                                );
                                table_cell(
                                    ui,
                                    widths[3],
                                    &format!("{} {}", category.level4_code, category.level4_name),
                                    INK,
                                );
                                if ui.small_button("编辑").clicked() {
                                    edit_id = Some(category.id);
                                }
                                if ui
                                    .add(
                                        egui::Button::new(RichText::new("删除").color(DANGER))
                                            .small(),
                                    )
                                    .clicked()
                                {
                                    delete_id = Some(category.id);
                                }
                            });
                        }
                    });
            },
        );
        if let Some(id) = edit_id
            && let Some(category) = self
                .categories
                .categories
                .iter()
                .find(|category| category.id == id)
        {
            self.category_editor = Some(CategoryEditor {
                original_id: Some(id),
                draft: category.clone(),
                error: None,
            });
        }
        if let Some(id) = delete_id {
            self.confirmation = Some(ConfirmAction::DeleteCategory(id));
        }
    }

    fn import_rules(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("导入划型标准")
            .add_filter("Excel 工作簿", &["xlsx"])
            .pick_file()
        else {
            return;
        };
        match load_rules_from_path(&path) {
            Ok(rules) => {
                if let Err(error) = self.replace_rules(rules, "划型标准已导入并保存") {
                    self.set_notice(NoticeLevel::Error, error);
                }
            }
            Err(error) => {
                self.set_notice(NoticeLevel::Error, format!("标准导入失败：{error:#}"));
            }
        }
    }

    fn import_categories(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("导入行业分类代码注释")
            .add_filter("Excel 97-2003 工作簿", &["xls"])
            .add_filter("Excel 工作簿", &["xls", "xlsx"])
            .pick_file()
        else {
            return;
        };
        match load_categories_from_path(&path) {
            Ok(categories) => {
                if let Err(error) = self.replace_categories(categories, "行业细类已导入并保存")
                {
                    self.set_notice(NoticeLevel::Error, error);
                }
            }
            Err(error) => {
                self.set_notice(NoticeLevel::Error, format!("行业细类导入失败：{error:#}"));
            }
        }
    }

    fn show_rule_editor(&mut self, context: &egui::Context) {
        let Some(mut editor) = self.rule_editor.take() else {
            return;
        };
        let is_new = editor.original_id.is_none();
        let mut save_requested = false;
        let mut cancel_requested = false;
        let mut metric_to_remove = None;

        egui::Window::new(if is_new {
            "新增划型标准"
        } else {
            "编辑划型标准"
        })
        .id(egui::Id::new("rule_editor"))
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .default_width(650.0)
        .collapsible(false)
        .resizable(true)
        .show(context, |ui| {
            if let Some(error) = editor.error.as_ref() {
                form_error(ui, error);
                ui.add_space(8.0);
            }

            egui::Grid::new("rule_identity_fields")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("行业代码");
                    ui.add_sized(
                        [220.0, 32.0],
                        TextEdit::singleline(&mut editor.draft.industry_code)
                            .hint_text("例如 C、F51；兜底使用 *"),
                    );
                    ui.end_row();
                    ui.label("行业名称");
                    ui.add_sized(
                        [420.0, 32.0],
                        TextEdit::singleline(&mut editor.draft.industry_name),
                    );
                    ui.end_row();
                });

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("划型指标").strong());
                ui.label(RichText::new("阈值单位由指标决定").color(MUTED).size(12.0));
            });
            ui.add_space(4.0);

            for (index, metric) in editor.draft.metrics.iter_mut().enumerate() {
                Frame::new()
                    .fill(Color32::from_rgb(248, 249, 249))
                    .stroke(Stroke::new(1.0, BORDER))
                    .corner_radius(4)
                    .inner_margin(Margin::same(10))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("指标");
                            ComboBox::from_id_salt(("rule_metric_kind", editor.draft.id, index))
                                .selected_text(metric.metric.label())
                                .width(150.0)
                                .show_ui(ui, |ui| {
                                    for kind in MetricKind::ALL {
                                        ui.selectable_value(&mut metric.metric, kind, kind.label());
                                    }
                                });
                            ui.label(
                                RichText::new(format!(
                                    "来源 {} 列，单位 {}",
                                    metric.metric.source_column(),
                                    metric.metric.unit_label()
                                ))
                                .color(MUTED)
                                .size(12.0),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add(
                                        egui::Button::new(RichText::new("移除").color(DANGER))
                                            .small(),
                                    )
                                    .clicked()
                                {
                                    metric_to_remove = Some(index);
                                }
                            });
                        });
                        ui.add_space(4.0);
                        ui.columns(3, |columns| {
                            threshold_input(
                                &mut columns[0],
                                "大型下限",
                                &mut metric.large_min,
                                metric.metric.unit_label(),
                            );
                            threshold_input(
                                &mut columns[1],
                                "中型下限",
                                &mut metric.medium_min,
                                metric.metric.unit_label(),
                            );
                            threshold_input(
                                &mut columns[2],
                                "小型下限",
                                &mut metric.small_min,
                                metric.metric.unit_label(),
                            );
                        });
                    });
                ui.add_space(6.0);
            }
            if let Some(index) = metric_to_remove {
                editor.draft.metrics.remove(index);
                metric_to_remove = None;
            }

            if editor.draft.metrics.len() < MetricKind::ALL.len() {
                let next_kind = MetricKind::ALL.into_iter().find(|kind| {
                    !editor
                        .draft
                        .metrics
                        .iter()
                        .any(|metric| metric.metric == *kind)
                });
                if ui.button("添加指标").clicked()
                    && let Some(kind) = next_kind
                {
                    editor.draft.metrics.push(default_metric_rule(kind));
                }
            }

            ui.separator();
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                save_requested = ui
                    .add(
                        egui::Button::new(RichText::new("保存").color(Color32::WHITE)).fill(ACCENT),
                    )
                    .clicked();
                cancel_requested = ui.button("取消").clicked();
            });
        });

        if save_requested {
            editor.draft.industry_code = normalize_rule_code(&editor.draft.industry_code);
            editor.draft.industry_name = editor.draft.industry_name.trim().to_owned();
            if let Some(original_id) = editor.original_id {
                editor.draft.id = original_id;
            }

            let mut rules = self.rules.clone();
            let update_result = if let Some(original_id) = editor.original_id {
                if let Some(existing) = rules.rules.iter_mut().find(|rule| rule.id == original_id) {
                    *existing = editor.draft.clone();
                    Ok(())
                } else {
                    Err("要编辑的标准已不存在".to_owned())
                }
            } else {
                rules.rules.push(editor.draft.clone());
                Ok(())
            };

            match update_result.and_then(|()| self.replace_rules(rules, "划型标准已保存")) {
                Ok(()) => return,
                Err(error) => editor.error = Some(error),
            }
        }
        if !cancel_requested {
            self.rule_editor = Some(editor);
        }
    }

    fn show_category_editor(&mut self, context: &egui::Context) {
        let Some(mut editor) = self.category_editor.take() else {
            return;
        };
        let is_new = editor.original_id.is_none();
        let mut save_requested = false;
        let mut cancel_requested = false;

        egui::Window::new(if is_new {
            "新增行业细类"
        } else {
            "编辑行业细类"
        })
        .id(egui::Id::new("category_editor"))
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .default_size([680.0, 360.0])
        .collapsible(false)
        .resizable(false)
        .show(context, |ui| {
            if let Some(error) = editor.error.as_ref() {
                form_error(ui, error);
                ui.add_space(8.0);
            }

            egui::Grid::new("category_fields")
                .num_columns(3)
                .spacing([10.0, 8.0])
                .striped(true)
                .show(ui, |ui| {
                    ui.label(RichText::new("层级").strong());
                    ui.label(RichText::new("代码").strong());
                    ui.label(RichText::new("名称").strong());
                    ui.end_row();
                    category_level_fields(
                        ui,
                        "一级",
                        &mut editor.draft.level1_code,
                        &mut editor.draft.level1_name,
                        "A",
                    );
                    category_level_fields(
                        ui,
                        "二级",
                        &mut editor.draft.level2_code,
                        &mut editor.draft.level2_name,
                        "A01",
                    );
                    category_level_fields(
                        ui,
                        "三级",
                        &mut editor.draft.level3_code,
                        &mut editor.draft.level3_name,
                        "A011",
                    );
                    category_level_fields(
                        ui,
                        "四级",
                        &mut editor.draft.level4_code,
                        &mut editor.draft.level4_name,
                        "A0111",
                    );
                });

            ui.add_space(10.0);
            ui.separator();
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                save_requested = ui
                    .add(
                        egui::Button::new(RichText::new("保存").color(Color32::WHITE)).fill(ACCENT),
                    )
                    .clicked();
                cancel_requested = ui.button("取消").clicked();
            });
        });

        if save_requested {
            editor.draft.normalize();
            if let Some(original_id) = editor.original_id {
                editor.draft.id = original_id;
            }
            let original = editor.original_id.and_then(|id| {
                self.categories
                    .categories
                    .iter()
                    .find(|category| category.id == id)
                    .cloned()
            });
            let mut categories = self.categories.clone();
            harmonize_parent_names(
                &mut categories.categories,
                &mut editor.draft,
                original.as_ref(),
            );
            let update_result = if let Some(original_id) = editor.original_id {
                if let Some(existing) = categories
                    .categories
                    .iter_mut()
                    .find(|category| category.id == original_id)
                {
                    *existing = editor.draft.clone();
                    Ok(())
                } else {
                    Err("要编辑的行业细类已不存在".to_owned())
                }
            } else {
                categories.categories.push(editor.draft.clone());
                Ok(())
            };

            match update_result.and_then(|()| self.replace_categories(categories, "行业细类已保存"))
            {
                Ok(()) => return,
                Err(error) => editor.error = Some(error),
            }
        }
        if !cancel_requested {
            self.category_editor = Some(editor);
        }
    }

    fn show_confirmation(&mut self, context: &egui::Context) {
        let Some(action) = self.confirmation.take() else {
            return;
        };
        let (title, message, confirm_label) = match action {
            ConfirmAction::DeleteRule(id) => {
                let name = self
                    .rules
                    .rules
                    .iter()
                    .find(|rule| rule.id == id)
                    .map(|rule| format!("{} {}", rule.industry_code, rule.industry_name))
                    .unwrap_or_else(|| "该标准".to_owned());
                ("删除划型标准", format!("确定删除“{name}”吗？"), "删除")
            }
            ConfirmAction::DeleteCategory(id) => {
                let name = self
                    .categories
                    .categories
                    .iter()
                    .find(|category| category.id == id)
                    .map(|category| format!("{} {}", category.level4_code, category.level4_name))
                    .unwrap_or_else(|| "该行业细类".to_owned());
                ("删除行业细类", format!("确定删除“{name}”吗？"), "删除")
            }
            ConfirmAction::RestoreRules => (
                "恢复内置划型标准",
                "当前划型标准将被内置《标准 更新.xlsx》覆盖。".to_owned(),
                "恢复",
            ),
            ConfirmAction::RestoreCategories => (
                "恢复内置行业细类",
                "当前行业细类将被内置《行业分类代码注释.xls》覆盖。".to_owned(),
                "恢复",
            ),
        };
        let mut confirmed = false;
        let mut cancelled = false;
        egui::Window::new(title)
            .id(egui::Id::new("confirmation_dialog"))
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .default_width(420.0)
            .collapsible(false)
            .resizable(false)
            .show(context, |ui| {
                ui.label(message);
                ui.add_space(14.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    confirmed = ui
                        .add(
                            egui::Button::new(RichText::new(confirm_label).color(Color32::WHITE))
                                .fill(match action {
                                    ConfirmAction::DeleteRule(_)
                                    | ConfirmAction::DeleteCategory(_) => DANGER,
                                    _ => ACCENT,
                                }),
                        )
                        .clicked();
                    cancelled = ui.button("取消").clicked();
                });
            });

        if confirmed {
            match action {
                ConfirmAction::DeleteRule(id) => {
                    let mut rules = self.rules.clone();
                    rules.rules.retain(|rule| rule.id != id);
                    if let Err(error) = self.replace_rules(rules, "划型标准已删除") {
                        self.set_notice(NoticeLevel::Error, error);
                    }
                }
                ConfirmAction::DeleteCategory(id) => {
                    let mut categories = self.categories.clone();
                    categories.categories.retain(|category| category.id != id);
                    if let Err(error) = self.replace_categories(categories, "行业细类已删除")
                    {
                        self.set_notice(NoticeLevel::Error, error);
                    }
                }
                ConfirmAction::RestoreRules => match load_default_rules() {
                    Ok(rules) => {
                        if let Err(error) = self.replace_rules(rules, "已恢复内置划型标准")
                        {
                            self.set_notice(NoticeLevel::Error, error);
                        }
                    }
                    Err(error) => {
                        self.set_notice(NoticeLevel::Error, format!("内置标准载入失败：{error:#}"))
                    }
                },
                ConfirmAction::RestoreCategories => match load_default_categories() {
                    Ok(categories) => {
                        if let Err(error) =
                            self.replace_categories(categories, "已恢复内置行业细类")
                        {
                            self.set_notice(NoticeLevel::Error, error);
                        }
                    }
                    Err(error) => self.set_notice(
                        NoticeLevel::Error,
                        format!("内置行业细类载入失败：{error:#}"),
                    ),
                },
            }
        } else if !cancelled {
            self.confirmation = Some(action);
        }
    }
}

impl eframe::App for IndustryCheckApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_analysis();
        if self.is_analyzing() {
            context.request_repaint_after(Duration::from_millis(150));
        }

        self.show_navigation(context);
        egui::CentralPanel::default()
            .frame(Frame::new().fill(PAGE_BG).inner_margin(Margin::same(18)))
            .show(context, |ui| {
                self.show_notice(ui);
                match self.page {
                    Page::DataCheck => self.show_data_page(ui, context),
                    Page::Configuration => self.show_configuration_page(ui),
                }
            });

        self.show_rule_editor(context);
        self.show_category_editor(context);
        self.show_confirmation(context);
    }
}

fn summary_item(ui: &mut egui::Ui, label: &str, value: usize, color: Color32) {
    ui.vertical(|ui| {
        ui.label(
            RichText::new(value.to_string())
                .color(color)
                .strong()
                .size(22.0),
        );
        ui.label(RichText::new(label).color(MUTED).size(12.0));
    });
}

fn table_header(ui: &mut egui::Ui, width: f32, text: &str) {
    ui.add_sized(
        [width.max(0.0), 24.0],
        egui::Label::new(RichText::new(text).color(MUTED).strong().size(12.0)).truncate(),
    )
    .on_hover_text(text);
}

fn table_cell(ui: &mut egui::Ui, width: f32, text: &str, color: Color32) {
    ui.add_sized(
        [width.max(0.0), 28.0],
        egui::Label::new(RichText::new(text).color(color).size(13.0)).truncate(),
    )
    .on_hover_text(text);
}

fn row_matches_query(row: &CheckRow, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let calculated = row.calculated_size.map(|size| size.as_str()).unwrap_or("");
    [
        row.customer_id.as_str(),
        row.account_name.as_str(),
        row.trusted_code.as_str(),
        row.industry_path.as_str(),
        row.matched_rule_code.as_deref().unwrap_or(""),
        row.matched_rule_name.as_deref().unwrap_or(""),
        row.original_value.as_str(),
        calculated,
        row.status.label(),
        row.annotation.as_str(),
    ]
    .iter()
    .any(|value| value.to_lowercase().contains(query))
}

fn rule_matches_query(rule: &IndustryRule, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    rule.industry_code.to_lowercase().contains(query)
        || rule.industry_name.to_lowercase().contains(query)
        || rule
            .metrics
            .iter()
            .any(|metric| metric.metric.label().to_lowercase().contains(query))
}

fn category_matches_query(category: &IndustryCategory, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    [
        category.level1_code.as_str(),
        category.level1_name.as_str(),
        category.level2_code.as_str(),
        category.level2_name.as_str(),
        category.level3_code.as_str(),
        category.level3_name.as_str(),
        category.level4_code.as_str(),
        category.level4_name.as_str(),
    ]
    .iter()
    .any(|value| value.to_lowercase().contains(query))
}

#[derive(Clone, Copy)]
enum ParentLevel {
    One,
    Two,
    Three,
}

fn harmonize_parent_names(
    categories: &mut [IndustryCategory],
    draft: &mut IndustryCategory,
    original: Option<&IndustryCategory>,
) {
    for level in [ParentLevel::One, ParentLevel::Two, ParentLevel::Three] {
        let draft_code = parent_code(draft, level).to_owned();
        let draft_name = parent_name(draft, level).to_owned();
        let edits_same_parent = original.is_some_and(|original| {
            parent_code(original, level) == draft_code && parent_name(original, level) != draft_name
        });

        if edits_same_parent {
            for category in categories.iter_mut() {
                if parent_code(category, level) == draft_code {
                    set_parent_name(category, level, &draft_name);
                }
            }
        } else if let Some(existing_name) = categories
            .iter()
            .find(|category| parent_code(category, level) == draft_code)
            .map(|category| parent_name(category, level).to_owned())
        {
            set_parent_name(draft, level, &existing_name);
        }
    }
}

fn parent_code(category: &IndustryCategory, level: ParentLevel) -> &str {
    match level {
        ParentLevel::One => &category.level1_code,
        ParentLevel::Two => &category.level2_code,
        ParentLevel::Three => &category.level3_code,
    }
}

fn parent_name(category: &IndustryCategory, level: ParentLevel) -> &str {
    match level {
        ParentLevel::One => &category.level1_name,
        ParentLevel::Two => &category.level2_name,
        ParentLevel::Three => &category.level3_name,
    }
}

fn set_parent_name(category: &mut IndustryCategory, level: ParentLevel, name: &str) {
    match level {
        ParentLevel::One => category.level1_name = name.to_owned(),
        ParentLevel::Two => category.level2_name = name.to_owned(),
        ParentLevel::Three => category.level3_name = name.to_owned(),
    }
}

fn default_metric_rule(metric: MetricKind) -> MetricRule {
    match metric {
        MetricKind::Employees => MetricRule {
            metric,
            large_min: 1_000.0,
            medium_min: 300.0,
            small_min: 20.0,
        },
        MetricKind::Assets | MetricKind::Revenue => MetricRule {
            metric,
            large_min: 40_000.0,
            medium_min: 2_000.0,
            small_min: 300.0,
        },
    }
}

fn threshold_input(ui: &mut egui::Ui, label: &str, value: &mut f64, unit: &str) {
    ui.vertical(|ui| {
        ui.label(RichText::new(label).color(MUTED).size(12.0));
        ui.horizontal(|ui| {
            ui.add(
                egui::DragValue::new(value)
                    .range(0.0..=1_000_000_000_000_f64)
                    .speed(10.0),
            );
            ui.label(RichText::new(unit).color(MUTED).size(12.0));
        });
    });
}

fn category_level_fields(
    ui: &mut egui::Ui,
    level: &str,
    code: &mut String,
    name: &mut String,
    code_hint: &str,
) {
    ui.label(level);
    ui.add_sized(
        [110.0, 32.0],
        TextEdit::singleline(code).hint_text(code_hint),
    );
    ui.add_sized([430.0, 32.0], TextEdit::singleline(name));
    ui.end_row();
}

fn form_error(ui: &mut egui::Ui, message: &str) {
    Frame::new()
        .fill(Color32::from_rgb(251, 236, 235))
        .stroke(Stroke::new(1.0, Color32::from_rgb(220, 166, 162)))
        .corner_radius(4)
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.label(RichText::new(message).color(DANGER));
        });
}

fn format_number(value: f64) -> String {
    if value.fract().abs() < f64::EPSILON {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned()
    }
}

fn install_cjk_font(context: &egui::Context) -> Result<(), String> {
    const CANDIDATES: &[&str] = &[
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\simhei.ttf",
        "/System/Library/Fonts/PingFang.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJKsc-Regular.otf",
    ];
    let (path, bytes) = CANDIDATES
        .iter()
        .find_map(|candidate| {
            std::fs::read(candidate)
                .ok()
                .map(|bytes| ((*candidate).to_owned(), bytes))
        })
        .ok_or_else(|| "未找到可用的中文字体，中文显示可能不完整".to_owned())?;

    let font_name = "system-cjk".to_owned();
    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert(font_name.clone(), Arc::new(FontData::from_owned(bytes)));
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        if let Some(names) = fonts.families.get_mut(&family) {
            names.insert(0, font_name.clone());
        }
    }
    context.set_fonts(fonts);
    tracing::info!(font = %path, "loaded CJK font");
    Ok(())
}

fn configure_style(context: &egui::Context) {
    let mut visuals = egui::Visuals::light();
    visuals.panel_fill = PAGE_BG;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = Color32::from_rgb(238, 241, 242);
    visuals.faint_bg_color = Color32::from_rgb(248, 249, 249);
    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(4);
    visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(4);
    visuals.widgets.active.corner_radius = egui::CornerRadius::same(4);
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    context.set_visuals(visuals);

    let mut style = (*context.style()).clone();
    style.spacing.item_spacing = Vec2::new(8.0, 8.0);
    style.spacing.button_padding = Vec2::new(12.0, 7.0);
    style.spacing.interact_size.y = 32.0;
    style.text_styles.insert(
        TextStyle::Heading,
        FontId::new(22.0, FontFamily::Proportional),
    );
    style
        .text_styles
        .insert(TextStyle::Body, FontId::new(14.0, FontFamily::Proportional));
    style.text_styles.insert(
        TextStyle::Button,
        FontId::new(14.0, FontFamily::Proportional),
    );
    context.set_style(style);
}

fn non_empty_path(value: &str) -> Option<PathBuf> {
    (!value.trim().is_empty()).then(|| PathBuf::from(value.trim()))
}

fn existing_parent(value: &str) -> Option<&Path> {
    Path::new(value.trim())
        .parent()
        .filter(|path| path.exists())
}

fn ensure_extension(mut path: PathBuf, extension: &str) -> PathBuf {
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_none_or(|value| !value.eq_ignore_ascii_case(extension))
    {
        path.set_extension(extension);
    }
    path
}
