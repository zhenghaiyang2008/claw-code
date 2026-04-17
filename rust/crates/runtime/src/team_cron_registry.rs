#![allow(clippy::must_use_candidate)]
//! Runtime registries for Team and Cron lifecycle management.
//!
//! Team state is persisted under `.omx/runtime/teams/` so claw-native team
//! metadata survives process restarts. Cron entries remain in-memory for now.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

static WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);
const FILE_LOCK_RETRY_ATTEMPTS: u32 = 100;
const FILE_LOCK_RETRY_DELAY_MS: u64 = 10;
const FILE_LOCK_STALE_SECS: u64 = 30;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn now_rfc3339() -> String {
    format_rfc3339(now_secs())
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Team {
    pub team_id: String,
    pub name: String,
    pub status: TeamStatus,
    pub phase: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub task_ids: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TeamStatus {
    Created,
    Running,
    Completed,
    Deleted,
}

impl std::fmt::Display for TeamStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Created => write!(f, "created"),
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Deleted => write!(f, "deleted"),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TeamRuntimeMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TeamRegistry {
    workspace_root: PathBuf,
    inner: Arc<Mutex<TeamInner>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct TeamIndex {
    counter: u64,
    team_ids: Vec<String>,
}

#[derive(Debug, Default)]
struct TeamInner;

struct FilesystemTeamLock {
    path: PathBuf,
    token: String,
}

impl Drop for FilesystemTeamLock {
    fn drop(&mut self) {
        if lock_owner_matches(&self.path, &self.token) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

impl Default for TeamRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl TeamRegistry {
    #[must_use]
    pub fn new() -> Self {
        let workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::for_workspace(workspace_root)
    }

    #[must_use]
    pub fn for_workspace(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            inner: Arc::new(Mutex::new(TeamInner)),
        }
    }

    pub fn create(&self, name: &str, task_ids: Vec<String>) -> Team {
        self.create_with_metadata(name, task_ids, TeamRuntimeMetadata::default())
            .expect("team creation should persist")
    }

    pub fn create_with_metadata(
        &self,
        name: &str,
        task_ids: Vec<String>,
        metadata: TeamRuntimeMetadata,
    ) -> Result<Team, String> {
        let _guard = self.inner.lock().expect("team registry lock poisoned");
        let _fs_lock = self.acquire_filesystem_lock()?;
        let mut index = self.load_index()?;
        index.counter += 1;
        let ts = now_secs();
        let timestamp = format_rfc3339(ts);
        let team_id = format!("team_{:08x}_{}", ts, index.counter);
        validate_unique_task_ids(&task_ids, &format!("team {team_id}"))?;
        let team = Team {
            team_id: team_id.clone(),
            name: normalize_required_text("name", name)?,
            status: TeamStatus::Created,
            phase: normalize_optional_text(metadata.phase)
                .unwrap_or_else(|| String::from("created")),
            session_id: normalize_optional_text(metadata.session_id),
            task_ids,
            created_at: timestamp.clone(),
            updated_at: timestamp,
        };
        self.save_team(&team)?;
        index.team_ids.push(team_id);
        self.save_index(&index)?;
        Ok(team)
    }

    pub fn get(&self, team_id: &str) -> Option<Team> {
        self.try_get(team_id).ok().flatten()
    }

    pub fn try_get(&self, team_id: &str) -> Result<Option<Team>, String> {
        self.load_team(team_id)
    }

    pub fn list(&self) -> Vec<Team> {
        self.try_list().unwrap_or_default()
    }

    pub fn try_list(&self) -> Result<Vec<Team>, String> {
        let index = self.read_index()?;
        index
            .team_ids
            .iter()
            .map(|team_id| {
                self.load_team(team_id)?
                    .ok_or_else(|| format!("team not found: {team_id}"))
            })
            .collect()
    }

    pub fn delete(&self, team_id: &str) -> Result<Team, String> {
        self.update_team(team_id, |team| {
            team.status = TeamStatus::Deleted;
            team.phase = String::from("deleted");
            Ok(())
        })
    }

    pub fn delete_with_task_ids(
        &self,
        team_id: &str,
        task_ids: Vec<String>,
    ) -> Result<Team, String> {
        self.update_team(team_id, move |team| {
            team.status = TeamStatus::Deleted;
            team.phase = String::from("deleted");
            team.task_ids = task_ids;
            Ok(())
        })
    }

    pub fn remove(&self, team_id: &str) -> Option<Team> {
        self.try_remove(team_id).ok().flatten()
    }

    pub fn try_remove(&self, team_id: &str) -> Result<Option<Team>, String> {
        let _guard = self.inner.lock().expect("team registry lock poisoned");
        let _fs_lock = self.acquire_filesystem_lock()?;
        let Some(team) = self.load_team(team_id)? else {
            return Ok(None);
        };
        let path = self.team_path(team_id)?;
        let mut index = self.load_index()?;
        index.team_ids.retain(|id| id != team_id);
        self.save_index(&index)?;
        fs::remove_file(path).map_err(|error| error.to_string())?;
        Ok(Some(team))
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.load_index().map(|index| index.team_ids.len()).unwrap_or(0)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn update_team<F>(&self, team_id: &str, update: F) -> Result<Team, String>
    where
        F: FnOnce(&mut Team) -> Result<(), String>,
    {
        let _guard = self.inner.lock().expect("team registry lock poisoned");
        let _fs_lock = self.acquire_filesystem_lock()?;
        let mut team = self
            .load_team(team_id)?
            .ok_or_else(|| format!("team not found: {team_id}"))?;
        update(&mut team)?;
        team.updated_at = now_rfc3339();
        self.save_team(&team)?;
        Ok(team)
    }

    fn runtime_root(&self) -> PathBuf {
        self.workspace_root.join(".omx").join("runtime")
    }

    fn teams_dir(&self) -> PathBuf {
        self.runtime_root().join("teams")
    }

    fn indexes_dir(&self) -> PathBuf {
        self.runtime_root().join("indexes")
    }

    fn team_path(&self, team_id: &str) -> Result<PathBuf, String> {
        validate_team_id(team_id)?;
        Ok(self.teams_dir().join(format!("{team_id}.json")))
    }

    fn team_index_path(&self) -> PathBuf {
        self.indexes_dir().join("teams.json")
    }

    fn team_lock_path(&self) -> PathBuf {
        self.indexes_dir().join("teams.lock")
    }

    fn load_index(&self) -> Result<TeamIndex, String> {
        self.load_index_with_repair(true)
    }

    fn read_index(&self) -> Result<TeamIndex, String> {
        self.load_index_with_repair(false)
    }

    fn load_index_with_repair(&self, repair: bool) -> Result<TeamIndex, String> {
        let path = self.team_index_path();
        let index = if path.exists() {
            let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
            serde_json::from_str(&contents).map_err(|error| error.to_string())?
        } else {
            TeamIndex::default()
        };

        let recovered = self.recover_index(index.clone())?;
        if repair && recovered != index {
            self.save_index(&recovered)?;
        }
        Ok(recovered)
    }

    fn save_index(&self, index: &TeamIndex) -> Result<(), String> {
        let rendered = serde_json::to_string_pretty(index).map_err(|error| error.to_string())?;
        write_atomic(&self.team_index_path(), &rendered).map_err(|error| error.to_string())
    }

    fn load_team(&self, team_id: &str) -> Result<Option<Team>, String> {
        let path = self.team_path(team_id)?;
        if !path.exists() {
            return Ok(None);
        }
        let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
        let team: Team = serde_json::from_str(&contents).map_err(|error| error.to_string())?;
        validate_team(&team)?;
        Ok(Some(team))
    }

    fn save_team(&self, team: &Team) -> Result<(), String> {
        validate_team(team)?;
        let rendered = serde_json::to_string_pretty(team).map_err(|error| error.to_string())?;
        let path = self.team_path(&team.team_id)?;
        write_atomic(&path, &rendered).map_err(|error| error.to_string())
    }

    fn acquire_filesystem_lock(&self) -> Result<FilesystemTeamLock, String> {
        let lock_path = self.team_lock_path();
        let token = new_lock_token();
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }

        for _ in 0..FILE_LOCK_RETRY_ATTEMPTS {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&lock_path)
            {
                Ok(mut file) => {
                    use std::io::Write as _;
                    if let Err(error) = file.write_all(token.as_bytes()) {
                        let _ = fs::remove_file(&lock_path);
                        return Err(error.to_string());
                    }
                    if let Err(error) = file.sync_all() {
                        let _ = fs::remove_file(&lock_path);
                        return Err(error.to_string());
                    }
                    return Ok(FilesystemTeamLock {
                        path: lock_path,
                        token: token.clone(),
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if reclaim_stale_lock(&lock_path)? {
                        continue;
                    }
                    thread::sleep(std::time::Duration::from_millis(FILE_LOCK_RETRY_DELAY_MS));
                }
                Err(error) => return Err(error.to_string()),
            }
        }

        Err(format!(
            "timed out waiting for team registry filesystem lock: {}",
            lock_path.display()
        ))
    }

    fn recover_index(&self, index: TeamIndex) -> Result<TeamIndex, String> {
        let mut teams = self.read_all_teams_from_disk()?;
        if teams.is_empty() {
            return Ok(TeamIndex {
                counter: index.counter,
                team_ids: Vec::new(),
            });
        }

        teams.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.team_id.cmp(&right.team_id))
        });

        let disk_ids: Vec<String> = teams.iter().map(|team| team.team_id.clone()).collect();
        let mut recovered_ids = Vec::with_capacity(disk_ids.len());

        for team_id in &index.team_ids {
            if disk_ids.iter().any(|disk_id| disk_id == team_id) && !recovered_ids.contains(team_id) {
                recovered_ids.push(team_id.clone());
            }
        }

        for team_id in disk_ids {
            if !recovered_ids.contains(&team_id) {
                recovered_ids.push(team_id);
            }
        }

        Ok(TeamIndex {
            counter: index
                .counter
                .max(recovered_ids.iter().map(|team_id| team_counter(team_id)).max().unwrap_or(0)),
            team_ids: recovered_ids,
        })
    }

    fn read_all_teams_from_disk(&self) -> Result<Vec<Team>, String> {
        let entries = match fs::read_dir(self.teams_dir()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.to_string()),
        };

        let mut teams = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|value| value.to_str()) != Some("json")
            {
                continue;
            }
            let contents = fs::read_to_string(&path).map_err(|error| error.to_string())?;
            let team: Team = serde_json::from_str(&contents).map_err(|error| error.to_string())?;
            teams.push(team);
        }
        Ok(teams)
    }
}

