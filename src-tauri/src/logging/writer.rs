use crate::logging::events::{LogEvent, LogLevel};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

pub struct LogWriter {
    file: Option<BufWriter<File>>,
    buffer: Vec<LogEvent>,
    max_buffer_size: usize,
    min_level: LogLevel,
}

impl LogWriter {
    /// ログファイルを開く。書込不可配置でもアプリ起動を止めないため、
    /// 失敗時はメモリ保持のみへ縮退する（file_availableで通知）。
    pub fn new(log_file: PathBuf) -> Self {
        let file = std::fs::create_dir_all(log_file.parent().unwrap_or(&PathBuf::from(".")))
            .ok()
            .and_then(|_| {
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&log_file)
                    .ok()
            })
            .map(BufWriter::new);

        Self {
            file,
            buffer: Vec::new(),
            max_buffer_size: 100,
            min_level: LogLevel::Info,
        }
    }

    /// ログファイルへ書き込めるか。falseなら起動配置の見直しを案内する。
    pub fn file_available(&self) -> bool {
        self.file.is_some()
    }

    /// 出力レベル制御。debug_mode=falseではDebugを落とす。
    pub fn set_level(&mut self, level: LogLevel) {
        self.min_level = level;
    }

    pub fn log(&mut self, event: LogEvent) -> std::io::Result<()> {
        if event.level < self.min_level {
            return Ok(());
        }

        if let Some(file) = self.file.as_mut() {
            let line = serde_json::to_string(&event)?;
            writeln!(file, "{}", line)?;
            file.flush()?;
        }

        self.buffer.push(event);
        if self.buffer.len() > self.max_buffer_size {
            self.buffer.remove(0);
        }

        Ok(())
    }

    pub fn get_recent_logs(&self, count: usize) -> Vec<LogEvent> {
        let start = self.buffer.len().saturating_sub(count);
        self.buffer[start..].to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logging::events::LogEvent;

    #[test]
    fn test_level_filter_drops_debug_by_default() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let mut writer = LogWriter::new(dir.path().join("test.log"));
        assert!(writer.file_available());
        writer.log(LogEvent::debug("test", "hidden")).unwrap();
        assert!(writer.get_recent_logs(10).is_empty());
        writer.set_level(LogLevel::Debug);
        writer.log(LogEvent::debug("test", "shown")).unwrap();
        assert_eq!(writer.get_recent_logs(10).len(), 1);
    }

    #[test]
    fn test_unwritable_path_degrades_to_memory_only() {
        // ファイルを親に持つパスではディレクトリ作成に失敗し縮退する。
        let dir = tempfile::TempDir::new().expect("tempdir");
        let blocker = dir.path().join("a-file");
        std::fs::write(&blocker, b"x").unwrap();
        let mut writer = LogWriter::new(blocker.join("test.log"));
        assert!(!writer.file_available());
        writer.set_level(LogLevel::Debug);
        writer.log(LogEvent::info("test", "kept")).unwrap();
        assert_eq!(writer.get_recent_logs(10).len(), 1);
    }
}
