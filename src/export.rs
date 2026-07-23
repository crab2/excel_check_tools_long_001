use crate::checker::{AnalysisReport, RowStatus, column_index_to_label, column_label_to_index};
use anyhow::{Context, Result, anyhow};
use std::path::Path;

const ANNOTATION_HEADER: &str = "核对修改说明";
const INCOMPLETE_ROW_FILL: &str = "FFFFF2CC";
const FAILED_ROW_FILL: &str = "FFF4CCCC";

pub fn export_checked_workbook(
    report: &AnalysisReport,
    output_path: impl AsRef<Path>,
) -> Result<()> {
    let output_path = output_path.as_ref();
    if same_path(&report.source_path, output_path) {
        return Err(anyhow!("输出文件不能覆盖源数据，请选择新的 .xlsx 文件"));
    }
    if output_path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("xlsx"))
    {
        return Err(anyhow!("输出文件必须使用 .xlsx 扩展名"));
    }
    if output_path.exists() {
        return Err(anyhow!(
            "输出文件已存在，请选择新的 .xlsx 文件：{}",
            output_path.display()
        ));
    }
    if let Some(parent) = output_path.parent()
        && !parent.exists()
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("无法创建输出目录：{}", parent.display()))?;
    }

    let annotation_index = column_label_to_index(&report.annotation_column)?;
    if annotation_index <= 9 {
        return Err(anyhow!(
            "标注列不能占用源数据 A-I 列；请选择 J 或之后的空列"
        ));
    }
    let mut workbook = umya_spreadsheet::reader::xlsx::read(&report.source_path)
        .with_context(|| format!("无法读取源数据格式：{}", report.source_path.display()))?;
    let worksheet = workbook
        .get_sheet_by_name_mut(&report.sheet_name)
        .ok_or_else(|| anyhow!("输出时找不到工作表：{}", report.sheet_name))?;

    ensure_annotation_cells_available(worksheet, report, annotation_index as u32)?;

    worksheet
        .get_cell_mut((annotation_index as u32, 1_u32))
        .set_value(ANNOTATION_HEADER);
    let highlight_end_column = worksheet.get_highest_column().max(annotation_index as u32);

    for row in &report.rows {
        if row.status == RowStatus::Changed {
            let calculated = row
                .calculated_size
                .ok_or_else(|| anyhow!("第 {} 行缺少计算结果", row.row_number))?;
            worksheet
                .get_cell_mut((3_u32, row.row_number))
                .set_value(calculated.as_str());
            worksheet
                .get_cell_mut((annotation_index as u32, row.row_number))
                .set_value(&row.annotation);
        } else if matches!(row.status, RowStatus::Incomplete | RowStatus::Failed)
            && report.annotate_skipped_rows
        {
            worksheet
                .get_cell_mut((annotation_index as u32, row.row_number))
                .set_value(&row.annotation);
        }

        let fill = match row.status {
            RowStatus::Incomplete => Some(INCOMPLETE_ROW_FILL),
            RowStatus::Failed => Some(FAILED_ROW_FILL),
            RowStatus::Changed | RowStatus::Unchanged => None,
        };
        if let Some(fill) = fill {
            highlight_row(worksheet, row.row_number, highlight_end_column, fill);
        }
    }

    expand_auto_filter(worksheet, annotation_index as u32);

    umya_spreadsheet::writer::xlsx::write(&workbook, output_path)
        .with_context(|| format!("无法写入核对结果：{}", output_path.display()))
}

fn ensure_annotation_cells_available(
    worksheet: &umya_spreadsheet::Worksheet,
    report: &AnalysisReport,
    annotation_index: u32,
) -> Result<()> {
    let mut conflicts = Vec::new();
    let annotation_label = column_index_to_label(annotation_index as usize);

    check_annotation_conflict(
        worksheet,
        annotation_index,
        1,
        ANNOTATION_HEADER,
        &annotation_label,
        &mut conflicts,
    );
    for row in &report.rows {
        let will_annotate = row.status == RowStatus::Changed
            || (matches!(row.status, RowStatus::Incomplete | RowStatus::Failed)
                && report.annotate_skipped_rows);
        if will_annotate {
            check_annotation_conflict(
                worksheet,
                annotation_index,
                row.row_number,
                &row.annotation,
                &annotation_label,
                &mut conflicts,
            );
        }
    }

    if conflicts.is_empty() {
        return Ok(());
    }

    let displayed = conflicts.iter().take(8).cloned().collect::<Vec<_>>();
    let remainder = conflicts.len().saturating_sub(displayed.len());
    let suffix = if remainder == 0 {
        String::new()
    } else {
        format!(" 等 {} 个单元格", conflicts.len())
    };
    Err(anyhow!(
        "标注列已有内容，导出已取消：{}{}；请清空这些单元格或更换标注列",
        displayed.join("、"),
        suffix
    ))
}

