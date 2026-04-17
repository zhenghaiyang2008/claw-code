use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde::de::Deserializer;
use serde_json::Value;

use crate::omc_compat::normalize_mode_name;

static WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub enum ModeStateError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Format(String),
}

impl Display for ModeStateError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
            Self::Format(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ModeStateError {}

impl From<std::io::Error> for ModeStateError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ModeStateError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModeStateRecord {
    pub mode: String,
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub iteration: Option<u64>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_rfc3339_timestamp"
    )]
    pub started_at: Option<String>,
    #[serde(deserialize_with = "deserialize_rfc3339_timestamp")]
    pub updated_at: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_optional_rfc3339_timestamp"
    )]
    pub completed_at: Option<String>,
    #[serde(default = "default_context")]
    pub context: Value,
}

impl ModeStateRecord {
    #[must_use]
    pub fn new(mode: impl Into<String>, active: bool) -> Self {
        let mode = mode.into();
        let now = iso8601_now();
        Self {
            mode: normalize_mode_name(&mode).to_string(),
            active,
            current_phase: None,
            session_id: None,
            iteration: None,
            started_at: Some(now.clone()),
            updated_at: now,
            completed_at: None,
            context: default_context(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeStateSummary {
    pub mode: String,
    pub session_id: Option<String>,
    pub active: bool,
    pub current_phase: Option<String>,
    pub updated_at: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeStateStore {
    workspace_root: PathBuf,
}

impl ModeStateStore {
    #[must_use]
    pub fn new() -> Self {
        let workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::for_workspace(workspace_root)
    }

    #[must_use]
    pub fn for_workspace(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
        }
    }

    #[must_use]
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    #[must_use]
    pub fn state_root(&self) -> PathBuf {
        self.workspace_root.join(".omx").join("state")
    }

    #[must_use]
    pub fn mode_path(&self, mode: &str, session_id: Option<&str>) -> PathBuf {
        let mode = normalize_mode_name(mode);
        match session_id {
            Some(session_id) => self.session_mode_path(mode, session_id),
            None => self.global_mode_path(mode),
        }
    }

    pub fn write(&self, record: &ModeStateRecord) -> Result<PathBuf, ModeStateError> {
        let mut normalized_record = record.clone();
        normalized_record.mode = normalize_mode_name(&normalized_record.mode).to_string();
        let rendered = serde_json::to_string_pretty(&normalized_record)?;
        let global_path = self.global_mode_path(&normalized_record.mode);
        let previous_global_contents = fs::read_to_string(&global_path).ok();
        write_atomic(&global_path, &rendered)?;

        if let Some(session_id) = normalized_record.session_id.as_deref() {
            let session_path = self.session_mode_path(&normalized_record.mode, session_id);
            if let Err(error) = write_atomic(&session_path, &rendered) {
                restore_file(&global_path, previous_global_contents.as_deref())?;
                return Err(error.into());
            }
            return Ok(session_path);
        }

        Ok(global_path)
    }

    pub fn read(
        &self,
        mode: &str,
        session_id: Option<&str>,
    ) -> Result<Option<ModeStateRecord>, ModeStateError> {
        for path in self.mode_path_candidates(mode, session_id) {
            if let Some(record) = self.read_mode_file(&path)? {
                return Ok(Some(record));
            }
        }
        Ok(None)
    }

    pub fn clear(&self, mode: &str, session_id: Option<&str>) -> Result<bool, ModeStateError> {
        let mode = normalize_mode_name(mode);
        match session_id {
            Some(session_id) => {
                let mut removed = false;
                for path in self.mode_path_candidates(mode, Some(session_id)) {
                    removed |= remove_file_if_present(&path)?;
                }

                for global_path in self.mode_path_candidates(mode, None) {
                    if let Some(global_record) = self.read_mode_file(&global_path)? {
                        if global_record.session_id.as_deref() == Some(session_id) {
                            removed |= remove_file_if_present(&global_path)?;
                        }
                    }
                }
                Ok(removed)
            }
            None => {
                let mut removed = false;
                for path in self.mode_path_candidates(mode, None) {
                    removed |= remove_file_if_present(&path)?;
                }
                Ok(removed)
            }
        }
    }

    pub fn list_active(&self) -> Result<Vec<ModeStateSummary>, ModeStateError> {
        let mut summaries = BTreeMap::new();
        self.collect_active_from_dir(&self.state_root(), None, &mut summaries)?;

        let sessions_root = self.state_root().join("sessions");
        let session_dirs = match fs::read_dir(&sessions_root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(summaries.into_values().collect())
            }
            Err(error) => return Err(error.into()),
        };

        for entry in session_dirs {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let session_id = entry.file_name().to_string_lossy().to_string();
            self.collect_active_from_dir(&path, Some(session_id), &mut summaries)?;
        }

        let mut summaries: Vec<_> = summaries.into_values().collect();
        summaries.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.mode.cmp(&right.mode))
        });
        Ok(summaries)
    }

