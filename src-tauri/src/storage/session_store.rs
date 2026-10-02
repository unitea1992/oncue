use super::portable_paths::PortablePaths;
use super::session_crypto::{CryptoError, SessionCrypto};
use crate::domain::models::EventSchedule;
use chrono::Datelike;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("{0}")]
    Crypto(#[from] CryptoError),
    #[error("ファイルの読み書きに失敗しました: {0}")]
    Io(#[from] std::io::Error),
    #[error("保存データの形式が正しくありません（{}行目）", .0.line())]
    Serialization(#[from] serde_json::Error),
    #[error("保存済みのログイン状態がありません")]
    NotFound,
    #[error("保存済みのログイン状態が壊れています")]
    InvalidData,
    #[error("{0}")]
    InvalidInput(String),
}

/// Windows-safe atomic file write: temp in the same directory, fsync, rename.
/// Rename replaces the destination on both Windows and Unix. The temp name
/// carries the pid so two app instances never share it.
pub(crate) fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<(), SessionError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let tmp = path
        .to_str()
        .map(|s| format!("{}.tmp-{}.tmp", s, std::process::id()))
        .map(std::path::PathBuf::from)
        .ok_or(SessionError::InvalidData)?;

    let result = (|| -> std::io::Result<()> {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(SessionError::Io)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    pub auth_cookie: String,
    pub user_id: String,
    pub username: String,
    /// VRChatのユーザー画像URL。再起動後の表示復元用。旧セッションには無い。
    #[serde(default)]
    pub profile_icon_url: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub struct SessionStore {
    paths: PortablePaths,
    crypto: SessionCrypto,
}

impl SessionStore {
    pub fn new(paths: PortablePaths) -> Self {
        Self {
            paths,
            crypto: SessionCrypto::new(),
        }
    }

    /// テスト用：カスタムパスでセッションストアを作成
    pub fn with_custom_path(base_dir: &std::path::Path) -> Self {
        let paths = PortablePaths::with_base_dir(base_dir);
        Self {
            paths,
            crypto: SessionCrypto::new(),
        }
    }

    pub fn save(&self, session: &SessionData) -> Result<(), SessionError> {
        let json = serde_json::to_string(session)?;
        let encrypted = self
            .crypto
            .encrypt(json.as_bytes())
            .map_err(SessionError::Crypto)?;

        atomic_write_bytes(&self.paths.session_file(), &encrypted)
    }

    pub fn load(&self) -> Result<SessionData, SessionError> {
        let path = self.paths.session_file();

        if !path.exists() {
            return Err(SessionError::NotFound);
        }

        let encrypted = fs::read(&path)?;

        if encrypted.is_empty() {
            return Err(SessionError::InvalidData);
        }

        let decrypted = self
            .crypto
            .decrypt(&encrypted)
            .map_err(|_| SessionError::InvalidData)?;

        let json = String::from_utf8(decrypted).map_err(|_| SessionError::InvalidData)?;

        let session: SessionData = serde_json::from_str(&json)?;

        Ok(session)
    }
    pub fn clear(&self) -> Result<(), SessionError> {
        let path = self.paths.session_file();
        if path.exists() {
            fs::remove_file(&path)?;
        }
        Ok(())
    }

    pub fn exists(&self) -> bool {
        self.paths.session_file().exists()
    }
}

/// domain EventScheduleの保存前検証。once/weekly/biweeklyの形式を検証する。日付推測はしない。
fn validate_schedule(schedule: &EventSchedule) -> Result<(), SessionError> {
    let invalid = |e: crate::domain::models::AppError| SessionError::InvalidInput(e.to_string());
    match schedule {
        EventSchedule::Once { .. } => Ok(()),
        EventSchedule::Weekly { weekday, time } => {
            crate::domain::models::validate_weekday(*weekday).map_err(invalid)?;
            crate::domain::models::parse_daily_time_str(time)
                .map(|_| ())
                .map_err(invalid)
        }
        EventSchedule::Biweekly {
            weekday,
            time,
            anchor_date,
        } => {
            crate::domain::models::validate_weekday(*weekday).map_err(invalid)?;
            crate::domain::models::parse_daily_time_str(time)
                .map(|_| ())
                .map_err(invalid)?;
            let date = crate::domain::models::parse_anchor_date(anchor_date).map_err(invalid)?;
            if date.weekday().num_days_from_sunday() as u8 != *weekday {
                return Err(SessionError::InvalidInput(format!(
                    "基準日と曜日が一致しません: {}",
                    anchor_date
                )));
            }
            Ok(())
        }
    }
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let trimmed = v.trim().to_string();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    pub id: String,
    pub label: String,
    pub group_id: String,
    pub group_name: Option<String>,
    pub schedule: EventSchedule,
    pub source_event_id: Option<String>,
    pub preferred_instance_name: Option<String>,
}

impl Preset {
    /// 保存前バリデーション。不正は書き込む前に返すので半端な保存は残らない。
    pub fn validate(&self) -> Result<(), SessionError> {
        if self.id.trim().is_empty() {
            return Err(SessionError::InvalidInput("イベントIDが空です".to_string()));
        }
        if self.label.trim().is_empty() {
            return Err(SessionError::InvalidInput("イベント名が空です".to_string()));
        }
        if self.group_id.trim().is_empty() {
            return Err(SessionError::InvalidInput("グループIDが空です".to_string()));
        }
        if self.group_id.trim().chars().any(|c| c.is_whitespace()) {
            return Err(SessionError::InvalidInput(
                "グループIDに空白は使えません".to_string(),
            ));
        }
        validate_schedule(&self.schedule)?;
        Ok(())
    }

    /// 任意フィールドの空文字はNoneへ、端の空白は除去する。
    pub fn normalize(&mut self) {
        self.id = self.id.trim().to_string();
        self.label = self.label.trim().to_string();
        self.group_id = self.group_id.trim().to_string();
        self.group_name = normalize_optional(self.group_name.take());
        self.source_event_id = normalize_optional(self.source_event_id.take());
        self.preferred_instance_name = normalize_optional(self.preferred_instance_name.take());
        if let EventSchedule::Weekly { time, .. } = &mut self.schedule {
            *time = time.trim().to_string();
        }
        if let EventSchedule::Biweekly {
            time, anchor_date, ..
        } = &mut self.schedule
        {
            *time = time.trim().to_string();
            *anchor_date = anchor_date.trim().to_string();
        }
    }
}

/// 0.1.0のactual legacy shapeへdecodeできるpresetだけ新規扱いする。
/// marker文字列の有無ではなくdetection-only structへのdecode可否で判定する。
/// fieldはshape検証用で読み出さないためdead_codeを許容する。
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyEventStartPreset {
    id: String,
    label: String,
    group_id: String,
    #[serde(default)]
    group_name: Option<String>,
    event_start_time: chrono::NaiveTime,
    #[serde(default)]
    preferred_instance_name: Option<String>,
}

/// legacy daily形式として実際に成立している必要がある。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyDailySchedule {
    kind: String,
    time: String,
}

/// fieldはshape検証用で読み出さないためdead_codeを許容する。
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyDailyPreset {
    id: String,
    label: String,
    group_id: String,
    #[serde(default)]
    group_name: Option<String>,
    schedule: LegacyDailySchedule,
    #[serde(default)]
    source_event_id: Option<String>,
    #[serde(default)]
    preferred_instance_name: Option<String>,
}

/// 既知legacy schemaとして成立している場合のみtrue。
/// broken object・current混入・型不正・必須欠落はすべてfalse。
fn is_valid_known_legacy_preset(value: &serde_json::Value) -> bool {
    if let Ok(preset) = serde_json::from_value::<LegacyEventStartPreset>(value.clone()) {
        return !preset.id.is_empty() && !preset.label.is_empty() && !preset.group_id.is_empty();
    }
    if let Ok(preset) = serde_json::from_value::<LegacyDailyPreset>(value.clone()) {
        return !preset.id.is_empty()
            && !preset.label.is_empty()
            && !preset.group_id.is_empty()
            && preset.schedule.kind == "daily"
            && crate::domain::models::parse_daily_time_str(&preset.schedule.time).is_ok();
    }
    false
}

/// 配列が空でなく全要素が既知の旧形式として妥当なときのみ新規扱いしてよい。
/// 現行・未知・破損のいずれかが1件でも混ざれば安全側で失敗させるためfalse。
fn all_known_legacy_presets(value: &serde_json::Value) -> bool {
    value.as_array().is_some_and(|presets| {
        !presets.is_empty() && presets.iter().all(is_valid_known_legacy_preset)
    })
}

/// 現行の `schedule` オブジェクトを持つプリセットに旧 `eventStartTime` キーが混入したら
/// 破損として扱う（serdeは未知キーを黙って捨てるため、構造で先に検出する）。
fn has_stray_legacy_key(value: &serde_json::Value) -> bool {
    value.as_array().is_some_and(|presets| {
        presets.iter().any(|preset| {
            preset
                .as_object()
                .is_some_and(|p| p.contains_key("schedule") && p.contains_key("eventStartTime"))
        })
    })
}

fn parse_presets_json(json: &str) -> Result<Vec<Preset>, SessionError> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(json) {
        if has_stray_legacy_key(&value) {
            return Err(SessionError::InvalidInput(
                "旧eventStartTimeキーと現行scheduleの混在は不正".to_string(),
            ));
        }
    }
    match serde_json::from_str::<Vec<Preset>>(json) {
        Ok(presets) => Ok(presets),
        Err(e) => {
            let value = match serde_json::from_str::<serde_json::Value>(json) {
                Ok(value) => value,
                Err(_) => return Err(SessionError::Serialization(e)),
            };
            if all_known_legacy_presets(&value) {
                Ok(Vec::new())
            } else {
                Err(SessionError::Serialization(e))
            }
        }
    }
}

fn default_theme() -> String {
    "light".to_string()
}

fn is_valid_theme(theme: &str) -> bool {
    matches!(theme, "light" | "dark" | "system")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    #[serde(default)]
    pub presets: Vec<Preset>,
    #[serde(default)]
    pub selected_preset_id: Option<String>,
    #[serde(default)]
    pub debug_mode: bool,
    #[serde(default = "default_theme")]
    pub theme: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            presets: Vec::new(),
            selected_preset_id: None,
            debug_mode: false,
            theme: default_theme(),
        }
    }
}

