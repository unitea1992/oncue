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

/// 残す起動ログの数。古いものから消す。
const MAX_LOG_FILES: usize = 20;

#[derive(Debug, Clone)]
pub struct PortablePaths {
    base_dir: PathBuf,
    /// この起動で書くログのファイル名。起動ごとに1つ作り、実行中は変えない。
    log_file_name: String,
}

/// 起動時刻を名前に含めたログファイル名。名前順が古い順になる。
fn launch_log_file_name() -> String {
    format!(
        "oncue_{}.jsonl",
        chrono::Local::now().format("%Y%m%d_%H%M%S")
    )
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

        Ok(Self {
            base_dir,
            log_file_name: launch_log_file_name(),
        })
    }

    /// カスタムベースディレクトリでインスタンスを作成（テスト用）
    pub fn with_base_dir(base_dir: &std::path::Path) -> Self {
        Self {
            base_dir: base_dir.to_path_buf(),
            log_file_name: launch_log_file_name(),
        }
    }

    #[cfg(test)]
    pub fn with_base(base_dir: PathBuf) -> Self {
        Self {
            base_dir,
            log_file_name: launch_log_file_name(),
        }
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

    /// この起動のログファイル。起動ごとに別のファイルにし、共有するときに前の起動の記録が混ざらないようにする。
    pub fn log_file(&self) -> PathBuf {
        self.logs_dir().join(&self.log_file_name)
    }

    /// 古いログを消し、新しい順に `MAX_LOG_FILES` 件だけ残す。消せなくても起動は続ける。
    pub fn prune_old_logs(&self) {
        let Ok(entries) = std::fs::read_dir(self.logs_dir()) else {
            return;
        };
        let mut logs: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("oncue_") && name.ends_with(".jsonl"))
            })
            .collect();
        // 日付だけの旧形式（oncue_YYYYMMDD.jsonl）も、名前順で同じ日の起動ログより前に並ぶ。
        logs.sort();
        let excess = logs.len().saturating_sub(MAX_LOG_FILES);
        for path in logs.into_iter().take(excess) {
            let _ = std::fs::remove_file(path);
        }
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
    fn log_file_is_fixed_per_launch_and_old_logs_are_pruned() {
        let (paths, _temp) = create_test_paths();
        assert_eq!(paths.log_file(), paths.clone().log_file());
        let logs_dir = paths.logs_dir();
        std::fs::write(logs_dir.join("oncue_20260101.jsonl"), "").unwrap();
        for i in 0..MAX_LOG_FILES {
            std::fs::write(
                logs_dir.join(format!("oncue_20260102_0000{:02}.jsonl", i)),
                "",
            )
            .unwrap();
        }
        std::fs::write(logs_dir.join("other.txt"), "").unwrap();
        paths.prune_old_logs();
        assert!(!logs_dir.join("oncue_20260101.jsonl").exists());
        assert!(logs_dir.join("oncue_20260102_000000.jsonl").exists());
        assert!(logs_dir.join("other.txt").exists());
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