    fn collect_active_from_dir(
        &self,
        directory: &Path,
        session_id: Option<String>,
        summaries: &mut BTreeMap<(String, Option<String>), ModeStateSummary>,
    ) -> Result<(), ModeStateError> {
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|value| value.to_str()) != Some("json")
            {
                continue;
            }
            let file_name = entry.file_name().to_string_lossy().to_string();
            if file_name.strip_suffix("-state.json").is_none() {
                continue;
            }
            let Some(record) = self.read_mode_file(&path)? else {
                continue;
            };
            if !record.active {
                continue;
            }
            let effective_session_id = session_id.clone().or_else(|| record.session_id.clone());
            let summary = ModeStateSummary {
                mode: record.mode.clone(),
                session_id: effective_session_id.clone(),
                active: record.active,
                current_phase: record.current_phase.clone(),
                updated_at: record.updated_at,
                path: path.clone(),
            };
            let key = (summary.mode.clone(), effective_session_id);
            match summaries.get(&key) {
                Some(existing)
                    if existing.updated_at > summary.updated_at
                        || (existing.updated_at == summary.updated_at
                            && existing.path.components().count()
                                >= summary.path.components().count()) => {}
                _ => {
                    summaries.insert(key, summary);
                }
            }
        }
        Ok(())
    }

    fn global_mode_path(&self, mode: &str) -> PathBuf {
        self.state_root().join(format!("{mode}-state.json"))
    }

    fn session_mode_path(&self, mode: &str, session_id: &str) -> PathBuf {
        self.state_root()
            .join("sessions")
            .join(session_id)
            .join(format!("{mode}-state.json"))
    }

    fn mode_path_candidates(&self, mode: &str, session_id: Option<&str>) -> Vec<PathBuf> {
        let mode = normalize_mode_name(mode);
        let mut candidates = Vec::with_capacity(1 + legacy_mode_file_aliases(mode).len());
        let canonical_path = match session_id {
            Some(session_id) => self.session_mode_path(mode, session_id),
            None => self.global_mode_path(mode),
        };
        candidates.push(canonical_path);

        for alias in legacy_mode_file_aliases(mode) {
            let path = match session_id {
                Some(session_id) => self.session_mode_path(alias, session_id),
                None => self.global_mode_path(alias),
            };
            if !candidates.contains(&path) {
                candidates.push(path);
            }
        }

        candidates
    }

    fn read_mode_file(&self, path: &Path) -> Result<Option<ModeStateRecord>, ModeStateError> {
        if !path.exists() {
            return Ok(None);
        }
        let contents = fs::read_to_string(path)?;
        let mut record: ModeStateRecord = serde_json::from_str(&contents)?;
        record.mode = normalize_mode_name(&record.mode).to_string();
        Ok(Some(record))
    }
}

fn default_context() -> Value {
    Value::Object(serde_json::Map::new())
}