impl AppConfig {
    fn validate_scalars(&self) -> Result<(), SessionError> {
        if !is_valid_theme(self.theme.trim()) {
            return Err(SessionError::InvalidInput(format!(
                "テーマはlight/dark/systemのいずれかです: {}",
                self.theme
            )));
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), SessionError> {
        self.validate_scalars()?;
        for preset in &self.presets {
            preset.validate()?;
        }
        Ok(())
    }
}

/// 0.1.0のactual legacy config shape。存在したfieldだけを持ち、未知fieldはdenyする。
/// fieldはshape検証用で読み出さないためdead_codeを許容する。
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyConfigShape {
    #[serde(default)]
    presets: Vec<serde_json::Value>,
    #[serde(default)]
    selected_preset_id: Option<String>,
    #[serde(default)]
    debug_mode: bool,
    #[serde(default = "default_theme")]
    theme: String,
}

/// 設定全体が既知の形状として矛盾なく、`presets` が空でなく全件旧形式として妥当な
/// ときのみ新規扱いしてよい。単一値の破損が混ざれば安全側で失敗させる。
fn is_known_legacy_config(value: &serde_json::Value) -> bool {
    let Ok(config) = serde_json::from_value::<LegacyConfigShape>(value.clone()) else {
        return false;
    };
    if !is_valid_theme(&config.theme) {
        return false;
    }
    !config.presets.is_empty() && config.presets.iter().all(is_valid_known_legacy_preset)
}

fn parse_config_json(json: &str) -> Result<AppConfig, SessionError> {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(json) {
        if value.get("presets").is_some_and(has_stray_legacy_key) {
            return Err(SessionError::InvalidInput(
                "旧eventStartTimeキーと現行scheduleの混在は不正".to_string(),
            ));
        }
    }
    match serde_json::from_str::<AppConfig>(json) {
        Ok(config) => Ok(config),
        Err(e) => {
            let value = match serde_json::from_str::<serde_json::Value>(json) {
                Ok(value) => value,
                Err(_) => return Err(SessionError::Serialization(e)),
            };
            if is_known_legacy_config(&value) {
                Ok(AppConfig::default())
            } else {
                Err(SessionError::Serialization(e))
            }
        }
    }
}

pub struct ConfigStore {
    paths: PortablePaths,
    /// read-modify-writeの直列化。load+saveを同一ガード下で行う。
    lock: std::sync::Mutex<()>,
}

impl ConfigStore {
    pub fn new(paths: PortablePaths) -> Self {
        Self {
            paths,
            lock: std::sync::Mutex::new(()),
        }
    }

    fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        self.lock.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// ガード配下の実処理。presets.jsonが正本で、検証済みだけ返す。
    fn load_presets_locked(&self) -> Result<Vec<Preset>, SessionError> {
        let path = self.paths.presets_file();

        if !path.exists() {
            return Ok(Vec::new());
        }

        let json = fs::read_to_string(&path)?;
        // 現行parse成功→Ok。既知0.1.0 legacyのみ新規扱い、それ以外はErr。読取で壊さない。
        // 形式は正しいが内容が不正なものは検証でエラーにする。
        let presets = parse_presets_json(&json)?;
        for preset in &presets {
            preset.validate()?;
        }
        Ok(presets)
    }

    pub fn load_presets(&self) -> Result<Vec<Preset>, SessionError> {
        let _guard = self.guard();
        self.load_presets_locked()
    }

    pub fn save_presets(&self, presets: &[Preset]) -> Result<(), SessionError> {
        for preset in presets {
            preset.validate()?;
        }
        let _guard = self.guard();
        let json = serde_json::to_string_pretty(presets)?;
        atomic_write_bytes(&self.paths.presets_file(), json.as_bytes())
    }

    /// load→変更→saveを1ガード下で直列化し、更新競合のlost updateを防ぐ。
    pub fn update_presets(
        &self,
        f: impl FnOnce(&mut Vec<Preset>) -> Result<(), SessionError>,
    ) -> Result<(), SessionError> {
        let _guard = self.guard();
        let mut presets = self.load_presets_locked()?;
        f(&mut presets)?;
        for preset in &presets {
            preset.validate()?;
        }
        let json = serde_json::to_string_pretty(&presets)?;
        atomic_write_bytes(&self.paths.presets_file(), json.as_bytes())?;
        Ok(())
    }

