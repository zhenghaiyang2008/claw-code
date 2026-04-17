use std::fmt::{Display, Formatter};
use std::fs;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::omc_compat::{build_omc_handoff, normalize_mode_name, OmcCompatHandoff};

static WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);
const DEFAULT_VERIFICATION_RUN_ID_PREFIX: &str = "verification-run";
const FILE_LOCK_RETRY_ATTEMPTS: u32 = 100;
const FILE_LOCK_RETRY_DELAY_MS: u64 = 10;
const FILE_LOCK_STALE_SECS: u64 = 30;

#[derive(Debug)]
pub enum VerificationRuntimeError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Format(String),
}

impl Display for VerificationRuntimeError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
            Self::Format(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for VerificationRuntimeError {}

impl From<std::io::Error> for VerificationRuntimeError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for VerificationRuntimeError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Pending,
    InProgress,
    NeedsReview,
    Passed,
    Failed,
    ChangesRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationCheckStatus {
    Pending,
    Passed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationReviewerOutcome {
    Approved,
    ChangesRequested,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationCheck {
    pub check_id: String,
    pub description: String,
    pub status: VerificationCheckStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationReviewer {
    pub reviewer: String,
    pub outcome: VerificationReviewerOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationRecord {
    pub verification_run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<OmcCompatHandoff>,
    pub status: VerificationStatus,
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub checks: Vec<VerificationCheck>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer: Option<VerificationReviewer>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationInitRequest {
    pub verification_run_id: Option<String>,
    pub session_id: Option<String>,
    pub mode: String,
    pub acceptance_criteria: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationCheckUpdateRequest {
    pub verification_run_id: String,
    pub check_id: String,
    pub description: String,
    pub status: VerificationCheckStatus,
    pub evidence: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationReviewerOutcomeRequest {
    pub verification_run_id: String,
    pub reviewer: String,
    pub outcome: VerificationReviewerOutcome,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerificationRuntimeStore {
    workspace_root: PathBuf,
}

struct FilesystemVerificationLock {
    path: PathBuf,
    token: String,
}

impl Drop for FilesystemVerificationLock {
    fn drop(&mut self) {
        if lock_owner_matches(&self.path, &self.token) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

impl VerificationRuntimeStore {
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
    pub fn verification_root(&self) -> PathBuf {
        self.workspace_root.join(".omx").join("runtime").join("verification")
    }

    #[must_use]
    pub fn record_path(&self, verification_run_id: &str) -> PathBuf {
        self.verification_root()
            .join(format!("{verification_run_id}.json"))
    }

    fn lock_path(&self, verification_run_id: &str) -> PathBuf {
        self.verification_root()
            .join(format!("{verification_run_id}.lock"))
    }

    fn acquire_record_lock(
        &self,
        verification_run_id: &str,
    ) -> Result<FilesystemVerificationLock, VerificationRuntimeError> {
        let path = self.lock_path(verification_run_id);
        let token = new_lock_token();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        for attempt in 0..FILE_LOCK_RETRY_ATTEMPTS {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut file) => {
                    if let Err(error) = file.write_all(token.as_bytes()) {
                        let _ = fs::remove_file(&path);
                        return Err(error.into());
                    }
                    if let Err(error) = file.sync_all() {
                        let _ = fs::remove_file(&path);
                        return Err(error.into());
                    }
                    return Ok(FilesystemVerificationLock {
                        path,
                        token: token.clone(),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if reclaim_stale_lock(&path)? {
                        continue;
                    }

                    if attempt + 1 == FILE_LOCK_RETRY_ATTEMPTS {
                        return Err(VerificationRuntimeError::Format(format!(
                            "verification record `{verification_run_id}` is locked"
                        )));
                    }
                    thread::sleep(std::time::Duration::from_millis(FILE_LOCK_RETRY_DELAY_MS));
                }
                Err(error) => return Err(error.into()),
            }
        }

        Err(VerificationRuntimeError::Format(format!(
            "verification record `{verification_run_id}` is locked"
        )))
    }
}

pub fn initialize_verification_record(
    store: &VerificationRuntimeStore,
    request: VerificationInitRequest,
) -> Result<VerificationRecord, VerificationRuntimeError> {
    let verification_run_id = match request.verification_run_id.as_deref() {
        Some(value) => normalize_existing_identifier("verification_run_id", value)?,
        None => format!(
            "{}-{}",
            DEFAULT_VERIFICATION_RUN_ID_PREFIX,
            unique_timestamp_nanos()
        ),
    };
    let session_id = request
        .session_id
        .as_deref()
        .map(|value| normalize_existing_identifier("session_id", value))
        .transpose()?;
    let mode = normalize_mode_name(&normalize_required_text("mode", &request.mode)?).to_string();
    let acceptance_criteria = normalize_acceptance_criteria(request.acceptance_criteria)?;
    let _lock = store.acquire_record_lock(&verification_run_id)?;
    if read_verification_record_unlocked(store, &verification_run_id)?.is_some() {
        return Err(VerificationRuntimeError::Format(format!(
            "verification record `{verification_run_id}` already exists"
        )));
    }
    let record = VerificationRecord {
        verification_run_id: verification_run_id.clone(),
        session_id,
        mode,
        handoff: Some(build_verification_handoff(&verification_run_id)),
        status: VerificationStatus::Pending,
        acceptance_criteria,
        checks: Vec::new(),
        reviewer: None,
        updated_at: iso8601_now(),
    };
    write_verification_record_unlocked(store, &record)?;
    Ok(record)
}

pub fn append_or_update_verification_check(
    store: &VerificationRuntimeStore,
    request: VerificationCheckUpdateRequest,
) -> Result<VerificationRecord, VerificationRuntimeError> {
    let verification_run_id =
        normalize_existing_identifier("verification_run_id", &request.verification_run_id)?;
    let check_id = normalize_existing_identifier("check_id", &request.check_id)?;
    let description = normalize_required_text("description", &request.description)?;
    let evidence = normalize_optional_text(request.evidence);

    mutate_verification_record(store, &verification_run_id, move |record| {
        let now = iso8601_now();
        if let Some(check) = record.checks.iter_mut().find(|check| check.check_id == check_id) {
            check.description = description.clone();
            check.status = request.status;
            check.evidence = evidence.clone();
            check.updated_at = now.clone();
        } else {
            record.checks.push(VerificationCheck {
                check_id,
                description,
                status: request.status,
                evidence,
                updated_at: now.clone(),
            });
        }

        record.status = recompute_status(record);
        record.updated_at = now;
        Ok(())
    })
}

pub fn mark_verification_reviewer_outcome(
    store: &VerificationRuntimeStore,
    request: VerificationReviewerOutcomeRequest,
) -> Result<VerificationRecord, VerificationRuntimeError> {
    let verification_run_id =
        normalize_existing_identifier("verification_run_id", &request.verification_run_id)?;
    let reviewer = normalize_required_text("reviewer", &request.reviewer)?;
    let notes = normalize_optional_text(request.notes);

    mutate_verification_record(store, &verification_run_id, move |record| {
        let now = iso8601_now();
        record.reviewer = Some(VerificationReviewer {
            reviewer,
            outcome: request.outcome,
            notes,
            updated_at: now.clone(),
        });
        record.status = recompute_status(record);
        record.updated_at = now;
        Ok(())
    })
}

pub fn read_verification_record(
    store: &VerificationRuntimeStore,
    verification_run_id: &str,
) -> Result<Option<VerificationRecord>, VerificationRuntimeError> {
    let verification_run_id =
        normalize_existing_identifier("verification_run_id", verification_run_id)?;
    read_verification_record_unlocked(store, &verification_run_id)
}

fn read_verification_record_unlocked(
    store: &VerificationRuntimeStore,
    verification_run_id: &str,
) -> Result<Option<VerificationRecord>, VerificationRuntimeError> {
    let path = store.record_path(&verification_run_id);
    if !path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(path)?;
    let mut record: VerificationRecord = serde_json::from_str(&contents)?;
    record.mode = normalize_mode_name(&record.mode).to_string();
    if record.handoff.is_none() {
        record.handoff = Some(build_verification_handoff(&record.verification_run_id));
    }
    Ok(Some(record))
}

fn write_verification_record_unlocked(
    store: &VerificationRuntimeStore,
    record: &VerificationRecord,
) -> Result<PathBuf, VerificationRuntimeError> {
    let path = store.record_path(&record.verification_run_id);
    let rendered = serde_json::to_string_pretty(record)?;
    write_atomic(&path, &rendered)?;
    Ok(path)
}

fn mutate_verification_record<F>(
    store: &VerificationRuntimeStore,
    verification_run_id: &str,
    mutate: F,
) -> Result<VerificationRecord, VerificationRuntimeError>
where
    F: FnOnce(&mut VerificationRecord) -> Result<(), VerificationRuntimeError>,
{
    let _lock = store.acquire_record_lock(verification_run_id)?;
    let mut record = read_verification_record_unlocked(store, verification_run_id)?.ok_or_else(|| {
        VerificationRuntimeError::Format(format!(
            "verification record `{verification_run_id}` does not exist"
        ))
    })?;
    mutate(&mut record)?;
    record.mode = normalize_mode_name(&record.mode).to_string();
    if record.handoff.is_none() {
        record.handoff = Some(build_verification_handoff(&record.verification_run_id));
    }
    write_verification_record_unlocked(store, &record)?;
    Ok(record)
}

fn build_verification_handoff(verification_run_id: &str) -> OmcCompatHandoff {
    build_omc_handoff(
        None,
        &[],
        &format!(".omx/runtime/verification/{verification_run_id}.json"),
    )
}

fn recompute_status(record: &VerificationRecord) -> VerificationStatus {
    if record
        .reviewer
        .as_ref()
        .is_some_and(|reviewer| reviewer.outcome == VerificationReviewerOutcome::ChangesRequested)
    {
        return VerificationStatus::ChangesRequested;
    }

    if record
        .checks
        .iter()
        .any(|check| check.status == VerificationCheckStatus::Failed)
    {
        return VerificationStatus::Failed;
    }

    if record.checks.is_empty() {
        return VerificationStatus::Pending;
    }

    if record
        .checks
        .iter()
        .all(|check| check.status == VerificationCheckStatus::Passed)
    {
        if record
            .reviewer
            .as_ref()
            .is_some_and(|reviewer| reviewer.outcome == VerificationReviewerOutcome::Approved)
        {
            VerificationStatus::Passed
        } else {
            VerificationStatus::NeedsReview
        }
    } else {
        VerificationStatus::InProgress
    }
}

fn normalize_required_text(
    field: &str,
    value: &str,
) -> Result<String, VerificationRuntimeError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(VerificationRuntimeError::Format(format!(
            "{field} must not be empty"
        )));
    }
    Ok(trimmed.to_string())
}

fn normalize_optional_text(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn normalize_acceptance_criteria(
    acceptance_criteria: Vec<String>,
) -> Result<Vec<String>, VerificationRuntimeError> {
    let normalized = acceptance_criteria
        .into_iter()
        .filter_map(|criterion| {
            let trimmed = criterion.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
        .collect::<Vec<_>>();
    if normalized.is_empty() {
        return Err(VerificationRuntimeError::Format(
            "acceptance_criteria must contain at least one non-empty item".to_string(),
        ));
    }
    Ok(normalized)
}

fn normalize_existing_identifier(
    field: &str,
    value: &str,
) -> Result<String, VerificationRuntimeError> {
    let slug = slugify(value);
    if slug.is_empty() {
        Err(VerificationRuntimeError::Format(format!(
            "{field} must not be empty"
        )))
    } else {
        Ok(slug)
    }
}

fn slugify(value: &str) -> String {
    let mut slug = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    slug.trim_matches('-').chars().take(64).collect()
}

fn unique_timestamp_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
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
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
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

fn write_atomic(path: &Path, contents: &str) -> Result<(), VerificationRuntimeError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = temporary_path_for(path);
    fs::write(&temp_path, contents)?;
    replace_file(&temp_path, path)?;
    Ok(())
}

fn lock_is_stale(path: &Path) -> bool {
    fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|elapsed| elapsed.as_secs() > FILE_LOCK_STALE_SECS)
}

fn reclaim_stale_lock(path: &Path) -> Result<bool, VerificationRuntimeError> {
    let Some(stale_owner_token) = read_lock_owner_token(path)? else {
        return Ok(false);
    };

    if !lock_is_stale(path) {
        return Ok(false);
    }

    reclaim_lock_for_owner(path, &stale_owner_token)
}

fn reclaim_lock_for_owner(
    path: &Path,
    owner_token: &str,
) -> Result<bool, VerificationRuntimeError> {
    if !lock_owner_matches(path, owner_token) {
        return Ok(false);
    }

    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

fn lock_owner_matches(path: &Path, token: &str) -> bool {
    read_lock_owner_token(path)
        .ok()
        .flatten()
        .is_some_and(|contents| contents == token)
}

fn read_lock_owner_token(path: &Path) -> Result<Option<String>, VerificationRuntimeError> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn new_lock_token() -> String {
    format!(
        "verification-lock-pid-{}-nanos-{}-seq-{}",
        std::process::id(),
        unique_timestamp_nanos(),
        WRITE_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn temporary_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("verification");
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

#[cfg(test)]
mod tests {
    use super::{
        append_or_update_verification_check, initialize_verification_record,
        mark_verification_reviewer_outcome, new_lock_token, read_verification_record,
        reclaim_lock_for_owner,
        VerificationCheckStatus, VerificationCheckUpdateRequest, VerificationInitRequest,
        VerificationReviewerOutcome, VerificationReviewerOutcomeRequest, VerificationRuntimeStore,
        VerificationStatus,
    };
    use std::fs;
    use std::path::PathBuf;
    use std::thread;
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
            let root = std::env::temp_dir().join(format!("runtime-verification-{nanos}"));
            fs::create_dir_all(&root).expect("workspace root should exist");
            Self { root }
        }

        fn store(&self) -> VerificationRuntimeStore {
            VerificationRuntimeStore::for_workspace(&self.root)
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn persists_verification_record_round_trip() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();

        let initialized = initialize_verification_record(
            &store,
            VerificationInitRequest {
                verification_run_id: Some("Milestone 3 Runtime".to_string()),
                session_id: Some("session-42".to_string()),
                mode: "verification".to_string(),
                acceptance_criteria: vec![
                    "Persist verification state".to_string(),
                    "Allow reviewer persistence".to_string(),
                ],
            },
        )
        .expect("record should initialize");

        assert_eq!(initialized.verification_run_id, "milestone-3-runtime");
        assert_eq!(initialized.status, VerificationStatus::Pending);
        assert_eq!(
            initialized
                .handoff
                .as_ref()
                .map(|handoff| handoff.handoff_path.as_str()),
            Some(".omx/runtime/verification/milestone-3-runtime.json")
        );

        let restored = read_verification_record(&store, "milestone-3-runtime")
            .expect("record should read")
            .expect("record should exist");
        assert_eq!(restored, initialized);
        assert!(workspace
            .root
            .join(".omx/runtime/verification/milestone-3-runtime.json")
            .exists());
    }

    #[test]
    fn read_backfills_handoff_for_legacy_verification_record() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();
        let path = store.record_path("legacy-run");
        fs::create_dir_all(
            path.parent()
                .expect("verification record path should have parent"),
        )
        .expect("verification directory should exist");
        fs::write(
            &path,
            serde_json::to_string_pretty(&serde_json::json!({
                "verification_run_id": "legacy-run",
                "session_id": "legacy-session",
                "mode": "verification",
                "status": "pending",
                "acceptance_criteria": ["Backfill legacy handoff"],
                "checks": [],
                "updated_at": "2026-04-17T00:00:00Z"
            }))
            .expect("legacy record should serialize"),
        )
        .expect("legacy record should write");

        let raw_before = fs::read_to_string(&path).expect("legacy record should exist");
        assert!(!raw_before.contains("\"handoff\""));

        let restored = read_verification_record(&store, "legacy-run")
            .expect("record should read")
            .expect("record should exist");
        assert_eq!(
            restored.handoff.as_ref().map(|handoff| handoff.handoff_path.as_str()),
            Some(".omx/runtime/verification/legacy-run.json")
        );

        let raw_after = fs::read_to_string(&path).expect("legacy record should still exist");
        assert!(!raw_after.contains("\"handoff\""));
    }

    #[test]
    fn read_normalizes_legacy_aliased_mode_values() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();
        let path = store.record_path("legacy-mode-run");
        fs::create_dir_all(
            path.parent()
                .expect("verification record path should have parent"),
        )
        .expect("verification directory should exist");
        fs::write(
            &path,
            serde_json::to_string_pretty(&serde_json::json!({
                "verification_run_id": "legacy-mode-run",
                "session_id": "legacy-session",
                "mode": "verifier",
                "status": "pending",
                "acceptance_criteria": ["Normalize legacy verification mode aliases"],
                "checks": [],
                "updated_at": "2026-04-17T00:00:00Z"
            }))
            .expect("legacy record should serialize"),
        )
        .expect("legacy record should write");

        let restored = read_verification_record(&store, "legacy-mode-run")
            .expect("record should read")
            .expect("record should exist");
        assert_eq!(restored.mode, "verification");
    }

    #[test]
    fn appending_and_updating_checks_persists_latest_state() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();

        initialize_verification_record(
            &store,
            VerificationInitRequest {
                verification_run_id: Some("check-run".to_string()),
                session_id: None,
                mode: "verification".to_string(),
                acceptance_criteria: vec!["Run targeted runtime tests".to_string()],
            },
        )
        .expect("record should initialize");

        let appended = append_or_update_verification_check(
            &store,
            VerificationCheckUpdateRequest {
                verification_run_id: "check-run".to_string(),
                check_id: "cargo-test".to_string(),
                description: "cargo test -p runtime verification_runtime".to_string(),
                status: VerificationCheckStatus::Passed,
                evidence: Some("3 tests passed".to_string()),
            },
        )
        .expect("check should append");
        assert_eq!(appended.checks.len(), 1);
        assert_eq!(appended.status, VerificationStatus::NeedsReview);
        assert_eq!(appended.checks[0].evidence.as_deref(), Some("3 tests passed"));

        let updated = append_or_update_verification_check(
            &store,
            VerificationCheckUpdateRequest {
                verification_run_id: "check-run".to_string(),
                check_id: "cargo-test".to_string(),
                description: "cargo test -p runtime verification_runtime".to_string(),
                status: VerificationCheckStatus::Failed,
                evidence: Some("1 test failed".to_string()),
            },
        )
        .expect("check should update");
        assert_eq!(updated.checks.len(), 1);
        assert_eq!(updated.status, VerificationStatus::Failed);
        assert_eq!(updated.checks[0].status, VerificationCheckStatus::Failed);
        assert_eq!(updated.checks[0].evidence.as_deref(), Some("1 test failed"));

        let restored = read_verification_record(&store, "check-run")
            .expect("record should read")
            .expect("record should exist");
        assert_eq!(restored.checks.len(), 1);
        assert_eq!(restored.checks[0].status, VerificationCheckStatus::Failed);
    }

    #[test]
    fn reviewer_outcome_is_persisted_and_updates_status() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();

        initialize_verification_record(
            &store,
            VerificationInitRequest {
                verification_run_id: Some("review-run".to_string()),
                session_id: Some("review-session".to_string()),
                mode: "verification".to_string(),
                acceptance_criteria: vec![
                    "Persist checks".to_string(),
                    "Persist reviewer outcome".to_string(),
                ],
            },
        )
        .expect("record should initialize");

        let record = read_verification_record(&store, "review-run")
            .expect("record should read")
            .expect("record should exist");
        assert_eq!(
            record.handoff.as_ref().map(|handoff| handoff.handoff_path.as_str()),
            Some(".omx/runtime/verification/review-run.json")
        );

        append_or_update_verification_check(
            &store,
            VerificationCheckUpdateRequest {
                verification_run_id: "review-run".to_string(),
                check_id: "runtime-tests".to_string(),
                description: "cargo test -p runtime verification_runtime".to_string(),
                status: VerificationCheckStatus::Passed,
                evidence: Some("all verification runtime tests passed".to_string()),
            },
        )
        .expect("check should append");

        let reviewed = mark_verification_reviewer_outcome(
            &store,
            VerificationReviewerOutcomeRequest {
                verification_run_id: "review-run".to_string(),
                reviewer: "Verifier Agent".to_string(),
                outcome: VerificationReviewerOutcome::Approved,
                notes: Some("Ready for the next runtime slice.".to_string()),
            },
        )
        .expect("review outcome should persist");

        assert_eq!(reviewed.status, VerificationStatus::Passed);
        assert_eq!(
            reviewed.reviewer.as_ref().map(|reviewer| reviewer.reviewer.as_str()),
            Some("Verifier Agent")
        );
        assert_eq!(
            reviewed.reviewer.as_ref().map(|reviewer| reviewer.notes.as_deref()),
            Some(Some("Ready for the next runtime slice."))
        );

        let restored = read_verification_record(&store, "review-run")
            .expect("record should read")
            .expect("record should exist");
        assert_eq!(restored.status, VerificationStatus::Passed);
        assert_eq!(
            restored.reviewer.map(|reviewer| reviewer.outcome),
            Some(VerificationReviewerOutcome::Approved)
        );
    }

    #[test]
    fn normalizes_mode_name_when_initializing_record() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();

        let initialized = initialize_verification_record(
            &store,
            VerificationInitRequest {
                verification_run_id: Some("normalized-mode".to_string()),
                session_id: None,
                mode: "Verifier".to_string(),
                acceptance_criteria: vec!["Normalize OMC compatibility mode names".to_string()],
            },
        )
        .expect("record should initialize");

        assert_eq!(initialized.mode, "verification");
    }

    #[test]
    fn duplicate_initialization_is_rejected_after_normalization() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();

        initialize_verification_record(
            &store,
            VerificationInitRequest {
                verification_run_id: Some("Review Run".to_string()),
                session_id: None,
                mode: "verification".to_string(),
                acceptance_criteria: vec!["Keep original record".to_string()],
            },
        )
        .expect("first record should initialize");

        let error = initialize_verification_record(
            &store,
            VerificationInitRequest {
                verification_run_id: Some("review-run".to_string()),
                session_id: Some("other-session".to_string()),
                mode: "verification".to_string(),
                acceptance_criteria: vec!["Attempt duplicate".to_string()],
            },
        )
        .expect_err("duplicate normalized id should be rejected");

        assert!(error
            .to_string()
            .contains("verification record `review-run` already exists"));
        let restored = read_verification_record(&store, "review-run")
            .expect("record should read")
            .expect("record should exist");
        assert_eq!(restored.session_id, None);
        assert_eq!(
            restored.acceptance_criteria,
            vec!["Keep original record".to_string()]
        );
    }

    #[test]
    fn update_waits_for_existing_lock_before_persisting() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();

        initialize_verification_record(
            &store,
            VerificationInitRequest {
                verification_run_id: Some("locked-run".to_string()),
                session_id: None,
                mode: "verification".to_string(),
                acceptance_criteria: vec!["Serialize updates".to_string()],
            },
        )
        .expect("record should initialize");

        let lock = store
            .acquire_record_lock("locked-run")
            .expect("test should acquire lock");
        let store_for_thread = store.clone();
        let handle = thread::spawn(move || {
            append_or_update_verification_check(
                &store_for_thread,
                VerificationCheckUpdateRequest {
                    verification_run_id: "locked-run".to_string(),
                    check_id: "runtime-check".to_string(),
                    description: "serialized update".to_string(),
                    status: VerificationCheckStatus::Passed,
                    evidence: Some("lock released".to_string()),
                },
            )
            .expect("update should succeed once lock is released")
        });

        thread::sleep(std::time::Duration::from_millis(50));
        assert!(
            !handle.is_finished(),
            "update should block while the record lock is held"
        );

        drop(lock);
        let updated = handle.join().expect("thread should join");
        assert_eq!(updated.checks.len(), 1);
        assert_eq!(updated.status, VerificationStatus::NeedsReview);

        let restored = read_verification_record(&store, "locked-run")
            .expect("record should read")
            .expect("record should exist");
        assert_eq!(restored.checks.len(), 1);
        assert_eq!(restored.checks[0].check_id, "runtime-check");
    }

    #[test]
    fn older_guard_cannot_remove_newer_reclaimed_lock() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();

        let original_lock = store
            .acquire_record_lock("takeover-run")
            .expect("original lock should be acquired");
        let lock_path = original_lock.path.clone();

        fs::remove_file(&lock_path).expect("stale lock should be reclaimed by the next owner");

        let replacement_lock = store
            .acquire_record_lock("takeover-run")
            .expect("replacement lock should be acquired");
        let replacement_token = replacement_lock.token.clone();

        drop(original_lock);

        assert!(
            lock_path.exists(),
            "dropping the older guard must not remove the replacement owner's lock"
        );
        assert_eq!(
            fs::read_to_string(&lock_path).expect("replacement token should remain"),
            replacement_token
        );

        drop(replacement_lock);
        assert!(
            !lock_path.exists(),
            "replacement owner should still clean up its own lock"
        );
    }

    #[test]
    fn reclaim_requires_same_owner_token_before_delete() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();
        let lock_path = store.lock_path("reclaim-check");

        fs::create_dir_all(lock_path.parent().expect("lock parent should exist"))
            .expect("lock parent should be creatable");
        fs::write(&lock_path, "stale-owner-token").expect("stale token should write");
        fs::write(&lock_path, "replacement-owner-token")
            .expect("replacement token should overwrite");

        let reclaimed = reclaim_lock_for_owner(&lock_path, "stale-owner-token")
            .expect("identity-safe reclaim should not error");
        assert!(!reclaimed, "reclaim should refuse to delete a different owner token");
        assert!(lock_path.exists(), "replacement lock should remain in place");
        assert_eq!(
            fs::read_to_string(&lock_path).expect("replacement token should remain"),
            "replacement-owner-token"
        );
    }

    #[test]
    fn lock_tokens_include_pid_and_unique_suffixes() {
        let first = new_lock_token();
        let second = new_lock_token();

        assert_ne!(first, second, "lock tokens should be unique per acquisition");
        assert!(
            first.contains(&format!("pid-{}", std::process::id())),
            "lock token should include the current process id"
        );
        assert!(first.contains("-nanos-"));
        assert!(first.contains("-seq-"));
    }
}