fn legacy_mode_file_aliases(mode: &str) -> &'static [&'static str] {
    match mode {
        "deep-interview" => &["deep_interview", "deep interview", "deepinterview"],
        "ultrawork" => &["ultra-work", "ultra_work", "ultra work"],
        "verification" => &["verify", "verifier", "verificationagent", "verification-agent", "verification_agent"],
        "team" => &["swarm"],
        _ => &[],
    }
}

fn iso8601_now() -> String {
    format_rfc3339(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn format_rfc3339(secs: u64) -> String {
    let days_since_epoch = secs / 86_400;
    let seconds_of_day = secs % 86_400;
    let hours = seconds_of_day / 3_600;
    let minutes = (seconds_of_day % 3_600) / 60;
    let seconds = seconds_of_day % 60;
    let (year, month, day) = civil_from_days(i64::try_from(days_since_epoch).unwrap_or(0));
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

#[allow(
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::cast_possible_truncation
)]
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 {
        z / 146_097
    } else {
        (z - 146_096) / 146_097
    };
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = y + i64::from(m <= 2);
    (y as i32, m as u32, d as u32)
}

fn write_atomic(path: &Path, contents: &str) -> Result<(), ModeStateError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = temporary_path_for(path);
    fs::write(&temp_path, contents)?;
    replace_file(&temp_path, path)?;
    Ok(())
}

fn remove_file_if_present(path: &Path) -> Result<bool, ModeStateError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn deserialize_rfc3339_timestamp<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum TimestampRepr {
        String(String),
        Integer(u64),
    }

    match TimestampRepr::deserialize(deserializer)? {
        TimestampRepr::String(value) => Ok(value),
        TimestampRepr::Integer(value) => Ok(format_rfc3339(value)),
    }
}

fn deserialize_optional_rfc3339_timestamp<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OptionalTimestampRepr {
        String(String),
        Integer(u64),
        Null,
    }

    Ok(match OptionalTimestampRepr::deserialize(deserializer)? {
        OptionalTimestampRepr::String(value) => Some(value),
        OptionalTimestampRepr::Integer(value) => Some(format_rfc3339(value)),
        OptionalTimestampRepr::Null => None,
    })
}

