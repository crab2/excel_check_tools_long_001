use crate::hierarchy::HierarchyIndex;
use crate::model::{CompanySize, MetricValues, RuleSet};
use anyhow::{Context, Result, anyhow};
use calamine::{Data, Reader, open_workbook_auto};
use std::path::{Path, PathBuf};

pub const DEFAULT_ANNOTATION_COLUMN: &str = "Q";

#[derive(Debug, Clone)]
pub struct CheckOptions {
    pub source_path: PathBuf,
    pub annotation_column: String,
    pub annotate_skipped_rows: bool,
}

impl CheckOptions {
    pub fn new(source_path: impl Into<PathBuf>) -> Self {
        Self {
            source_path: source_path.into(),
            annotation_column: DEFAULT_ANNOTATION_COLUMN.to_owned(),
            annotate_skipped_rows: true,
        }
    }

    pub fn validate(&self) -> Result<usize> {
        if !self.source_path.exists() {
            return Err(anyhow!("源数据文件不存在：{}", self.source_path.display()));
        }
        if self
            .source_path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("xlsx"))
        {
            return Err(anyhow!("源数据必须是 .xlsx 文件"));
        }
        let annotation_index = column_label_to_index(&self.annotation_column)?;
        if annotation_index <= 9 {
            return Err(anyhow!(
                "标注列不能占用源数据 A-I 列；请选择 J 或之后的空列"
            ));
        }
        Ok(annotation_index)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowStatus {
    Changed,
    Unchanged,
    Skipped,
}

impl RowStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Changed => "待修改",
            Self::Unchanged => "一致",
            Self::Skipped => "已跳过",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CheckRow {
    pub row_number: u32,
    pub customer_id: String,
    pub account_name: String,
    pub trusted_code: String,
    pub industry_path: String,
    pub matched_rule_code: Option<String>,
    pub matched_rule_name: Option<String>,
    pub original_value: String,
    pub calculated_size: Option<CompanySize>,
    pub status: RowStatus,
    pub annotation: String,
}

#[derive(Debug, Clone, Default)]
pub struct AnalysisSummary {
    pub total_rows: usize,
    pub changed_rows: usize,
    pub unchanged_rows: usize,
    pub skipped_rows: usize,
}

#[derive(Debug, Clone)]
pub struct AnalysisReport {
    pub source_path: PathBuf,
    pub sheet_name: String,
    pub annotation_column: String,
    pub annotate_skipped_rows: bool,
    pub rows: Vec<CheckRow>,
    pub summary: AnalysisSummary,
}

pub fn analyze_workbook(
    options: &CheckOptions,
    hierarchy: &HierarchyIndex,
    rules: &RuleSet,
) -> Result<AnalysisReport> {
    options.validate()?;
    rules.validate().map_err(anyhow::Error::msg)?;

    let mut workbook = open_workbook_auto(&options.source_path)
        .with_context(|| format!("无法打开源数据：{}", options.source_path.display()))?;
    let sheet_name = workbook
        .sheet_names()
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("源数据工作簿没有工作表"))?;
    let range = workbook
        .worksheet_range(&sheet_name)
        .with_context(|| format!("无法读取源数据工作表：{sheet_name}"))?;
    validate_source_headers(range.rows().next())?;

    let mut rows = Vec::with_capacity(range.height().saturating_sub(1));
    let mut summary = AnalysisSummary::default();

    for (zero_based_row, row) in range.rows().enumerate().skip(1) {
        if row.iter().all(|cell| matches!(cell, Data::Empty)) {
            continue;
        }
        let row_number = zero_based_row as u32 + 1;
        let customer_id = cell_text(row.first());
        let account_name = cell_text(row.get(1));
        let original_value = cell_text(row.get(2));
        let trusted_code = cell_text(row.get(4));
        let path = hierarchy.resolve(&trusted_code);
        let industry_path = path.display();

        let (matched_rule_code, matched_rule_name, calculated_size, status, annotation) =
            if trusted_code.trim().is_empty() {
                (
                    None,
                    None,
                    None,
                    RowStatus::Skipped,
                    "跳过：E列客户行业四级为空".to_owned(),
                )
            } else if !path.found_in_reference {
                (
                    None,
                    None,
                    None,
                    RowStatus::Skipped,
                    format!("跳过：行业细类配置中未找到 {trusted_code}"),
                )
            } else if let Some(rule) = rules.find_for_codes(&path.codes) {
                let values = MetricValues {
                    employees: cell_number(row.get(6)),
                    assets: cell_number(row.get(7)).map(|value| value / 10_000.0),
                    revenue: cell_number(row.get(8)).map(|value| value / 10_000.0),
                };
                match rule.classify(&values) {
                    Ok(size) => {
                        let changed = CompanySize::parse(&original_value) != Some(size);
                        let status = if changed {
                            RowStatus::Changed
                        } else {
                            RowStatus::Unchanged
                        };
                        let annotation = if changed {
                            format!(
                                "C列：{} -> {}（匹配标准 {} {}）",
                                display_or_empty(&original_value),
                                size,
                                rule.industry_code,
                                rule.industry_name
                            )
                        } else {
                            String::new()
                        };
                        (
                            Some(rule.industry_code.clone()),
                            Some(rule.industry_name.clone()),
                            Some(size),
                            status,
                            annotation,
                        )
                    }
                    Err(missing_metrics) => {
                        let labels = missing_metrics
                            .iter()
                            .map(|metric| {
                                format!("{}({}列)", metric.label(), metric.source_column())
                            })
                            .collect::<Vec<_>>()
                            .join("、");
                        (
                            Some(rule.industry_code.clone()),
                            Some(rule.industry_name.clone()),
                            None,
                            RowStatus::Skipped,
                            format!("跳过：缺少{labels}"),
                        )
                    }
                }
            } else {
                (
                    None,
                    None,
                    None,
                    RowStatus::Skipped,
                    format!("跳过：{} 未匹配到划型标准", path.full_code),
                )
            };

        summary.total_rows += 1;
        match status {
            RowStatus::Changed => summary.changed_rows += 1,
            RowStatus::Unchanged => summary.unchanged_rows += 1,
            RowStatus::Skipped => summary.skipped_rows += 1,
        }
        rows.push(CheckRow {
            row_number,
            customer_id,
            account_name,
            trusted_code,
            industry_path,
            matched_rule_code,
            matched_rule_name,
            original_value,
            calculated_size,
            status,
            annotation,
        });
    }

