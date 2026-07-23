pub mod checker;
pub mod export;
pub mod hierarchy;
pub mod model;
mod persistence;
pub mod rules;
pub mod settings;

pub use checker::{
    AnalysisReport, CheckOptions, CheckRow, RowStatus, analyze_workbook, default_output_path,
};
pub use export::export_checked_workbook;
pub use hierarchy::{
    CategoryRepository, CategorySet, HierarchyIndex, IndustryCategory, IndustryPath,
    load_categories_from_path, load_default_categories,
};
pub use model::{Classification, CompanySize, IndustryRule, MetricKind, MetricRule, RuleSet};
pub use rules::{RuleRepository, load_default_rules, load_rules_from_path};
pub use settings::{AppSettings, SettingsRepository};
