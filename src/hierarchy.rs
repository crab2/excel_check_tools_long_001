use crate::persistence::atomic_write;
use anyhow::{Context, Result, anyhow};
use calamine::{Data, Range, Reader, Xls, open_workbook_auto};
use directories::ProjectDirs;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};

const DEFAULT_HIERARCHY_BYTES: &[u8] = include_bytes!("../docs/行业分类代码注释.xls");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndustryCategory {
    pub id: u64,
    pub level1_code: String,
    pub level1_name: String,
    pub level2_code: String,
    pub level2_name: String,
    pub level3_code: String,
    pub level3_name: String,
    pub level4_code: String,
    pub level4_name: String,
}

impl IndustryCategory {
    pub fn validate(&self) -> Result<(), String> {
        let level1 = normalize_category_code(&self.level1_code);
        let level2 = normalize_category_code(&self.level2_code);
        let level3 = normalize_category_code(&self.level3_code);
        let level4 = normalize_category_code(&self.level4_code);
        let valid_level1 = Regex::new(r"^[A-T]$").expect("valid category regex");
        let valid_level2 = Regex::new(r"^[A-T]\d{2}$").expect("valid category regex");
        let valid_level3 = Regex::new(r"^[A-T]\d{3}$").expect("valid category regex");
        let valid_level4 = Regex::new(r"^[A-T]\d{4}$").expect("valid category regex");

        if !valid_level1.is_match(&level1)
            || !valid_level2.is_match(&level2)
            || !valid_level3.is_match(&level3)
            || !valid_level4.is_match(&level4)
        {
            return Err("分类代码格式应依次为 A、A01、A011、A0111".to_owned());
        }
        if !level2.starts_with(&level1)
            || !level3.starts_with(&level2)
            || !level4.starts_with(&level3)
        {
            return Err("四级分类代码必须保持逐级前缀关系".to_owned());
        }
        if [
            self.level1_name.as_str(),
            self.level2_name.as_str(),
            self.level3_name.as_str(),
            self.level4_name.as_str(),
        ]
        .iter()
        .any(|name| name.trim().is_empty())
        {
            return Err("一级至四级分类名称均不能为空".to_owned());
        }
        Ok(())
    }

    pub fn normalize(&mut self) {
        self.level1_code = normalize_category_code(&self.level1_code);
        self.level2_code = normalize_category_code(&self.level2_code);
        self.level3_code = normalize_category_code(&self.level3_code);
        self.level4_code = normalize_category_code(&self.level4_code);
        self.level1_name = self.level1_name.trim().to_owned();
        self.level2_name = self.level2_name.trim().to_owned();
        self.level3_name = self.level3_name.trim().to_owned();
        self.level4_name = self.level4_name.trim().to_owned();
    }

    pub fn codes(&self) -> Vec<String> {
        vec![
            self.level1_code.clone(),
            self.level2_code.clone(),
            self.level3_code.clone(),
            self.level4_code.clone(),
        ]
    }

