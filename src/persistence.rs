use anyhow::{Context, Result};
use std::io::Write;
use std::path::Path;

pub(crate) fn atomic_write(path: &Path, contents: &[u8], label: &str) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .with_context(|| format!("无法创建{label}目录：{}", parent.display()))?;

    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("无法创建{label}临时文件：{}", parent.display()))?;
    temporary
        .write_all(contents)
        .with_context(|| format!("无法写入{label}临时文件"))?;
    temporary
        .as_file_mut()
        .sync_all()
        .with_context(|| format!("无法同步{label}临时文件"))?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("无法原子替换{label}：{}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_existing_file_atomically() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        std::fs::write(&path, b"old").unwrap();
        atomic_write(&path, b"new", "测试配置").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }
}