fn temporary_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("state");
    path.with_file_name(format!(
        "{file_name}.tmp-{}-{}",
        unix_now_secs(),
        WRITE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

fn replace_file(temp_path: &Path, path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        if path.exists() {
            fs::remove_file(path)?;
        }
    }

    fs::rename(temp_path, path)
}

fn restore_file(path: &Path, previous_contents: Option<&str>) -> std::io::Result<()> {
    match previous_contents {
        Some(contents) => {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(path, contents)
        }
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{ModeStateRecord, ModeStateStore};
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TestWorkspace {
        root: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time should be after epoch")
                .as_nanos();
            let root = std::env::temp_dir().join(format!("runtime-mode-state-{nanos}"));
            fs::create_dir_all(&root).expect("workspace root should exist");
            Self { root }
        }

        fn store(&self) -> ModeStateStore {
            ModeStateStore::for_workspace(&self.root)
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn writes_and_reads_global_mode_state() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();
        let mut record = ModeStateRecord::new("ultrawork", true);
        record.current_phase = Some("dispatch".to_string());
        record.context = json!({ "current_task_ids": ["task_1"] });

        let path = store.write(&record).expect("state should write");
        assert!(path.ends_with(PathBuf::from(".omx/state/ultrawork-state.json")));

        let restored = store
            .read("ultrawork", None)
            .expect("read should succeed")
            .expect("state should exist");
        assert_eq!(restored.mode, "ultrawork");
        assert_eq!(restored.current_phase.as_deref(), Some("dispatch"));
        assert_eq!(restored.context, json!({ "current_task_ids": ["task_1"] }));
        assert!(looks_like_rfc3339(restored.updated_at.as_str()));
        assert!(looks_like_rfc3339(
            restored.started_at.as_deref().expect("started_at should exist")
        ));
    }

    #[test]
    fn writes_and_reads_session_scoped_mode_state() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();
        let mut record = ModeStateRecord::new("deep-interview", true);
        record.session_id = Some("session-123".to_string());
        record.iteration = Some(3);

        let path = store.write(&record).expect("state should write");
        assert!(path.ends_with(PathBuf::from(
            ".omx/state/sessions/session-123/deep-interview-state.json"
        )));

        let restored = store
            .read("deep-interview", Some("session-123"))
            .expect("read should succeed")
            .expect("state should exist");
        assert_eq!(restored.iteration, Some(3));
        assert_eq!(restored.session_id.as_deref(), Some("session-123"));
        assert!(looks_like_rfc3339(restored.updated_at.as_str()));

        let latest = store
            .read("deep-interview", None)
            .expect("global alias should read")
            .expect("global alias should exist");
        assert_eq!(latest.session_id.as_deref(), Some("session-123"));
        assert!(store
            .mode_path("deep-interview", None)
            .ends_with(PathBuf::from(".omx/state/deep-interview-state.json")));
    }

    #[test]
    fn clear_removes_state_file() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();
        let record = ModeStateRecord::new("team", true);
        store.write(&record).expect("state should write");

        assert!(store.clear("team", None).expect("clear should succeed"));
        assert!(store.read("team", None).expect("read should succeed").is_none());
    }

    #[test]
    fn list_active_includes_global_and_session_scoped_records() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();

        let mut global = ModeStateRecord::new("ultrawork", true);
        global.current_phase = Some("dispatch".to_string());
        global.updated_at = "2026-04-16T10:00:00Z".to_string();
        store.write(&global).expect("global state should write");

        let mut session_scoped = ModeStateRecord::new("ralph", true);
        session_scoped.session_id = Some("session-abc".to_string());
        session_scoped.current_phase = Some("verify".to_string());
        session_scoped.updated_at = "2026-04-16T10:05:00Z".to_string();
        store
            .write(&session_scoped)
            .expect("session state should write");

        let active = store.list_active().expect("list should succeed");
        assert_eq!(active.len(), 2);
        assert_eq!(active[0].mode, "ralph");
        assert_eq!(active[0].session_id.as_deref(), Some("session-abc"));
        assert_eq!(active[1].mode, "ultrawork");
    }

    #[test]
    fn clear_session_scoped_state_removes_global_alias_for_same_session() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();
        let mut record = ModeStateRecord::new("team", true);
        record.session_id = Some("session-xyz".to_string());
        store.write(&record).expect("state should write");

        assert!(store
            .clear("team", Some("session-xyz"))
            .expect("clear should succeed"));
        assert!(store
            .read("team", Some("session-xyz"))
            .expect("session read should succeed")
            .is_none());
        assert!(store
            .read("team", None)
            .expect("global read should succeed")
            .is_none());
    }

    #[test]
    fn list_active_deduplicates_global_aliases_for_session_records() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();
        let mut record = ModeStateRecord::new("deep-interview", true);
        record.session_id = Some("session-456".to_string());
        record.current_phase = Some("question".to_string());
        store.write(&record).expect("state should write");

        let active = store.list_active().expect("list should succeed");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].mode, "deep-interview");
        assert_eq!(active[0].session_id.as_deref(), Some("session-456"));
        assert!(active[0]
            .path
            .ends_with(PathBuf::from(".omx/state/sessions/session-456/deep-interview-state.json")));
    }

    #[test]
    fn serialized_mode_state_uses_rfc3339_timestamp_strings() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();
        let record = ModeStateRecord::new("ultrawork", true);
        let path = store.write(&record).expect("state should write");

        let serialized = fs::read_to_string(path).expect("state file should be readable");
        let json: serde_json::Value = serde_json::from_str(&serialized).expect("valid json");
        assert!(json["updated_at"].is_string());
        assert!(json["started_at"].is_string());
        assert!(looks_like_rfc3339(
            json["updated_at"].as_str().expect("updated_at string")
        ));
    }

    #[test]
    fn reads_legacy_numeric_timestamps_as_rfc3339_strings() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();
        let path = store.mode_path("ralph", None);
        fs::create_dir_all(path.parent().expect("state parent")).expect("state parent should exist");
        fs::write(
            &path,
            r#"{
  "mode": "ralph",
  "active": true,
  "updated_at": 1673786096,
  "started_at": 1673786036,
  "completed_at": null,
  "context": {}
}"#,
        )
        .expect("legacy state should write");

        let restored = store
            .read("ralph", None)
            .expect("read should succeed")
            .expect("state should exist");
        assert_eq!(restored.updated_at, "2023-01-15T12:34:56Z");
        assert_eq!(restored.started_at.as_deref(), Some("2023-01-15T12:33:56Z"));
    }

    #[test]
    fn session_write_failure_rolls_back_global_alias() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();

        let mut original = ModeStateRecord::new("deep-interview", true);
        original.current_phase = Some("question".to_string());
        original.updated_at = "2026-04-16T10:00:00Z".to_string();
        store.write(&original).expect("initial global state should write");

        let sessions_path = workspace.root.join(".omx").join("state").join("sessions");
        fs::write(&sessions_path, "blocked").expect("sessions path should become a file");

        let mut updated = ModeStateRecord::new("deep-interview", true);
        updated.session_id = Some("session-fail".to_string());
        updated.current_phase = Some("handoff".to_string());
        updated.updated_at = "2026-04-16T10:05:00Z".to_string();
        let error = store.write(&updated).expect_err("session-scoped write should fail");
        let rendered = error.to_string();
        assert!(rendered.contains("directory") || rendered.contains("Not a directory"));

        let restored = store
            .read("deep-interview", None)
            .expect("global alias should read")
            .expect("global alias should still exist");
        assert_eq!(restored.current_phase.as_deref(), Some("question"));
        assert!(store
            .read("deep-interview", Some("session-fail"))
            .expect("session read should succeed")
            .is_none());
    }

    #[test]
    fn mode_state_normalizes_omc_mode_aliases_for_storage() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();
        let record = ModeStateRecord::new(" Deep_Interview ", true);

        let path = store.write(&record).expect("state should write");
        assert!(path.ends_with(PathBuf::from(".omx/state/deep-interview-state.json")));

        let restored = store
            .read("deep interview", None)
            .expect("alias read should succeed")
            .expect("state should exist");
        assert_eq!(restored.mode, "deep-interview");
    }

    #[test]
    fn read_normalizes_legacy_aliased_mode_values_from_persisted_state() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();
        let path = store.mode_path("deep-interview", None);
        fs::create_dir_all(path.parent().expect("state parent")).expect("state parent should exist");
        fs::write(
            &path,
            r#"{
  "mode": "deep_interview",
  "active": true,
  "updated_at": "2026-04-17T00:00:00Z",
  "started_at": "2026-04-17T00:00:00Z",
  "completed_at": null,
  "context": {}
}"#,
        )
        .expect("legacy aliased state should write");

        let restored = store
            .read("deep-interview", None)
            .expect("read should succeed")
            .expect("state should exist");
        assert_eq!(restored.mode, "deep-interview");
    }

    #[test]
    fn reads_global_legacy_alias_named_mode_state_file() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();
        let path = workspace
            .root
            .join(".omx")
            .join("state")
            .join("deep_interview-state.json");
        fs::create_dir_all(path.parent().expect("state parent")).expect("state parent should exist");
        fs::write(
            &path,
            r#"{
  "mode": "deep_interview",
  "active": true,
  "updated_at": "2026-04-17T00:00:00Z",
  "started_at": "2026-04-17T00:00:00Z",
  "completed_at": null,
  "context": {}
}"#,
        )
        .expect("legacy alias-named state should write");

        let restored = store
            .read("deep-interview", None)
            .expect("read should succeed")
            .expect("state should exist");
        assert_eq!(restored.mode, "deep-interview");
        assert!(store.mode_path("deep-interview", None).ends_with("deep-interview-state.json"));
    }

    #[test]
    fn reads_session_scoped_legacy_alias_named_mode_state_file() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();
        let path = workspace
            .root
            .join(".omx")
            .join("state")
            .join("sessions")
            .join("session-legacy")
            .join("deep_interview-state.json");
        fs::create_dir_all(path.parent().expect("state parent")).expect("state parent should exist");
        fs::write(
            &path,
            r#"{
  "mode": "deep_interview",
  "active": true,
  "session_id": "session-legacy",
  "updated_at": "2026-04-17T00:00:00Z",
  "started_at": "2026-04-17T00:00:00Z",
  "completed_at": null,
  "context": {}
}"#,
        )
        .expect("legacy alias-named session state should write");

        let restored = store
            .read("deep-interview", Some("session-legacy"))
            .expect("read should succeed")
            .expect("state should exist");
        assert_eq!(restored.mode, "deep-interview");
        assert_eq!(restored.session_id.as_deref(), Some("session-legacy"));
    }

    #[test]
    fn clear_normalizes_global_alias_mode_names() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();
        let record = ModeStateRecord::new("deep-interview", true);
        store.write(&record).expect("state should write");

        assert!(store
            .clear("deep interview", None)
            .expect("alias clear should succeed"));
        assert!(store
            .read("deep-interview", None)
            .expect("read should succeed")
            .is_none());
    }

    #[test]
    fn clear_normalizes_session_alias_mode_names() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();
        let mut record = ModeStateRecord::new("deep-interview", true);
        record.session_id = Some("session-alias".to_string());
        store.write(&record).expect("state should write");

        assert!(store
            .clear("deep_interview", Some("session-alias"))
            .expect("alias clear should succeed"));
        assert!(store
            .read("deep-interview", Some("session-alias"))
            .expect("session read should succeed")
            .is_none());
        assert!(store
            .read("deep-interview", None)
            .expect("global read should succeed")
            .is_none());
    }

    #[test]
    fn clear_removes_legacy_alias_named_mode_state_files() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();
        let global_alias_path = workspace
            .root
            .join(".omx")
            .join("state")
            .join("deep_interview-state.json");
        fs::create_dir_all(global_alias_path.parent().expect("state parent"))
            .expect("state parent should exist");
        fs::write(
            &global_alias_path,
            r#"{
  "mode": "deep_interview",
  "active": true,
  "session_id": "session-legacy-clear",
  "updated_at": "2026-04-17T00:00:00Z",
  "started_at": "2026-04-17T00:00:00Z",
  "completed_at": null,
  "context": {}
}"#,
        )
        .expect("legacy alias-named global state should write");

        let session_alias_path = workspace
            .root
            .join(".omx")
            .join("state")
            .join("sessions")
            .join("session-legacy-clear")
            .join("deep_interview-state.json");
        fs::create_dir_all(session_alias_path.parent().expect("state parent"))
            .expect("state parent should exist");
        fs::write(
            &session_alias_path,
            r#"{
  "mode": "deep_interview",
  "active": true,
  "session_id": "session-legacy-clear",
  "updated_at": "2026-04-17T00:00:00Z",
  "started_at": "2026-04-17T00:00:00Z",
  "completed_at": null,
  "context": {}
}"#,
        )
        .expect("legacy alias-named session state should write");

        assert!(store
            .clear("deep-interview", Some("session-legacy-clear"))
            .expect("alias clear should succeed"));
        assert!(!global_alias_path.exists());
        assert!(!session_alias_path.exists());
        assert!(store
            .read("deep-interview", Some("session-legacy-clear"))
            .expect("session read should succeed")
            .is_none());
        assert!(store
            .read("deep-interview", None)
            .expect("global read should succeed")
            .is_none());
    }

    fn looks_like_rfc3339(value: &str) -> bool {
        value.len() == 20
            && value.as_bytes()[4] == b'-'
            && value.as_bytes()[7] == b'-'
            && value.as_bytes()[10] == b'T'
            && value.as_bytes()[13] == b':'
            && value.as_bytes()[16] == b':'
            && value.as_bytes()[19] == b'Z'
    }
}
