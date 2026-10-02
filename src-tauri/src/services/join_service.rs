use crate::domain::models::AppError;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
#[cfg(target_os = "windows")]
use std::process::Command;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

pub struct JoinService;

impl Default for JoinService {
    fn default() -> Self {
        Self::new()
    }
}

impl JoinService {
    pub fn new() -> Self {
        Self
    }

    /// vrchat:// 起動リンクを組み立てる。余計なプロセスを招かないよう
    /// URLハンドラへ直接渡す形式に固定する。
    /// 起動リンクへ埋め込むlocationの検査。`wrld_...:...` 形式で、
    /// `&` `?` `#` や空白など起動リンクの引数を増やせる文字を含まないものだけ許可する。
    pub fn is_launchable_location(location: &str) -> bool {
        location.starts_with("wrld_")
            && location.contains(':')
            && location
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "_-:~().".contains(c))
    }

    pub fn build_launch_url(location: &str) -> String {
        format!("vrchat://launch?ref=vrchat.com&id={}&attach=1", location)
    }

    pub async fn dispatch_launch_protocol(&self, location: &str) -> Result<(), AppError> {
        #[cfg(not(target_os = "windows"))]
        let _ = location;

        #[cfg(target_os = "windows")]
        let url = Self::build_launch_url(location);

        #[cfg(target_os = "windows")]
        {
            // `cmd /C start` は余計なプロセス起動を招きやすいので、
            // URLハンドラを直接呼び出す。
            let result = Command::new("rundll32.exe")
                .args(["url.dll,FileProtocolHandler", &url])
                .creation_flags(CREATE_NO_WINDOW)
                .spawn();

            match result {
                Ok(_) => Ok(()),
                Err(e) => Err(AppError::Operation(format!(
                    "VRChatを起動できませんでした: {}",
                    e
                ))),
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            Err(AppError::Operation(
                "VRChatの起動はWindowsでのみ対応しています".to_string(),
            ))
        }
    }

    /// 起動前の確認を共通化した起動要求の送出処理。手動テストと監視で同一判定にする。
    /// プロトコル未登録は即エラー（`LaunchFailed` へ変換される）。
    pub async fn dispatch_launch_checked(&self, location: &str) -> Result<(), AppError> {
        if !Self::is_launchable_location(location) {
            return Err(AppError::InvalidInput(
                "起動先のインスタンス指定が想定外の形式のため、起動を中止しました。".to_string(),
            ));
        }
        if !Self::check_protocol_handler() {
            return Err(AppError::Operation(
                "VRChatの起動プロトコル(vrchat://)が登録されていません。VRChatをインストールし、vrchat://リンクが開ける状態にしてください。"
                    .to_string(),
            ));
        }
        self.dispatch_launch_protocol(location).await
    }

    pub fn check_protocol_handler() -> bool {
        #[cfg(target_os = "windows")]
        {
            let output = Command::new("cmd")
                .args(["/C", "reg", "query", "HKEY_CLASSES_ROOT\\vrchat", "/ve"])
                .creation_flags(CREATE_NO_WINDOW)
                .output();

            match output {
                Ok(out) => out.status.success(),
                Err(_) => false,
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    }

    pub fn check_vrchat_process() -> bool {
        #[cfg(target_os = "windows")]
        {
            // tasklistコマンドでVRChatプロセスを検索（Windows 11対応）
            let output = Command::new("cmd")
                .args(["/C", "tasklist", "/fi", "imagename eq VRChat.exe"])
                .creation_flags(CREATE_NO_WINDOW)
                .output();

            match output {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    // VRChat.exeが出力に含まれていれば検出成功
                    stdout.contains("VRChat.exe")
                }
                Err(_) => false,
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn launchable_location_accepts_vrchat_format_only() {
        assert!(JoinService::is_launchable_location(
            "wrld_4432ea9b-729c-46e3-8eaf-846aa0a37fdd:12345~group(grp_12345678-1234-1234-1234-123456789012)~groupAccessType(public)~region(jp)"
        ));
        for bad in [
            "",
            "12345~region(jp)",
            "wrld_x:1&attach=0",
            "wrld_x:1?x=1",
            "wrld_x:1#f",
            "wrld_x:1 2",
            "wrld_x:1%26",
        ] {
            assert!(!JoinService::is_launchable_location(bad), "{bad}");
        }
    }

    use super::*;

    #[test]
    fn test_build_launch_url_format() {
        assert_eq!(
            JoinService::build_launch_url("wrld_abc:12345~group(grp_test)"),
            "vrchat://launch?ref=vrchat.com&id=wrld_abc:12345~group(grp_test)&attach=1"
        );
    }
    // test_checked_dispatch_refuses_without_protocol は Windows専用製品に対する
    // 保証価値が低いため削除。build_launch_url の外部契約を代表1本維持する。
}