fn unique_timestamp_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn normalize_required_text(field: &str, value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(format!("{field} must not be empty"))
    } else {
        Ok(trimmed.to_string())
    }
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

fn validate_team(team: &Team) -> Result<(), String> {
    validate_team_id(&team.team_id)?;
    validate_unique_task_ids(&team.task_ids, &format!("persisted team {}", team.team_id))
}

fn validate_unique_task_ids(task_ids: &[String], context: &str) -> Result<(), String> {
    let mut seen = HashSet::with_capacity(task_ids.len());
    for task_id in task_ids {
        if !seen.insert(task_id.as_str()) {
            return Err(format!(
                "duplicate task_id {task_id} is not allowed in {context}"
            ));
        }
    }
    Ok(())
}

fn validate_team_id(team_id: &str) -> Result<(), String> {
    let Some(rest) = team_id.strip_prefix("team_") else {
        return Err(format!("invalid team id: {team_id}"));
    };
    let Some((hex_part, counter_part)) = rest.rsplit_once('_') else {
        return Err(format!("invalid team id: {team_id}"));
    };
    if hex_part.is_empty()
        || counter_part.is_empty()
        || !hex_part.chars().all(|ch| ch.is_ascii_hexdigit())
        || !counter_part.chars().all(|ch| ch.is_ascii_digit())
    {
        return Err(format!("invalid team id: {team_id}"));
    }
    Ok(())
}