    Ok(AnalysisReport {
        source_path: options.source_path.clone(),
        sheet_name,
        annotation_column: options.annotation_column.trim().to_ascii_uppercase(),
        annotate_skipped_rows: options.annotate_skipped_rows,
        rows,
        summary,
    })
}

fn validate_source_headers(header: Option<&[Data]>) -> Result<()> {
    let header = header.ok_or_else(|| anyhow!("源数据工作表为空"))?;
    let expected = [
        (1_usize, "账号名称"),
        (2, "贷款类型"),
        (4, "客户行业四级"),
        (6, "员工"),
        (7, "资产总额"),
        (8, "营业收入"),
    ];
    let mismatches = expected
        .iter()
        .filter_map(|(index, keyword)| {
            let actual = cell_text(header.get(*index));
            (!actual.contains(keyword)).then(|| {
                format!(
                    "{}列应包含“{}”，实际为“{}”",
                    column_index_to_label(index + 1),
                    keyword,
                    display_or_empty(&actual)
                )
            })
        })
        .collect::<Vec<_>>();
    if mismatches.is_empty() {
        Ok(())
    } else {
        Err(anyhow!("源数据表头与预期不符：{}", mismatches.join("；")))
    }
}

fn cell_text(value: Option<&Data>) -> String {
    match value {
        Some(Data::Float(number)) if number.fract() == 0.0 => format!("{number:.0}"),
        Some(Data::Int(number)) => number.to_string(),
        Some(Data::Empty) | None => String::new(),
        Some(cell) => cell.to_string().trim().to_owned(),
    }
}

fn cell_number(value: Option<&Data>) -> Option<f64> {
    match value? {
        Data::Int(number) => Some(*number as f64),
        Data::Float(number) => Some(*number),
        Data::String(text) => {
            let normalized = text.trim().replace([',', '，', ' '], "");
            (!normalized.is_empty())
                .then(|| normalized.parse::<f64>().ok())
                .flatten()
        }
        _ => None,
    }
}

fn display_or_empty(value: &str) -> &str {
    if value.trim().is_empty() {
        "空"
    } else {
        value.trim()
    }
}

pub fn column_label_to_index(label: &str) -> Result<usize> {
    let label = label.trim().to_ascii_uppercase();
    if label.is_empty() || label.len() > 3 || !label.chars().all(|ch| ch.is_ascii_alphabetic()) {
        return Err(anyhow!("标注列应填写 Excel 列名，例如 Q 或 AA"));
    }
    let mut index = 0_usize;
    for character in label.chars() {
        index = index * 26 + (character as usize - 'A' as usize + 1);
    }
    if index > 16_384 {
        return Err(anyhow!("标注列不能超过 Excel 最大列 XFD"));
    }
    Ok(index)
}

pub fn column_index_to_label(mut index: usize) -> String {
    let mut label = String::new();
    while index > 0 {
        index -= 1;
        label.push((b'A' + (index % 26) as u8) as char);
        index /= 26;
    }
    label.chars().rev().collect()
}

pub fn default_output_path(source_path: &Path) -> PathBuf {
    let parent = source_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = source_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("核对结果");
    parent.join(format!("{stem}_核对结果.xlsx"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hierarchy::load_default_categories;
    use crate::rules::load_default_rules;

    #[test]
    fn excel_column_labels_round_trip() {
        for (label, index) in [("J", 10), ("Q", 17), ("AA", 27), ("XFD", 16_384)] {
            assert_eq!(column_label_to_index(label).unwrap(), index);
            assert_eq!(column_index_to_label(index), label);
        }
    }

    #[test]
    fn real_fixture_produces_expected_changes_and_skips() {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/对公贷-.xlsx");
        if !source.exists() {
            return;
        }
        let categories = load_default_categories().unwrap();
        let hierarchy = HierarchyIndex::from_categories(&categories).unwrap();
        let rules = load_default_rules().unwrap();
        let report = analyze_workbook(&CheckOptions::new(source), &hierarchy, &rules).unwrap();
        assert_eq!(report.summary.total_rows, 1_256);
        assert_eq!(report.summary.changed_rows, 2);
        assert_eq!(report.summary.skipped_rows, 155);
        let changed_rows = report
            .rows
            .iter()
            .filter(|row| row.status == RowStatus::Changed)
            .map(|row| row.row_number)
            .collect::<Vec<_>>();
        assert_eq!(changed_rows, vec![627, 955]);
    }
}
