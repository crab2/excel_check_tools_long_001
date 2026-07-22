use crate::model::{IndustryRule, MetricKind, MetricRule, RuleSet, normalize_rule_code};
use crate::persistence::atomic_write;
use anyhow::{Context, Result, anyhow};
use calamine::{Data, Range, Reader, Xlsx, open_workbook_auto};
use directories::ProjectDirs;
use regex::Regex;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

const DEFAULT_STANDARD_BYTES: &[u8] = include_bytes!("../docs/标准 更新.xlsx");

pub fn load_default_rules() -> Result<RuleSet> {
    let mut workbook: Xlsx<_> =
        Xlsx::new(Cursor::new(DEFAULT_STANDARD_BYTES)).context("无法读取内置《标准 更新.xlsx》")?;
    let range = workbook
        .worksheet_range_at(0)
        .ok_or_else(|| anyhow!("内置标准工作簿没有工作表"))??;
    parse_standard_range(&range)
}

pub fn load_rules_from_path(path: impl AsRef<Path>) -> Result<RuleSet> {
    let path = path.as_ref();
    let mut workbook = open_workbook_auto(path)
        .with_context(|| format!("无法打开标准文件：{}", path.display()))?;
    let range = workbook
        .worksheet_range_at(0)
        .ok_or_else(|| anyhow!("标准文件没有工作表"))?
        .with_context(|| format!("无法读取标准文件第一张工作表：{}", path.display()))?;
    parse_standard_range(&range)
}

pub fn parse_standard_range(range: &Range<Data>) -> Result<RuleSet> {
    let number_pattern = Regex::new(r"(\d+(?:\.\d+)?)").expect("valid threshold regex");
    let mut rules: Vec<IndustryRule> = Vec::new();
    let mut current_rule_index: Option<usize> = None;
    let mut next_id = 1_u64;

    for (row_index, row) in range.rows().enumerate() {
        // 标准表前 3 行是标题；同时通过指标名称过滤，兼容标题行位置的小幅变化。
        if row_index < 3 {
            continue;
        }
        let industry_name = cell_text(row.first());
        let industry_code = cell_text(row.get(1));
        let metric_name = cell_text(row.get(2));
        let has_threshold_data = (3..=6).any(|column| !cell_text(row.get(column)).is_empty());
        let metric = match MetricKind::parse(&metric_name) {
            Some(metric) => metric,
            None if has_threshold_data || !industry_code.trim().is_empty() => {
                return Err(anyhow!(
                    "标准表第 {} 行的指标名称无法识别：{}",
                    row_index + 1,
                    if metric_name.is_empty() {
                        "空".to_owned()
                    } else {
                        metric_name
                    }
                ));
            }
            None => continue,
        };

        if !industry_name.trim().is_empty() || !industry_code.trim().is_empty() {
            let mut code = normalize_rule_code(&industry_code);
            if code.is_empty()
                && (industry_name.contains("其他行业") || industry_name.contains("其它行业"))
            {
                code = "*".to_owned();
            }
            rules.push(IndustryRule {
                id: next_id,
                industry_name: industry_name.trim().to_owned(),
                industry_code: code,
                metrics: Vec::new(),
            });
            current_rule_index = Some(rules.len() - 1);
            next_id += 1;
        }

        let rule_index = current_rule_index.ok_or_else(|| {
            anyhow!(
                "标准表第 {} 行存在指标，但没有可归属的行业名称/代码",
                row_index + 1
            )
        })?;
        let large_min = parse_threshold(row.get(3), &number_pattern, row_index, "中小微型上限")?;
        let medium_min = parse_threshold(row.get(4), &number_pattern, row_index, "中型下限")?;
        let small_min = parse_threshold(row.get(5), &number_pattern, row_index, "小型下限")?;

        rules[rule_index].metrics.push(MetricRule {
            metric,
            large_min,
            medium_min,
            small_min,
        });
    }

    let rule_set = RuleSet::new(rules);
    rule_set.validate().map_err(anyhow::Error::msg)?;
    Ok(rule_set)
}

fn parse_threshold(
    value: Option<&Data>,
    number_pattern: &Regex,
    row_index: usize,
    label: &str,
) -> Result<f64> {
    let text = cell_text(value).replace(',', "");
    let capture = number_pattern
        .captures(&text)
        .ok_or_else(|| anyhow!("标准表第 {} 行的{}无法识别：{}", row_index + 1, label, text))?;
    capture[1].parse::<f64>().with_context(|| {
        format!(
            "标准表第 {} 行的{}不是有效数字：{}",
            row_index + 1,
            label,
            &capture[1]
        )
    })
}

fn cell_text(value: Option<&Data>) -> String {
    value
        .filter(|cell| !matches!(cell, Data::Empty))
        .map(ToString::to_string)
        .unwrap_or_default()
}

#[derive(Debug, Clone)]
pub struct RuleRepository {
    path: PathBuf,
}

impl RuleRepository {
    pub fn discover() -> Result<Self> {
        let project_dirs = ProjectDirs::from("cn", "ExcelCheck", "IndustryExcelChecker")
            .ok_or_else(|| anyhow!("无法确定本机应用数据目录"))?;
        Ok(Self {
            path: project_dirs.data_local_dir().join("rules.json"),
        })
    }

    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load_or_default(&self) -> Result<RuleSet> {
        if !self.path.exists() {
            let defaults = load_default_rules()?;
            self.save(&defaults)?;
            return Ok(defaults);
        }
        let bytes = fs::read(&self.path)
            .with_context(|| format!("无法读取规则配置：{}", self.path.display()))?;
        let rules: RuleSet = serde_json::from_slice(&bytes)
            .with_context(|| format!("规则配置格式无效：{}", self.path.display()))?;
        if rules.version != RuleSet::VERSION {
            return Err(anyhow!(
                "规则配置版本 {} 不受支持，当前版本为 {}",
                rules.version,
                RuleSet::VERSION
            ));
        }
        rules.validate().map_err(anyhow::Error::msg)?;
        Ok(rules)
    }

    pub fn save(&self, rules: &RuleSet) -> Result<()> {
        rules.validate().map_err(anyhow::Error::msg)?;
        let contents = serde_json::to_vec_pretty(rules).context("无法序列化规则配置")?;
        atomic_write(&self.path, &contents, "规则配置")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_standard_has_expected_rules() {
        let rules = load_default_rules().expect("default standard should parse");
        assert_eq!(rules.rules.len(), 24);
        let manufacturing = rules
            .rules
            .iter()
            .find(|rule| rule.industry_code == "C")
            .expect("manufacturing rule");
        assert_eq!(manufacturing.metrics.len(), 2);
        assert_eq!(manufacturing.metrics[0].large_min, 1_000.0);
        assert_eq!(manufacturing.metrics[1].large_min, 40_000.0);
        assert!(rules.rules.iter().any(|rule| rule.industry_code == "*"));
    }

    #[test]
    fn repository_round_trips_rules() {
        let directory = tempfile::tempdir().unwrap();
        let repository = RuleRepository::at(directory.path().join("rules.json"));
        let expected = repository.load_or_default().unwrap();
        assert!(repository.path().exists());
        let actual = repository.load_or_default().unwrap();
        assert_eq!(actual.rules, expected.rules);
    }
}