fn team_counter(team_id: &str) -> u64 {
    team_id
        .rsplit_once('_')
        .and_then(|(_, suffix)| suffix.parse::<u64>().ok())
        .unwrap_or(0)
}

fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_path = temporary_path_for(path);
    fs::write(&temp_path, contents)?;
    replace_file(&temp_path, path)
}

fn temporary_path_for(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("team");
    path.with_file_name(format!(
        "{file_name}.tmp-{}-{}",
        now_secs(),
        WRITE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ))
}

fn lock_is_stale(path: &Path) -> bool {
    fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|elapsed| elapsed.as_secs() > FILE_LOCK_STALE_SECS)
}

fn reclaim_stale_lock(path: &Path) -> Result<bool, String> {
    let Some(stale_owner_token) = read_lock_owner_token(path)? else {
        return Ok(false);
    };

    if !lock_is_stale(path) {
        return Ok(false);
    }

    reclaim_lock_for_owner(path, &stale_owner_token)
}

fn reclaim_lock_for_owner(path: &Path, owner_token: &str) -> Result<bool, String> {
    if !lock_owner_matches(path, owner_token) {
        return Ok(false);
    }

    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn lock_owner_matches(path: &Path, token: &str) -> bool {
    read_lock_owner_token(path)
        .ok()
        .flatten()
        .is_some_and(|contents| contents == token)
}

fn read_lock_owner_token(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn new_lock_token() -> String {
    format!(
        "team-lock-pid-{}-nanos-{}-seq-{}",
        std::process::id(),
        unique_timestamp_nanos(),
        WRITE_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronEntry {
    pub cron_id: String,
    pub schedule: String,
    pub prompt: String,
    pub description: Option<String>,
    pub enabled: bool,
    pub created_at: u64,
    pub updated_at: u64,
    pub last_run_at: Option<u64>,
    pub run_count: u64,
}

#[derive(Debug, Clone, Default)]
pub struct CronRegistry {
    inner: Arc<Mutex<CronInner>>,
}

#[derive(Debug, Default)]
struct CronInner {
    entries: HashMap<String, CronEntry>,
    counter: u64,
}

impl CronRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create(&self, schedule: &str, prompt: &str, description: Option<&str>) -> CronEntry {
        let mut inner = self.inner.lock().expect("cron registry lock poisoned");
        inner.counter += 1;
        let ts = now_secs();
        let cron_id = format!("cron_{:08x}_{}", ts, inner.counter);
        let entry = CronEntry {
            cron_id: cron_id.clone(),
            schedule: schedule.to_owned(),
            prompt: prompt.to_owned(),
            description: description.map(str::to_owned),
            enabled: true,
            created_at: ts,
            updated_at: ts,
            last_run_at: None,
            run_count: 0,
        };
        inner.entries.insert(cron_id, entry.clone());
        entry
    }

    pub fn get(&self, cron_id: &str) -> Option<CronEntry> {
        let inner = self.inner.lock().expect("cron registry lock poisoned");
        inner.entries.get(cron_id).cloned()
    }

    pub fn list(&self, enabled_only: bool) -> Vec<CronEntry> {
        let inner = self.inner.lock().expect("cron registry lock poisoned");
        inner
            .entries
            .values()
            .filter(|e| !enabled_only || e.enabled)
            .cloned()
            .collect()
    }

    pub fn delete(&self, cron_id: &str) -> Result<CronEntry, String> {
        let mut inner = self.inner.lock().expect("cron registry lock poisoned");
        inner
            .entries
            .remove(cron_id)
            .ok_or_else(|| format!("cron not found: {cron_id}"))
    }

    /// Disable a cron entry without removing it.
    pub fn disable(&self, cron_id: &str) -> Result<(), String> {
        let mut inner = self.inner.lock().expect("cron registry lock poisoned");
        let entry = inner
            .entries
            .get_mut(cron_id)
            .ok_or_else(|| format!("cron not found: {cron_id}"))?;
        entry.enabled = false;
        entry.updated_at = now_secs();
        Ok(())
    }

    /// Record a cron run.
    pub fn record_run(&self, cron_id: &str) -> Result<(), String> {
        let mut inner = self.inner.lock().expect("cron registry lock poisoned");
        let entry = inner
            .entries
            .get_mut(cron_id)
            .ok_or_else(|| format!("cron not found: {cron_id}"))?;
        entry.last_run_at = Some(now_secs());
        entry.run_count += 1;
        entry.updated_at = now_secs();
        Ok(())
    }

    #[must_use]
    pub fn len(&self) -> usize {
        let inner = self.inner.lock().expect("cron registry lock poisoned");
        inner.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            let root = std::env::temp_dir().join(format!("runtime-team-registry-{nanos}"));
            fs::create_dir_all(&root).expect("workspace root should exist");
            Self { root }
        }

        fn registry(&self) -> TeamRegistry {
            TeamRegistry::for_workspace(&self.root)
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    // ── Team tests ──────────────────────────────────────

    #[test]
    fn persists_team_create_get_list_delete_round_trip() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        let team = registry.create("Alpha Squad", vec!["task_001".into(), "task_002".into()]);
        assert_eq!(team.name, "Alpha Squad");
        assert_eq!(team.task_ids.len(), 2);
        assert_eq!(team.status, TeamStatus::Created);
        assert_eq!(team.phase, "created");
        assert!(looks_like_rfc3339(&team.created_at));
        assert!(workspace
            .root
            .join(".omx/runtime/teams")
            .join(format!("{}.json", team.team_id))
            .exists());

        let restored_registry = workspace.registry();
        let fetched = restored_registry
            .get(&team.team_id)
            .expect("team should exist after reload");
        assert_eq!(fetched, team);

        let listed = restored_registry.list();
        assert_eq!(listed, vec![team.clone()]);

        let deleted = restored_registry
            .delete(&team.team_id)
            .expect("delete should succeed");
        assert_eq!(deleted.status, TeamStatus::Deleted);
        assert_eq!(deleted.phase, "deleted");

        let still_there = workspace
            .registry()
            .get(&team.team_id)
            .expect("deleted team should remain persisted");
        assert_eq!(still_there.status, TeamStatus::Deleted);
        assert_eq!(still_there.phase, "deleted");
    }

    #[test]
    fn persists_team_phase_and_session_metadata() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        let created = registry
            .create_with_metadata(
                "Dispatch Lane",
                vec!["task_010".into()],
                TeamRuntimeMetadata {
                    phase: Some("dispatch".to_string()),
                    session_id: Some("session-42".to_string()),
                },
            )
            .expect("team with metadata should persist");

        let restored = workspace
            .registry()
            .get(&created.team_id)
            .expect("team should exist");
        assert_eq!(restored.phase, "dispatch");
        assert_eq!(restored.session_id.as_deref(), Some("session-42"));
    }

    #[test]
    fn rejects_missing_team_operations() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        assert!(registry.delete("nonexistent").is_err());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn create_with_metadata_rejects_duplicate_task_ids() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();

        let error = registry
            .create_with_metadata(
                "Alpha Squad",
                vec!["task_dup".into(), "task_dup".into()],
                TeamRuntimeMetadata::default(),
            )
            .expect_err("duplicate task ids should be rejected");

        assert!(
            error.contains("duplicate task_id task_dup is not allowed in team team_"),
            "unexpected duplicate-task error: {error}"
        );
        assert!(registry.is_empty());
    }

    #[test]
    fn delete_with_task_ids_rejects_duplicate_task_ids() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        let created = registry.create("Alpha Squad", vec!["task_001".into(), "task_002".into()]);

        let error = registry
            .delete_with_task_ids(&created.team_id, vec!["task_dup".into(), "task_dup".into()])
            .expect_err("duplicate task ids should be rejected on delete writes");

        assert_eq!(
            error,
            format!(
                "duplicate task_id task_dup is not allowed in persisted team {}",
                created.team_id
            )
        );
        let restored = registry.get(&created.team_id).expect("team should remain persisted");
        assert_eq!(restored.status, TeamStatus::Created);
        assert_eq!(
            restored.task_ids,
            vec![String::from("task_001"), String::from("task_002")]
        );
    }

    #[test]
    fn save_team_rejects_duplicate_task_ids() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        let mut created = registry.create("Alpha Squad", vec!["task_001".into(), "task_002".into()]);
        created.task_ids = vec!["task_dup".into(), "task_dup".into()];

        let error = registry
            .save_team(&created)
            .expect_err("duplicate task ids should be rejected before persisting");

        assert_eq!(
            error,
            format!(
                "duplicate task_id task_dup is not allowed in persisted team {}",
                created.team_id
            )
        );
        let restored = registry
            .get(&created.team_id)
            .expect("original team should still exist");
        assert_eq!(
            restored.task_ids,
            vec![String::from("task_001"), String::from("task_002")]
        );
    }

    #[test]
    fn try_get_reports_corrupt_duplicate_task_ids() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        let team = registry.create("Alpha Squad", vec!["task_001".into(), "task_002".into()]);
        let path = workspace
            .root
            .join(".omx/runtime/teams")
            .join(format!("{}.json", team.team_id));
        fs::write(
            &path,
            format!(
                r#"{{
  "team_id": "{}",
  "name": "Alpha Squad",
  "status": "created",
  "phase": "created",
  "task_ids": ["task_dup", "task_dup"],
  "created_at": "2026-04-16T12:00:00Z",
  "updated_at": "2026-04-16T12:00:00Z"
}}"#,
                team.team_id
            ),
        )
        .expect("corrupt team file should write");

        let error = registry
            .try_get(&team.team_id)
            .expect_err("duplicate task ids should surface as corruption");
        assert_eq!(
            error,
            format!(
                "duplicate task_id task_dup is not allowed in persisted team {}",
                team.team_id
            )
        );
    }

    #[test]
    fn try_list_reports_corrupt_index_instead_of_looking_empty() {
        let workspace = TestWorkspace::new();
        fs::create_dir_all(workspace.root.join(".omx/runtime/indexes"))
            .expect("indexes dir should exist");
        fs::write(
            workspace.root.join(".omx/runtime/indexes/teams.json"),
            "{ not valid json",
        )
        .expect("corrupt index should write");

        let registry = workspace.registry();
        let error = registry
            .try_list()
            .expect_err("corrupt index should not look empty");
        assert!(
            error.contains("json")
                || error.contains("key")
                || error.contains("line")
                || error.contains("EOF")
        );
    }

    #[test]
    fn try_remove_reports_corrupt_index_instead_of_success() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        let team = registry.create("Alpha Squad", vec!["task_001".into()]);
        fs::write(
            workspace.root.join(".omx/runtime/indexes/teams.json"),
            "{ not valid json",
        )
        .expect("corrupt index should write");

        let error = registry
            .try_remove(&team.team_id)
            .expect_err("corrupt index should not look like cleanup success");
        assert!(
            error.contains("json")
                || error.contains("key")
                || error.contains("line")
                || error.contains("EOF")
        );
        assert!(
            workspace
                .root
                .join(".omx/runtime/teams")
                .join(format!("{}.json", team.team_id))
                .exists(),
            "team file should remain when cleanup fails before removal"
        );
    }

    #[test]
    fn try_list_recovers_in_memory_without_rewriting_index() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        let team = registry.create("Alpha Squad", vec!["task_001".into()]);
        let index_path = workspace.root.join(".omx/runtime/indexes/teams.json");
        fs::remove_file(&index_path).expect("test should remove persisted index");

        let listed = registry.try_list().expect("try_list should recover from disk");

        assert_eq!(listed, vec![team]);
        assert!(
            !index_path.exists(),
            "read-only list should not recreate the index file"
        );
    }

    #[test]
    fn older_guard_cannot_remove_newer_reclaimed_lock() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();

        let original_lock = registry
            .acquire_filesystem_lock()
            .expect("original lock should be acquired");
        let lock_path = original_lock.path.clone();

        fs::remove_file(&lock_path).expect("stale lock should be reclaimed by the next owner");

        let replacement_lock = registry
            .acquire_filesystem_lock()
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
        let registry = workspace.registry();
        let lock_path = registry.team_lock_path();

        fs::create_dir_all(lock_path.parent().expect("lock parent should exist"))
            .expect("lock parent should be creatable");
        fs::write(&lock_path, "stale-owner-token").expect("stale token should write");
        fs::write(&lock_path, "replacement-owner-token")
            .expect("replacement token should overwrite");

        let reclaimed = reclaim_lock_for_owner(&lock_path, "stale-owner-token")
            .expect("identity-safe reclaim should not error");
        assert!(
            !reclaimed,
            "reclaim should refuse to delete a different owner token"
        );
        assert!(lock_path.exists(), "replacement lock should remain in place");
        assert_eq!(
            fs::read_to_string(&lock_path).expect("replacement token should remain"),
            "replacement-owner-token"
        );
    }

    // ── Cron tests ──────────────────────────────────────

    #[test]
    fn creates_and_retrieves_cron() {
        let registry = CronRegistry::new();
        let entry = registry.create("0 * * * *", "Check status", Some("hourly check"));
        assert_eq!(entry.schedule, "0 * * * *");
        assert_eq!(entry.prompt, "Check status");
        assert!(entry.enabled);
        assert_eq!(entry.run_count, 0);
        assert!(entry.last_run_at.is_none());

        let fetched = registry.get(&entry.cron_id).expect("cron should exist");
        assert_eq!(fetched.cron_id, entry.cron_id);
    }

    #[test]
    fn lists_with_enabled_filter() {
        let registry = CronRegistry::new();
        let c1 = registry.create("* * * * *", "Task 1", None);
        let c2 = registry.create("0 * * * *", "Task 2", None);
        registry
            .disable(&c1.cron_id)
            .expect("disable should succeed");

        let all = registry.list(false);
        assert_eq!(all.len(), 2);

        let enabled_only = registry.list(true);
        assert_eq!(enabled_only.len(), 1);
        assert_eq!(enabled_only[0].cron_id, c2.cron_id);
    }

    #[test]
    fn deletes_cron_entry() {
        let registry = CronRegistry::new();
        let entry = registry.create("* * * * *", "To delete", None);
        let deleted = registry
            .delete(&entry.cron_id)
            .expect("delete should succeed");
        assert_eq!(deleted.cron_id, entry.cron_id);
        assert!(registry.get(&entry.cron_id).is_none());
        assert!(registry.is_empty());
    }

    #[test]
    fn records_cron_runs() {
        let registry = CronRegistry::new();
        let entry = registry.create("*/5 * * * *", "Recurring", None);
        registry.record_run(&entry.cron_id).unwrap();
        registry.record_run(&entry.cron_id).unwrap();

        let fetched = registry.get(&entry.cron_id).unwrap();
        assert_eq!(fetched.run_count, 2);
        assert!(fetched.last_run_at.is_some());
    }

    #[test]
    fn rejects_missing_cron_operations() {
        let registry = CronRegistry::new();
        assert!(registry.delete("nonexistent").is_err());
        assert!(registry.disable("nonexistent").is_err());
        assert!(registry.record_run("nonexistent").is_err());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn team_status_display_all_variants() {
        // given
        let cases = [
            (TeamStatus::Created, "created"),
            (TeamStatus::Running, "running"),
            (TeamStatus::Completed, "completed"),
            (TeamStatus::Deleted, "deleted"),
        ];

        // when
        let rendered: Vec<_> = cases
            .into_iter()
            .map(|(status, expected)| (status.to_string(), expected))
            .collect();

        // then
        assert_eq!(
            rendered,
            vec![
                ("created".to_string(), "created"),
                ("running".to_string(), "running"),
                ("completed".to_string(), "completed"),
                ("deleted".to_string(), "deleted"),
            ]
        );
    }

    #[test]
    fn new_team_registry_is_empty() {
        // given
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();

        // when
        let teams = registry.list();

        // then
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(teams.is_empty());
    }

    #[test]
    fn team_remove_nonexistent_returns_none() {
        // given
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();

        // when
        let removed = registry
            .try_remove("team_deadbeef_1")
            .expect("missing team should not error");

        // then
        assert!(removed.is_none());
    }

    #[test]
    fn team_len_transitions() {
        // given
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();

        // when
        let alpha = registry.create("Alpha", vec![]);
        let beta = registry.create("Beta", vec![]);
        let after_create = registry.len();
        registry.remove(&alpha.team_id);
        let after_first_remove = registry.len();
        registry.remove(&beta.team_id);

        // then
        assert_eq!(after_create, 2);
        assert_eq!(after_first_remove, 1);
        assert_eq!(registry.len(), 0);
        assert!(registry.is_empty());
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

    #[test]
    fn cron_list_all_disabled_returns_empty_for_enabled_only() {
        // given
        let registry = CronRegistry::new();
        let first = registry.create("* * * * *", "Task 1", None);
        let second = registry.create("0 * * * *", "Task 2", None);
        registry
            .disable(&first.cron_id)
            .expect("disable should succeed");
        registry
            .disable(&second.cron_id)
            .expect("disable should succeed");

        // when
        let enabled_only = registry.list(true);
        let all_entries = registry.list(false);

        // then
        assert!(enabled_only.is_empty());
        assert_eq!(all_entries.len(), 2);
    }

    #[test]
    fn cron_create_without_description() {
        // given
        let registry = CronRegistry::new();

        // when
        let entry = registry.create("*/15 * * * *", "Check health", None);

        // then
        assert!(entry.cron_id.starts_with("cron_"));
        assert_eq!(entry.description, None);
        assert!(entry.enabled);
        assert_eq!(entry.run_count, 0);
        assert_eq!(entry.last_run_at, None);
    }

    #[test]
    fn new_cron_registry_is_empty() {
        // given
        let registry = CronRegistry::new();

        // when
        let enabled_only = registry.list(true);
        let all_entries = registry.list(false);

        // then
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(enabled_only.is_empty());
        assert!(all_entries.is_empty());
    }

    #[test]
    fn cron_record_run_updates_timestamp_and_counter() {
        // given
        let registry = CronRegistry::new();
        let entry = registry.create("*/5 * * * *", "Recurring", None);

        // when
        registry
            .record_run(&entry.cron_id)
            .expect("first run should succeed");
        registry
            .record_run(&entry.cron_id)
            .expect("second run should succeed");
        let fetched = registry.get(&entry.cron_id).expect("entry should exist");

        // then
        assert_eq!(fetched.run_count, 2);
        assert!(fetched.last_run_at.is_some());
        assert!(fetched.updated_at >= entry.updated_at);
    }

    #[test]
    fn cron_disable_updates_timestamp() {
        // given
        let registry = CronRegistry::new();
        let entry = registry.create("0 0 * * *", "Nightly", None);

        // when
        registry
            .disable(&entry.cron_id)
            .expect("disable should succeed");
        let fetched = registry.get(&entry.cron_id).expect("entry should exist");

        // then
        assert!(!fetched.enabled);
        assert!(fetched.updated_at >= entry.updated_at);
    }
}
