use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("フォルダに書き込めません: {path}（{reason}）")]
    NotWritable { path: PathBuf, reason: String },
    #[error("フォルダを作成できませんでした: {0}")]
    CreateDirFailed(String),
    #[error("ファイルの読み書きに失敗しました: {0}")]
    Io(#[from] std::io::Error),
    #[error("データの変換に失敗しました: {0}")]
    Serialization(String),
}

#[derive(Debug, Clone)]
pub struct PortablePaths {
    base_dir: PathBuf,
}

impl PortablePaths {
    pub fn new() -> Result<Self, StorageError> {
        let exe_path = std::env::current_exe().map_err(StorageError::Io)?;

        let base_dir = exe_path
            .parent()
            .ok_or_else(|| {
                StorageError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Could not determine executable directory",
                ))
            })?
            .to_path_buf();

        Ok(Self { base_dir })
    }

    /// カスタムベースディレクトリでインスタンスを作成（テスト用）
    pub fn with_base_dir(base_dir: &std::path::Path) -> Self {
        Self {
            base_dir: base_dir.to_path_buf(),
        }
    }

    #[cfg(test)]
    pub fn with_base(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    pub fn verify_writable(&self) -> Result<(), StorageError> {
        // dataディレクトリの書き込み可能性をテスト（実際にデータを保存する場所）
        let data_dir = self.data_dir(); // これでディレクトリが作成される
        let test_file = data_dir.join(".write_test");

        match std::fs::write(&test_file, b"test") {
            Ok(_) => {
                let _ = std::fs::remove_file(&test_file);
                Ok(())
            }
            Err(e) => Err(StorageError::NotWritable {
                path: data_dir.clone(),
                reason: e.to_string(),
            }),
        }
    }

    pub fn data_dir(&self) -> PathBuf {
        let path = self.base_dir.join("data");
        let _ = std::fs::create_dir_all(&path);
        path
    }

    pub fn cache_dir(&self) -> PathBuf {
        let path = self.base_dir.join("cache");
        let _ = std::fs::create_dir_all(&path);
        path
    }

    pub fn logs_dir(&self) -> PathBuf {
        let path = self.base_dir.join("logs");
        let _ = std::fs::create_dir_all(&path);
        path
    }

    pub fn session_file(&self) -> PathBuf {
        self.data_dir().join("session.enc")
    }

    pub fn presets_file(&self) -> PathBuf {
        self.data_dir().join("presets.json")
    }

    pub fn config_file(&self) -> PathBuf {
        self.data_dir().join("config.json")
    }

    pub fn log_file(&self) -> PathBuf {
        let now = chrono::Local::now();
        let filename = format!("oncue_{}.jsonl", now.format("%Y%m%d"));
        self.logs_dir().join(filename)
    }

    pub fn cache_file(&self, name: &str) -> PathBuf {
        self.cache_dir().join(format!("{}.json", name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_paths() -> (PortablePaths, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let paths = PortablePaths::with_base(temp_dir.path().to_path_buf());
        (paths, temp_dir)
    }

    #[test]
    fn test_portable_paths_directory_creation_and_writable() {
        let (paths, _temp) = create_test_paths();

        // 各ディレクトリパス取得時に作成されることを確認
        let data_dir = paths.data_dir();
        let cache_dir = paths.cache_dir();
        let logs_dir = paths.logs_dir();

        assert!(data_dir.exists());
        assert!(cache_dir.exists());
        assert!(logs_dir.exists());
        assert!(paths.verify_writable().is_ok());
    }

    // test_file_paths は実装の join() 再記述のため削除。
    // test_log_file_format は内部ファイル名書式の自己確認のため削除。
    // test_verify_writable_success は上記へ統合。
}