fn highlight_row(
    worksheet: &mut umya_spreadsheet::Worksheet,
    row_number: u32,
    end_column: u32,
    fill: &str,
) {
    for column in 1..=end_column {
        worksheet
            .get_style_mut((column, row_number))
            .set_background_color(fill);
    }
}

fn check_annotation_conflict(
    worksheet: &umya_spreadsheet::Worksheet,
    annotation_index: u32,
    row_number: u32,
    new_value: &str,
    annotation_label: &str,
    conflicts: &mut Vec<String>,
) {
    let Some(cell) = worksheet.get_cell((annotation_index, row_number)) else {
        return;
    };
    let existing = cell.get_value();
    if !existing.trim().is_empty() && existing.as_ref() != new_value {
        conflicts.push(format!("{annotation_label}{row_number}"));
    }
}

fn expand_auto_filter(worksheet: &mut umya_spreadsheet::Worksheet, annotation_index: u32) {
    let Some(end_column) = worksheet
        .get_auto_filter_mut()
        .and_then(|filter| filter.get_range_mut().get_coordinate_end_col_mut())
    else {
        return;
    };
    if *end_column.get_num() < annotation_index {
        end_column.set_num(annotation_index);
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checker::{AnalysisSummary, CheckOptions, CheckRow, analyze_workbook};
    use crate::hierarchy::{HierarchyIndex, load_default_categories};
    use crate::model::CompanySize;
    use crate::rules::load_default_rules;
    use calamine::{Data, Range, Reader, open_workbook_auto};
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    fn real_fixture_exports_only_expected_cells_and_preserves_source() {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/对公贷-.xlsx");
        let source_before = fs::read(&source).unwrap();
        let categories = load_default_categories().unwrap();
        let hierarchy = HierarchyIndex::from_categories(&categories).unwrap();
        let rules = load_default_rules().unwrap();
        let report = analyze_workbook(&CheckOptions::new(&source), &hierarchy, &rules).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("对公贷-核对结果.xlsx");

        export_checked_workbook(&report, &output).unwrap();

        assert_eq!(fs::read(&source).unwrap(), source_before);
        let source_range = read_first_sheet(&source);
        let output_range = read_first_sheet(&output);
        assert_eq!(source_range.height(), 1_257);
        assert_eq!(output_range.height(), source_range.height());

        let changed_rows = (1..source_range.height())
            .filter(|row| source_range.get((*row, 2)) != output_range.get((*row, 2)))
            .map(|row| row as u32 + 1)
            .collect::<Vec<_>>();
        let expected_changed_rows = report
            .rows
            .iter()
            .filter(|row| row.status == RowStatus::Changed)
            .map(|row| row.row_number)
            .collect::<Vec<_>>();
        assert_eq!(changed_rows, expected_changed_rows);

        for row in 0..source_range.height() {
            for column in 0..source_range.width() {
                if column != 2 {
                    assert_eq!(
                        output_range.get((row, column)),
                        source_range.get((row, column)),
                        "unexpected change at row {}, column {}",
                        row + 1,
                        column + 1
                    );
                }
            }
        }

        assert_eq!(cell_text(output_range.get((0, 16))), ANNOTATION_HEADER);
        let mut written_exceptions = 0;
        for row in &report.rows {
            let expected = match row.status {
                RowStatus::Changed => row.annotation.as_str(),
                RowStatus::Incomplete | RowStatus::Failed if report.annotate_skipped_rows => {
                    written_exceptions += 1;
                    row.annotation.as_str()
                }
                RowStatus::Unchanged | RowStatus::Incomplete | RowStatus::Failed => "",
            };
            assert_eq!(
                cell_text(output_range.get(((row.row_number - 1) as usize, 16))),
                expected,
                "unexpected annotation at Q{}",
                row.row_number
            );
        }
        assert_eq!(
            written_exceptions,
            report.summary.incomplete_rows + report.summary.failed_rows
        );
        assert!(cell_text(output_range.get((626, 16))).starts_with("C列："));
        assert!(cell_text(output_range.get((954, 16))).starts_with("C列："));

        let output_book = umya_spreadsheet::reader::xlsx::read(&output).unwrap();
        let output_sheet = output_book.get_sheet_by_name(&report.sheet_name).unwrap();
        assert_eq!(
            output_sheet
                .get_auto_filter()
                .unwrap()
                .get_range()
                .get_range(),
            "A1:Q1257"
        );
    }

    #[test]
    fn export_rejects_existing_output_and_annotation_conflicts() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.xlsx");
        let output = directory.path().join("output.xlsx");
        let mut workbook = umya_spreadsheet::new_file();
        workbook
            .get_sheet_by_name_mut("Sheet1")
            .unwrap()
            .get_cell_mut("Q2")
            .set_value("人工备注");
        umya_spreadsheet::writer::xlsx::write(&workbook, &source).unwrap();
        let report = sample_report(source.clone());

        let error = export_checked_workbook(&report, &output).unwrap_err();
        assert!(error.to_string().contains("Q2"));
        assert!(!output.exists());

        fs::write(&output, b"existing output").unwrap();
        let error = export_checked_workbook(&report, &output).unwrap_err();
        assert!(error.to_string().contains("输出文件已存在"));
        assert_eq!(fs::read(&output).unwrap(), b"existing output");
    }

    #[test]
    fn export_highlights_incomplete_and_failed_rows_without_changing_classification() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.xlsx");
        let output = directory.path().join("output.xlsx");
        let mut workbook = umya_spreadsheet::new_file();
        let worksheet = workbook.get_sheet_by_name_mut("Sheet1").unwrap();
        worksheet.get_cell_mut("C2").set_value("中型");
        worksheet.get_cell_mut("C3").set_value("小型");
        umya_spreadsheet::writer::xlsx::write(&workbook, &source).unwrap();
        let report = AnalysisReport {
            source_path: source,
            sheet_name: "Sheet1".to_owned(),
            annotation_column: "Q".to_owned(),
            annotate_skipped_rows: true,
            rows: vec![
                CheckRow {
                    row_number: 2,
                    customer_id: String::new(),
                    account_name: String::new(),
                    trusted_code: "C3833".to_owned(),
                    industry_path: String::new(),
                    matched_rule_code: Some("C".to_owned()),
                    matched_rule_name: Some("制造业".to_owned()),
                    original_value: "中型".to_owned(),
                    calculated_size: Some(CompanySize::Small),
                    status: RowStatus::Incomplete,
                    annotation: "单一字段判断不准确：数据不完整".to_owned(),
                },
                CheckRow {
                    row_number: 3,
                    customer_id: String::new(),
                    account_name: String::new(),
                    trusted_code: String::new(),
                    industry_path: String::new(),
                    matched_rule_code: None,
                    matched_rule_name: None,
                    original_value: "小型".to_owned(),
                    calculated_size: None,
                    status: RowStatus::Failed,
                    annotation: "处理失败：行业代码为空".to_owned(),
                },
            ],
            summary: AnalysisSummary {
                total_rows: 2,
                changed_rows: 0,
                unchanged_rows: 0,
                incomplete_rows: 1,
                failed_rows: 1,
            },
        };

        export_checked_workbook(&report, &output).unwrap();

        let output_book = umya_spreadsheet::reader::xlsx::read(&output).unwrap();
        let output_sheet = output_book.get_sheet_by_name("Sheet1").unwrap();
        assert_eq!(output_sheet.get_cell("C2").unwrap().get_value(), "中型");
        assert_eq!(output_sheet.get_cell("C3").unwrap().get_value(), "小型");
        for column in [1_u32, 3, 17] {
            assert_eq!(
                output_sheet
                    .get_style((column, 2_u32))
                    .get_background_color()
                    .unwrap()
                    .get_argb(),
                INCOMPLETE_ROW_FILL
            );
            assert_eq!(
                output_sheet
                    .get_style((column, 3_u32))
                    .get_background_color()
                    .unwrap()
                    .get_argb(),
                FAILED_ROW_FILL
            );
        }
        assert_eq!(
            output_sheet.get_cell("Q2").unwrap().get_value(),
            "单一字段判断不准确：数据不完整"
        );
        assert_eq!(
            output_sheet.get_cell("Q3").unwrap().get_value(),
            "处理失败：行业代码为空"
        );
    }

    fn read_first_sheet(path: &Path) -> Range<Data> {
        let mut workbook = open_workbook_auto(path).unwrap();
        let sheet_name = workbook.sheet_names().first().unwrap().clone();
        workbook.worksheet_range(&sheet_name).unwrap()
    }

    fn cell_text(cell: Option<&Data>) -> String {
        match cell {
            Some(Data::Empty) | None => String::new(),
            Some(value) => value.to_string(),
        }
    }

    fn sample_report(source_path: PathBuf) -> AnalysisReport {
        AnalysisReport {
            source_path,
            sheet_name: "Sheet1".to_owned(),
            annotation_column: "Q".to_owned(),
            annotate_skipped_rows: true,
            rows: vec![CheckRow {
                row_number: 2,
                customer_id: String::new(),
                account_name: String::new(),
                trusted_code: String::new(),
                industry_path: String::new(),
                matched_rule_code: None,
                matched_rule_name: None,
                original_value: "小型".to_owned(),
                calculated_size: Some(CompanySize::Micro),
                status: RowStatus::Changed,
                annotation: "C列：小型 -> 微型".to_owned(),
            }],
            summary: AnalysisSummary {
                total_rows: 1,
                changed_rows: 1,
                unchanged_rows: 0,
                incomplete_rows: 0,
                failed_rows: 0,
            },
        }
    }
}
