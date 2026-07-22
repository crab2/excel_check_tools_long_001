use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompanySize {
    Large,
    Medium,
    Small,
    Micro,
}

impl CompanySize {
    pub const ALL: [Self; 4] = [Self::Large, Self::Medium, Self::Small, Self::Micro];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Large => "大型",
            Self::Medium => "中型",
            Self::Small => "小型",
            Self::Micro => "微型",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let normalized = value.trim().replace("企业", "");
        match normalized.as_str() {
            "大型" | "大" => Some(Self::Large),
            "中型" | "中" => Some(Self::Medium),
            "小型" | "小" => Some(Self::Small),
            "微型" | "微" => Some(Self::Micro),
            _ => None,
        }
    }
}

impl fmt::Display for CompanySize {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MetricKind {
    Employees,
    Assets,
    Revenue,
}

impl MetricKind {
    pub const ALL: [Self; 3] = [Self::Employees, Self::Assets, Self::Revenue];

    pub fn label(self) -> &'static str {
        match self {
            Self::Employees => "从业人员数",
            Self::Assets => "资产总额",
            Self::Revenue => "营业收入",
        }
    }

    pub fn source_column(self) -> &'static str {
        match self {
            Self::Employees => "G",
            Self::Assets => "H",
            Self::Revenue => "I",
        }
    }

    pub fn unit_label(self) -> &'static str {
        match self {
            Self::Employees => "人",
            Self::Assets | Self::Revenue => "万元",
        }
    }

    pub fn divisor(self) -> f64 {
        match self {
            Self::Employees => 1.0,
            Self::Assets | Self::Revenue => 10_000.0,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        let normalized = value.trim().replace([' ', '\n', '\r'], "");
        if normalized.contains("从业人员") || normalized.contains("员工") {
            Some(Self::Employees)
        } else if normalized.contains("资产总额") || normalized.contains("资产") {
            Some(Self::Assets)
        } else if normalized.contains("营业收入") || normalized.contains("营收") {
            Some(Self::Revenue)
        } else {
            None
        }
    }
}

impl fmt::Display for MetricKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MetricRule {
    pub metric: MetricKind,
    /// 大型企业的下限。低于此值才属于中小微范围。
    pub large_min: f64,
    pub medium_min: f64,
    pub small_min: f64,
}