    /// ガード配下の実処理。presetsはpresets.jsonを正本として上書きする。
    fn load_config_locked(&self) -> Result<AppConfig, SessionError> {
        let path = self.paths.config_file();
        if !path.exists() {
            return Ok(AppConfig {
                presets: self.load_presets_locked()?,
                ..AppConfig::default()
            });
        }

        let json = fs::read_to_string(&path)?;
        let mut config = parse_config_json(&json)?;
        config.validate_scalars()?;
        if self.paths.presets_file().exists() {
            config.presets = self.load_presets_locked()?;
        } else {
            for preset in &config.presets {
                preset.validate()?;
            }
        }
        Ok(config)
    }

    pub fn load_config(&self) -> Result<AppConfig, SessionError> {
        let _guard = self.guard();
        self.load_config_locked()
    }

    pub fn save_config(&self, config: &AppConfig) -> Result<(), SessionError> {
        config.validate()?;
        let _guard = self.guard();
        let json = serde_json::to_string_pretty(config)?;
        atomic_write_bytes(&self.paths.config_file(), json.as_bytes())
    }

    pub fn update_config(
        &self,
        f: impl FnOnce(&mut AppConfig) -> Result<(), SessionError>,
    ) -> Result<(), SessionError> {
        let _guard = self.guard();
        let mut config = self.load_config_locked()?;
        f(&mut config)?;
        // presetsは正本で上書きし、f経由の混入も黙って通さない。
        config.presets = self.load_presets_locked()?;
        config.validate()?;
        let json = serde_json::to_string_pretty(&config)?;
        atomic_write_bytes(&self.paths.config_file(), json.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_store() -> (ConfigStore, TempDir) {
        let dir = TempDir::new().unwrap();
        let paths = PortablePaths::with_base_dir(dir.path());
        (ConfigStore::new(paths), dir)
    }

    fn sample_preset(id: &str) -> Preset {
        Preset {
            id: id.to_string(),
            label: "イベント".to_string(),
            group_id: "grp_test".to_string(),
            group_name: None,
            schedule: EventSchedule::Weekly {
                weekday: 0,
                time: "21:00".to_string(),
            },
            source_event_id: None,
            preferred_instance_name: None,
        }
    }

    #[test]
    fn session_icon_url_legacy_missing_stays_readable() {
        // 画像URLなしの旧セッションはNoneとして読める（互換性の代表ケース）。
        let legacy: SessionData = serde_json::from_str(
            r#"{"auth_cookie":"c","user_id":"u","username":"n","created_at":"2026-01-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(legacy.profile_icon_url, None);
    }

    #[test]
    fn schedule_serde_shape_matches_contract() {
        let once = Preset {
            schedule: EventSchedule::Once {
                starts_at: "2026-09-07T12:00:00Z".parse().unwrap(),
            },
            ..sample_preset("a")
        };
        let json = serde_json::to_value(&once).unwrap();
        assert_eq!(json["schedule"]["kind"], "once");
        assert_eq!(json["schedule"]["startsAt"], "2026-09-07T12:00:00Z");
        assert_eq!(json["groupId"], "grp_test");

        let weekly = sample_preset("b");
        let json = serde_json::to_value(&weekly).unwrap();
        assert_eq!(
            json["schedule"],
            serde_json::json!({"kind": "weekly", "weekday": 0, "time": "21:00"})
        );
    }

    #[test]
    fn known_legacy_presets_fall_back_to_empty_table() {
        // 旧形式の引き継ぎはしない。読めない保存は新規扱いにし、読取で壊さない。
        let cases: Vec<(&str, &str)> = vec![
            (
                "eventStartTime単体",
                r#"[{
            "id": "p1", "label": "旧イベント", "groupId": "grp_old",
            "groupName": "旧", "eventStartTime": "21:30:00",
            "preferredInstanceName": "会場A"
        }]"#,
            ),
            (
                "daily単体",
                r#"[{"id":"p1","label":"旧","groupId":"grp_old","schedule":{"kind":"daily","time":"21:30"}}]"#,
            ),
            (
                "eventStartTimeとdailyの混在",
                r#"[
            {"id":"o1","label":"旧1","groupId":"grp_old","eventStartTime":"21:00:00"},
            {"id":"o2","label":"旧2","groupId":"grp_old","eventStartTime":"22:00:00"},
            {"id":"o3","label":"旧3","groupId":"grp_old","schedule":{"kind":"daily","time":"21:30"}}
        ]"#,
            ),
        ];
        for (name, legacy) in cases {
            let (store, _dir) = test_store();
            let path = store.paths.presets_file();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, legacy).unwrap();
            assert!(
                store.load_presets().unwrap().is_empty(),
                "legacy case: {}",
                name
            );
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                legacy,
                "legacy case: {}",
                name
            );
        }
    }

    #[test]
    fn invalid_presets_rejected_before_write_table() {
        // storage境界では不正値は書込前に拒否することだけを代表確認する。
        // 曜日・時刻の詳細検証は domain 正本が担う。
        let (store, _dir) = test_store();
        let mut bad_group = sample_preset("bad-group");
        bad_group.group_id = "  ".to_string();
        assert!(store.save_presets(&[bad_group]).is_err());
        assert!(!store.paths.presets_file().exists());
        let mut bad_weekday = sample_preset("bad-weekday");
        bad_weekday.schedule = EventSchedule::Weekly {
            weekday: 7,
            time: "21:30".to_string(),
        };
        assert!(store.save_presets(&[bad_weekday]).is_err());
        let mut bad_time = sample_preset("bad-time");
        bad_time.schedule = EventSchedule::Weekly {
            weekday: 0,
            time: "99:99".to_string(),
        };
        assert!(store.save_presets(&[bad_time]).is_err());
    }

    #[test]
    fn theme_defaults_to_light_and_keeps_explicit() {
        let (store, _dir) = test_store();
        assert_eq!(store.load_config().unwrap().theme, "light");

        let config = AppConfig {
            theme: "dark".to_string(),
            ..AppConfig::default()
        };
        store.save_config(&config).unwrap();
        assert_eq!(store.load_config().unwrap().theme, "dark");

        let bad = AppConfig {
            theme: "neon".to_string(),
            ..AppConfig::default()
        };
        assert!(store.save_config(&bad).is_err());
    }

    #[test]
    fn concurrent_updates_do_not_lose_writes() {
        let (store, dir) = test_store();
        let store = std::sync::Arc::new(store);
        let _ = dir;
        let mut handles = Vec::new();
        for i in 0..8 {
            let store = store.clone();
            handles.push(std::thread::spawn(move || {
                store
                    .update_presets(|presets| {
                        presets.push(Preset {
                            id: format!("p{}", i),
                            ..sample_preset(&format!("p{}", i))
                        });
                        Ok(())
                    })
                    .unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let presets = store.load_presets().unwrap();
        assert_eq!(presets.len(), 8);
    }

    #[test]
    fn legacy_config_falls_back_to_default_table() {
        // 旧形式の設定は引き継がず既定値で新規扱いにし、読取で壊さない。
        let cases: Vec<(&str, &str)> = vec![
            (
                "eventStartTime旧形式",
                r#"{"presets": [{"id":"p1","label":"旧","groupId":"grp_old","eventStartTime":"20:00:00"}],
                "selectedPresetId": "p1", "debugMode": false, "theme": "system"}"#,
            ),
            (
                "daily旧形式",
                r#"{"presets": [{"id":"p1","label":"旧","groupId":"grp_old","schedule":{"kind":"daily","time":"20:00"}}],
                "selectedPresetId": "p1", "debugMode": false, "theme": "system"}"#,
            ),
        ];
        for (name, legacy) in cases {
            let (store, _dir) = test_store();
            let path = store.paths.config_file();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, legacy).unwrap();
            let config = store.load_config().unwrap();
            assert_eq!(config.theme, "light", "case: {}", name);
            assert!(config.presets.is_empty(), "case: {}", name);
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                legacy,
                "case: {}",
                name
            );
        }
    }

    #[test]
    fn invalid_presets_fail_closed_table() {
        // 壊れたJSON・現行shape破損・unknown schedule・legacy+current混在・
        // legacy風だが型不正の代表同値クラスを1本で確認する。読取で上書きしない。
        let cases: Vec<(&str, &str)> = vec![
            ("壊れたJSON", "not json"),
            (
                "現行shape破損",
                r#"[{"id":"x","label":"y","groupId":"grp_z","schedule":{"kind":"weekly","weekday":0,"time":"21:00"#,
            ),
            (
                "unknown schedule",
                r#"[{"id":"p1","label":"旧","groupId":"grp_old","schedule":{"kind":"monthly","time":"21:30"}}]"#,
            ),
            (
                "legacyと現行の混在",
                r#"[
            {"id":"old","label":"legacy","groupId":"grp_old","eventStartTime":"21:00:00"},
            {"id":"broken","label":"current","groupId":"grp_new","schedule":{"kind":"weekly","weekday":0}}
        ]"#,
            ),
            (
                "legacy風だが型不正",
                r#"[{"id":"old","label":"Old","groupId":"grp_old","eventStartTime": null}]"#,
            ),
            (
                "内容不正",
                r#"[{"id":"p1","label":"x","groupId":"  ",
            "schedule":{"kind":"weekly","weekday":0,"time":"21:00"}}]"#,
            ),
        ];
        for (name, contents) in cases {
            let (store, _dir) = test_store();
            let path = store.paths.presets_file();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, contents).unwrap();
            assert!(store.load_presets().is_err(), "case: {}", name);
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                contents,
                "case: {}",
                name
            );
        }
    }

    #[test]
    fn config_patch_preserves_presets() {
        // save_config全面置換のlost update防衛: スカラ更新はpresetsを消さない。
        let (store, _dir) = test_store();
        store.save_presets(&[sample_preset("keep")]).unwrap();
        store
            .update_config(|config| {
                config.theme = "dark".to_string();
                config.selected_preset_id = Some("keep".to_string());
                Ok(())
            })
            .unwrap();
        let loaded = store.load_config().unwrap();
        assert_eq!(loaded.theme, "dark");
        assert_eq!(loaded.presets.len(), 1);
        assert_eq!(loaded.presets[0].id, "keep");
    }

    #[test]
    fn current_valid_presets_and_config_read() {
        let (store, _dir) = test_store();
        let preset = sample_preset("ok");
        store.save_presets(&[preset]).unwrap();
        let loaded = store.load_presets().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "ok");
        let config = AppConfig {
            theme: "dark".to_string(),
            ..AppConfig::default()
        };
        store.save_config(&config).unwrap();
        assert_eq!(store.load_config().unwrap().theme, "dark");
    }

    #[test]
    fn corrupt_config_with_legacy_presets_fails_closed() {
        // theme/debug/selectedPresetId型違反は代表1ケース（theme）で確認する。
        let (store, _dir) = test_store();
        let path = store.paths.config_file();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let legacy_presets = r#"[{"id":"p1","label":"旧","groupId":"grp_old","schedule":{"kind":"daily","time":"20:00"}}]"#;
        let bad_theme = format!(
            r#"{{"presets": {presets},"selectedPresetId":null,"debugMode":false,"theme":123}}"#,
            presets = legacy_presets
        );
        std::fs::write(&path, &bad_theme).unwrap();
        assert!(store.load_config().is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), bad_theme);
    }

    // legacy_config_keeps_file_unchanged は上記へ統合したため削除。
}
