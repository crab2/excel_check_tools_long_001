use crate::checker::{DEFAULT_ANNOTATION_COLUMN, column_label_to_index};
use crate::persistence::atomic_write;
use anyhow::{Context, Result, anyhow};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub version: u32,
    pub annotation_column: String,
    pub annotate_skipped_rows: bool,
    pub last_source_path: Option<PathBuf>,
    pub last_output_path: Option<PathBuf>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: Self::VERSION,
            annotation_column: DEFAULT_ANNOTATION_COLUMN.to_owned(),
            annotate_skipped_rows: true,
            last_source_path: None,
            last_output_path: None,
        }
    }
}

impl AppSettings {
    pub const VERSION: u32 = 1;

    pub fn validate(&self) -> Result<()> {
        if self.version != Self::VERSION {
            return Err(anyhow!(
                "应用设置版本 {} 不受支持，当前版本为 {}",
                self.version,
                Self::VERSION
            ));
        }
        let annotation_index = column_label_to_index(&self.annotation_column)?;
        if annotation_index <= 9 {
            return Err(anyhow!("标注列不能占用源数据 A-I 列"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SettingsRepository {
    path: PathBuf,
}

impl SettingsRepository {
    pub fn discover() -> Result<Self> {
        let project_dirs = ProjectDirs::from("cn", "ExcelCheck", "IndustryExcelChecker")
            .ok_or_else(|| anyhow!("无法确定本机应用数据目录"))?;
        Ok(Self {
            path: project_dirs.config_local_dir().join("settings.json"),
        })
    }

    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<AppSettings> {
        if !self.path.exists() {
            return Ok(AppSettings::default());
        }
        let bytes = fs::read(&self.path)
            .with_context(|| format!("无法读取应用设置：{}", self.path.display()))?;
        let settings: AppSettings = serde_json::from_slice(&bytes)
            .with_context(|| format!("应用设置格式无效：{}", self.path.display()))?;
        settings.validate()?;
        Ok(settings)
    }

    pub fn save(&self, settings: &AppSettings) -> Result<()> {
        settings.validate()?;
        let contents = serde_json::to_vec_pretty(settings).context("无法序列化应用设置")?;
        atomic_write(&self.path, &contents, "应用设置")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let repository = SettingsRepository::at(directory.path().join("settings.json"));
        let settings = AppSettings {
            annotation_column: "AA".to_owned(),
            annotate_skipped_rows: false,
            last_source_path: Some(PathBuf::from("输入.xlsx")),
            ..AppSettings::default()
        };
        repository.save(&settings).unwrap();
        let loaded = repository.load().unwrap();
        assert_eq!(loaded.annotation_column, "AA");
        assert!(!loaded.annotate_skipped_rows);
        assert_eq!(loaded.last_source_path, settings.last_source_path);
    }
}