impl MetricRule {
    pub fn validate(&self) -> Result<(), String> {
        if !self.large_min.is_finite()
            || !self.medium_min.is_finite()
            || !self.small_min.is_finite()
        {
            return Err(format!("{}的阈值必须是有效数字", self.metric));
        }
        if self.small_min < 0.0 {
            return Err(format!("{}的小型下限不能小于 0", self.metric));
        }
        if !(self.large_min > self.medium_min && self.medium_min > self.small_min) {
            return Err(format!(
                "{}阈值必须满足：大型下限 > 中型下限 > 小型下限",
                self.metric
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndustryRule {
    pub id: u64,
    pub industry_name: String,
    /// 标准表 B 列行业代码。`*` 表示未匹配到明确代码时使用的兜底规则。
    pub industry_code: String,
    pub metrics: Vec<MetricRule>,
}

impl IndustryRule {
    pub fn validate(&self) -> Result<(), String> {
        if self.industry_name.trim().is_empty() {
            return Err("行业名称不能为空".to_owned());
        }
        let code = normalize_rule_code(&self.industry_code);
        if code.is_empty() {
            return Err("行业代码不能为空；其他行业请使用 *".to_owned());
        }
        if code != "*" {
            let valid = code
                .chars()
                .next()
                .is_some_and(|character| ('A'..='T').contains(&character))
                && code
                    .chars()
                    .skip(1)
                    .all(|character| character.is_ascii_digit());
            if !valid {
                return Err("行业代码应为 A、F51、K701 等格式，兜底规则使用 *".to_owned());
            }
        }
        if self.metrics.is_empty() {
            return Err("每条标准至少需要一个指标".to_owned());
        }
        if self.metrics.len() > 3 {
            return Err("每条标准最多配置三个指标".to_owned());
        }
        for (index, metric) in self.metrics.iter().enumerate() {
            if self
                .metrics
                .iter()
                .skip(index + 1)
                .any(|other| other.metric == metric.metric)
            {
                return Err(format!("指标 {} 不能重复", metric.metric));
            }
            metric.validate()?;
        }
        Ok(())
    }

    pub fn classify(&self, values: &MetricValues) -> Result<CompanySize, Vec<MetricKind>> {
        let mut resolved = Vec::with_capacity(self.metrics.len());
        let mut missing = Vec::new();
        for metric_rule in &self.metrics {
            match values.get(metric_rule.metric) {
                Some(value) if value.is_finite() && value >= 0.0 => {
                    resolved.push((metric_rule, value));
                }
                _ => missing.push(metric_rule.metric),
            }
        }
        // 微型标准使用“或”：任一已知指标低于小型下限即可确定为微型。
        if resolved
            .iter()
            .any(|(metric_rule, value)| *value < metric_rule.small_min)
        {
            return Ok(CompanySize::Micro);
        }
        if !missing.is_empty() {
            return Err(missing);
        }

        if resolved
            .iter()
            .all(|(metric_rule, value)| *value >= metric_rule.large_min)
        {
            return Ok(CompanySize::Large);
        }
        if resolved
            .iter()
            .all(|(metric_rule, value)| *value >= metric_rule.medium_min)
        {
            return Ok(CompanySize::Medium);
        }
        if resolved
            .iter()
            .all(|(metric_rule, value)| *value >= metric_rule.small_min)
        {
            return Ok(CompanySize::Small);
        }
        Ok(CompanySize::Micro)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MetricValues {
    pub employees: Option<f64>,
    pub assets: Option<f64>,
    pub revenue: Option<f64>,
}

impl MetricValues {
    pub fn get(&self, metric: MetricKind) -> Option<f64> {
        match metric {
            MetricKind::Employees => self.employees,
            MetricKind::Assets => self.assets,
            MetricKind::Revenue => self.revenue,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleSet {
    pub version: u32,
    pub rules: Vec<IndustryRule>,
}

impl RuleSet {
    pub const VERSION: u32 = 1;

    pub fn new(rules: Vec<IndustryRule>) -> Self {
        Self {
            version: Self::VERSION,
            rules,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.rules.is_empty() {
            return Err("规则列表不能为空".to_owned());
        }
        let mut ids = std::collections::HashSet::new();
        for rule in &self.rules {
            rule.validate()?;
            if rule.id == 0 || !ids.insert(rule.id) {
                return Err(format!("规则 ID {} 无效或重复", rule.id));
            }
        }
        for (index, rule) in self.rules.iter().enumerate() {
            let code = normalize_rule_code(&rule.industry_code);
            if self
                .rules
                .iter()
                .skip(index + 1)
                .any(|other| normalize_rule_code(&other.industry_code) == code)
            {
                return Err(format!("行业代码 {code} 存在重复规则"));
            }
        }
        Ok(())
    }

    pub fn find_for_codes<'a>(&'a self, codes: &[String]) -> Option<&'a IndustryRule> {
        let mut exact_matches: Vec<(&IndustryRule, usize)> = self
            .rules
            .iter()
            .filter_map(|rule| {
                let rule_code = normalize_rule_code(&rule.industry_code);
                (rule_code != "*"
                    && codes
                        .iter()
                        .any(|candidate| normalize_rule_code(candidate) == rule_code))
                .then_some((rule, rule_code.len()))
            })
            .collect();
        exact_matches.sort_by_key(|(_, length)| std::cmp::Reverse(*length));
        exact_matches.first().map(|(rule, _)| *rule).or_else(|| {
            self.rules
                .iter()
                .find(|rule| normalize_rule_code(&rule.industry_code) == "*")
        })
    }

    pub fn next_id(&self) -> u64 {
        self.rules.iter().map(|rule| rule.id).max().unwrap_or(0) + 1
    }
}

pub fn normalize_rule_code(value: &str) -> String {
    value
        .trim()
        .to_ascii_uppercase()
        .chars()
        .filter(|character| !character.is_whitespace() && *character != ':' && *character != '：')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manufacturing_rule() -> IndustryRule {
        IndustryRule {
            id: 1,
            industry_name: "制造业".to_owned(),
            industry_code: "C".to_owned(),
            metrics: vec![
                MetricRule {
                    metric: MetricKind::Employees,
                    large_min: 1_000.0,
                    medium_min: 300.0,
                    small_min: 20.0,
                },
                MetricRule {
                    metric: MetricKind::Revenue,
                    large_min: 40_000.0,
                    medium_min: 2_000.0,
                    small_min: 300.0,
                },
            ],
        }
    }

    #[test]
    fn mixed_metrics_fall_to_the_lowest_eligible_size() {
        let rule = manufacturing_rule();
        let values = MetricValues {
            employees: Some(1_500.0),
            revenue: Some(1_500.0),
            ..MetricValues::default()
        };
        assert_eq!(rule.classify(&values), Ok(CompanySize::Small));
    }

    #[test]
    fn threshold_values_are_included_in_the_higher_size() {
        let rule = manufacturing_rule();
        let values = MetricValues {
            employees: Some(300.0),
            revenue: Some(2_000.0),
            ..MetricValues::default()
        };
        assert_eq!(rule.classify(&values), Ok(CompanySize::Medium));
    }

    #[test]
    fn missing_metric_is_reported() {
        let rule = manufacturing_rule();
        let values = MetricValues {
            employees: Some(300.0),
            ..MetricValues::default()
        };
        assert_eq!(rule.classify(&values), Err(vec![MetricKind::Revenue]));
    }

    #[test]
    fn known_value_below_small_threshold_proves_micro_despite_missing_metric() {
        let rule = manufacturing_rule();
        let values = MetricValues {
            employees: Some(10.0),
            revenue: None,
            ..MetricValues::default()
        };
        assert_eq!(rule.classify(&values), Ok(CompanySize::Micro));
    }

    #[test]
    fn longest_hierarchy_code_wins() {
        let rules = RuleSet::new(vec![
            manufacturing_rule(),
            IndustryRule {
                id: 2,
                industry_name: "特定中类".to_owned(),
                industry_code: "C38".to_owned(),
                metrics: vec![MetricRule {
                    metric: MetricKind::Employees,
                    large_min: 100.0,
                    medium_min: 50.0,
                    small_min: 10.0,
                }],
            },
        ]);
        let codes = vec!["C".to_owned(), "C38".to_owned(), "C383".to_owned()];
        assert_eq!(rules.find_for_codes(&codes).unwrap().industry_code, "C38");
    }
}