    pub fn names(&self) -> Vec<String> {
        vec![
            self.level1_name.clone(),
            self.level2_name.clone(),
            self.level3_name.clone(),
            self.level4_name.clone(),
        ]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategorySet {
    pub version: u32,
    pub categories: Vec<IndustryCategory>,
}

impl CategorySet {
    pub const VERSION: u32 = 1;

    pub fn new(categories: Vec<IndustryCategory>) -> Self {
        Self {
            version: Self::VERSION,
            categories,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.categories.is_empty() {
            return Err("行业细类列表不能为空".to_owned());
        }
        let mut seen = HashSet::new();
        let mut ids = HashSet::new();
        let mut names_by_code: HashMap<String, String> = HashMap::new();
        for category in &self.categories {
            category.validate()?;
            if category.id == 0 || !ids.insert(category.id) {
                return Err(format!("行业细类 ID {} 无效或重复", category.id));
            }
            let code = normalize_category_code(&category.level4_code);
            if !seen.insert(code.clone()) {
                return Err(format!("四级分类代码 {code} 存在重复记录"));
            }
            for (code, name) in [
                (&category.level1_code, &category.level1_name),
                (&category.level2_code, &category.level2_name),
                (&category.level3_code, &category.level3_name),
                (&category.level4_code, &category.level4_name),
            ] {
                let code = normalize_category_code(code);
                let name = name.trim();
                if let Some(existing) = names_by_code.get(&code) {
                    if existing != name {
                        return Err(format!(
                            "分类代码 {code} 存在不一致名称：“{existing}”与“{name}”"
                        ));
                    }
                } else {
                    names_by_code.insert(code, name.to_owned());
                }
            }
        }
        Ok(())
    }

    pub fn next_id(&self) -> u64 {
        self.categories
            .iter()
            .map(|category| category.id)
            .max()
            .unwrap_or(0)
            + 1
    }
}

#[derive(Debug, Clone, Default)]
pub struct HierarchyIndex {
    categories_by_full_code: HashMap<String, IndustryCategory>,
    full_code_by_numeric_code: HashMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndustryPath {
    pub input_code: String,
    pub full_code: String,
    /// 从一级到四级的完整行业代码，例如 C、C38、C383、C3833。
    pub codes: Vec<String>,
    pub names: Vec<String>,
    pub found_in_reference: bool,
}

impl IndustryPath {
    pub fn display(&self) -> String {
        self.codes
            .iter()
            .enumerate()
            .map(|(index, code)| {
                self.names
                    .get(index)
                    .filter(|name| !name.is_empty())
                    .map(|name| format!("{code} {name}"))
                    .unwrap_or_else(|| code.clone())
            })
            .collect::<Vec<_>>()
            .join(" / ")
    }
}

impl HierarchyIndex {
    pub fn from_categories(category_set: &CategorySet) -> Result<Self> {
        category_set.validate().map_err(anyhow::Error::msg)?;
        let mut index = Self::default();
        for category in &category_set.categories {
            let full_code = normalize_category_code(&category.level4_code);
            index
                .full_code_by_numeric_code
                .insert(full_code[1..].to_owned(), full_code.clone());
            index
                .categories_by_full_code
                .insert(full_code, category.clone());
        }
        Ok(index)
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        let categories = load_categories_from_path(path)?;
        Self::from_categories(&categories)
    }

    pub fn leaf_count(&self) -> usize {
        self.categories_by_full_code.len()
    }

    pub fn resolve(&self, input: &str) -> IndustryPath {
        let normalized = normalize_category_code(input);
        let numeric = normalized
            .chars()
            .filter(char::is_ascii_digit)
            .collect::<String>();
        let numeric = if !numeric.is_empty() && numeric.len() < 4 {
            format!("{numeric:0>4}")
        } else {
            numeric
        };
        let full_code = self
            .full_code_by_numeric_code
            .get(&numeric)
            .cloned()
            .or_else(|| {
                self.categories_by_full_code
                    .contains_key(&normalized)
                    .then_some(normalized.clone())
            })
            .unwrap_or(normalized.clone());

        if let Some(category) = self.categories_by_full_code.get(&full_code) {
            return IndustryPath {
                input_code: input.trim().to_owned(),
                full_code,
                codes: category.codes(),
                names: category.names(),
                found_in_reference: true,
            };
        }

        let codes = inferred_codes(&full_code);
        let names = vec![String::new(); codes.len()];
        IndustryPath {
            input_code: input.trim().to_owned(),
            full_code,
            codes,
            names,
            found_in_reference: false,
        }
    }
}

pub fn load_default_categories() -> Result<CategorySet> {
    let mut workbook: Xls<_> = Xls::new(Cursor::new(DEFAULT_HIERARCHY_BYTES))
        .context("无法读取内置《行业分类代码注释.xls》")?;
    let range = match workbook.worksheet_range("数据表") {
        Ok(range) => range,
        Err(_) => workbook
            .worksheet_range_at(0)
            .ok_or_else(|| anyhow!("内置行业分类工作簿没有工作表"))??,
    };
    parse_category_range(&range)
}

pub fn load_categories_from_path(path: impl AsRef<Path>) -> Result<CategorySet> {
    let path = path.as_ref();
    let mut workbook = open_workbook_auto(path)
        .with_context(|| format!("无法打开行业分类代码表：{}", path.display()))?;
    let range = workbook
        .worksheet_range("数据表")
        .or_else(|_| {
            workbook
                .worksheet_range_at(0)
                .ok_or(calamine::Error::Msg("工作簿没有工作表"))?
        })
        .with_context(|| format!("无法读取行业分类代码表：{}", path.display()))?;
    parse_category_range(&range)
}

pub fn parse_category_range(range: &Range<Data>) -> Result<CategorySet> {
    let code_pattern =
        Regex::new(r"^[A-T](?:\d{2}|\d{3}|\d{4})?$").expect("valid hierarchy code regex");
    let mut names_by_code: HashMap<String, String> = HashMap::new();

    for row in range.rows() {
        let name = first_non_empty(&[row.get(8), row.get(1)]);
        for column_index in [5_usize, 6, 7] {
            let code = cell_code(row.get(column_index));
            if code_pattern.is_match(&code) {
                names_by_code
                    .entry(code)
                    .and_modify(|existing| {
                        if existing.is_empty() && !name.is_empty() {
                            *existing = name.clone();
                        }
                    })
                    .or_insert_with(|| name.clone());
            }
        }
    }

    let mut categories = Vec::new();
    for (row_index, row) in range.rows().enumerate() {
        let level4_code = cell_code(row.get(7));
        let raw_numeric = cell_text(row.get(4));
        let raw_digits: String = raw_numeric.chars().filter(char::is_ascii_digit).collect();
        let looks_like_leaf = (level4_code
            .chars()
            .next()
            .is_some_and(|character| ('A'..='T').contains(&character))
            && level4_code.len() >= 5)
            || raw_digits.len() >= 3;
        if !looks_like_leaf {
            continue;
        }
        if level4_code.len() != 5 || !code_pattern.is_match(&level4_code) {
            return Err(anyhow!(
                "行业分类表第 {} 行的四级完整代码无效：{}",
                row_index + 1,
                if level4_code.is_empty() {
                    "空".to_owned()
                } else {
                    level4_code
                }
            ));
        }
        // E 列存在数值单元格；这一步既补齐前导零，也验证其与 H 列完整代码一致。
        let numeric_code = normalize_four_digit_code(row.get(4))
            .ok_or_else(|| anyhow!("行业分类表第 {} 行的 E 列四级数字代码无效", row_index + 1))?;
        if numeric_code != level4_code[1..] {
            return Err(anyhow!(
                "行业分类表第 {} 行的 E/H 代码不一致：{} 与 {}",
                row_index + 1,
                numeric_code,
                level4_code
            ));
        }

        let level1_code = level4_code[..1].to_owned();
        let level2_code = level4_code[..3].to_owned();
        let explicit_level3 = cell_code(row.get(6));
        let level3_code = if explicit_level3.len() == 4 && code_pattern.is_match(&explicit_level3) {
            explicit_level3
        } else {
            level4_code[..4].to_owned()
        };
        let level4_name = first_non_empty(&[row.get(8), row.get(1)]);
        let level1_name = names_by_code.get(&level1_code).cloned().ok_or_else(|| {
            anyhow!(
                "行业分类表第 {} 行缺少一级分类 {}",
                row_index + 1,
                level1_code
            )
        })?;
        let level2_name = names_by_code.get(&level2_code).cloned().ok_or_else(|| {
            anyhow!(
                "行业分类表第 {} 行缺少二级分类 {}",
                row_index + 1,
                level2_code
            )
        })?;
        let level3_name = names_by_code.get(&level3_code).cloned().ok_or_else(|| {
            anyhow!(
                "行业分类表第 {} 行缺少三级分类 {}",
                row_index + 1,
                level3_code
            )
        })?;
        if level4_name.is_empty() {
            return Err(anyhow!(
                "行业分类表第 {} 行的四级分类名称为空",
                row_index + 1
            ));
        }

        categories.push(IndustryCategory {
            id: categories.len() as u64 + 1,
            level1_name,
            level2_name,
            level3_name,
            level4_name,
            level1_code,
            level2_code,
            level3_code,
            level4_code,
        });
    }

    let category_set = CategorySet::new(categories);
    category_set.validate().map_err(anyhow::Error::msg)?;
    Ok(category_set)
}

#[derive(Debug, Clone)]
pub struct CategoryRepository {
    path: PathBuf,
}

impl CategoryRepository {
    pub fn discover() -> Result<Self> {
        let project_dirs = ProjectDirs::from("cn", "ExcelCheck", "IndustryExcelChecker")
            .ok_or_else(|| anyhow!("无法确定本机应用数据目录"))?;
        Ok(Self {
            path: project_dirs.data_local_dir().join("categories.json"),
        })
    }

    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load_or_default(&self) -> Result<CategorySet> {
        if !self.path.exists() {
            let defaults = load_default_categories()?;
            self.save(&defaults)?;
            return Ok(defaults);
        }
        let bytes = fs::read(&self.path)
            .with_context(|| format!("无法读取行业细类配置：{}", self.path.display()))?;
        let categories: CategorySet = serde_json::from_slice(&bytes)
            .with_context(|| format!("行业细类配置格式无效：{}", self.path.display()))?;
        if categories.version != CategorySet::VERSION {
            return Err(anyhow!(
                "行业细类配置版本 {} 不受支持，当前版本为 {}",
                categories.version,
                CategorySet::VERSION
            ));
        }
        categories.validate().map_err(anyhow::Error::msg)?;
        Ok(categories)
    }

    pub fn save(&self, categories: &CategorySet) -> Result<()> {
        categories.validate().map_err(anyhow::Error::msg)?;
        let contents = serde_json::to_vec_pretty(categories).context("无法序列化行业细类配置")?;
        atomic_write(&self.path, &contents, "行业细类配置")
    }
}

pub fn normalize_category_code(value: &str) -> String {
    value
        .trim()
        .to_ascii_uppercase()
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect()
}

fn inferred_codes(full_code: &str) -> Vec<String> {
    if full_code.len() == 5
        && full_code
            .chars()
            .next()
            .is_some_and(|character| ('A'..='T').contains(&character))
        && full_code
            .chars()
            .skip(1)
            .all(|character| character.is_ascii_digit())
    {
        vec![
            full_code[..1].to_owned(),
            full_code[..3].to_owned(),
            full_code[..4].to_owned(),
            full_code.to_owned(),
        ]
    } else if full_code.is_empty() {
        Vec::new()
    } else {
        vec![full_code.to_owned()]
    }
}

fn first_non_empty(values: &[Option<&Data>]) -> String {
    values
        .iter()
        .map(|value| cell_text(*value))
        .find(|value| !value.trim().is_empty())
        .unwrap_or_default()
}

fn cell_text(value: Option<&Data>) -> String {
    value
        .filter(|cell| !matches!(cell, Data::Empty))
        .map(ToString::to_string)
        .unwrap_or_default()
        .trim()
        .to_owned()
}

fn cell_code(value: Option<&Data>) -> String {
    normalize_category_code(&cell_text(value))
}

fn normalize_four_digit_code(value: Option<&Data>) -> Option<String> {
    let raw = match value? {
        Data::Int(number) => number.to_string(),
        Data::Float(number) if number.fract() == 0.0 => format!("{number:.0}"),
        cell => cell.to_string(),
    };
    let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
    (!digits.is_empty() && digits.len() <= 4).then(|| format!("{digits:0>4}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_range() -> Range<Data> {
        let mut range = Range::new((0, 0), (2, 8));
        range.set_value((0, 1), Data::String("农、林、牧、渔业".to_owned()));
        range.set_value((0, 7), Data::String("A".to_owned()));
        range.set_value((0, 8), Data::String("农、林、牧、渔业".to_owned()));
        range.set_value((1, 1), Data::String("农业".to_owned()));
        range.set_value((1, 7), Data::String("A01".to_owned()));
        range.set_value((1, 8), Data::String("农业".to_owned()));
        range.set_value((2, 1), Data::String("其他农业".to_owned()));
        range.set_value((2, 4), Data::Float(190.0));
        range.set_value((2, 6), Data::String("A019".to_owned()));
        range.set_value((2, 7), Data::String("A0190".to_owned()));
        range.set_value((2, 8), Data::String("其他农业".to_owned()));
        range
    }

    #[test]
    fn numeric_leaf_codes_are_zero_padded_and_use_explicit_third_level() {
        let categories = parse_category_range(&sample_range()).unwrap();
        let index = HierarchyIndex::from_categories(&categories).unwrap();
        let path = index.resolve("0190");
        assert!(path.found_in_reference);
        assert_eq!(path.full_code, "A0190");
        assert_eq!(path.codes, vec!["A", "A01", "A019", "A0190"]);
    }

    #[test]
    fn built_in_hierarchy_contains_all_fourth_level_categories() {
        let categories = load_default_categories().unwrap();
        assert_eq!(categories.categories.len(), 1_381);
        let index = HierarchyIndex::from_categories(&categories).unwrap();
        assert_eq!(index.leaf_count(), 1_381);
    }

    #[test]
    fn repository_round_trips_categories() {
        let directory = tempfile::tempdir().unwrap();
        let repository = CategoryRepository::at(directory.path().join("categories.json"));
        let expected = repository.load_or_default().unwrap();
        assert!(repository.path().exists());
        let actual = repository.load_or_default().unwrap();
        assert_eq!(actual.categories, expected.categories);
    }
}
