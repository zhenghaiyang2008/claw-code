#![allow(clippy::must_use_candidate, clippy::unnecessary_map_or)]
//! Persistent task registry for sub-agent task lifecycle management.
//!
//! Task metadata is stored under `.omx/runtime/tasks/` with a lightweight index
//! under `.omx/runtime/indexes/tasks.json` so task-driven orchestration can
//! survive process restarts.

use std::fs;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde::de::Deserializer;

use crate::{validate_packet, TaskPacket, TaskPacketValidationError};

static WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);
const FILE_LOCK_RETRY_ATTEMPTS: u32 = 100;
const FILE_LOCK_RETRY_DELAY_MS: u64 = 10;
const FILE_LOCK_STALE_SECS: u64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Created,
    Running,
    Completed,
    Failed,
    Stopped,
}

impl std::fmt::Display for TaskStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Created => write!(f, "created"),
            Self::Running => write!(f, "running"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Stopped => write!(f, "stopped"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Task {
    pub task_id: String,
    pub prompt: String,
    pub description: Option<String>,
    pub task_packet: Option<TaskPacket>,
    pub status: TaskStatus,
    #[serde(deserialize_with = "deserialize_rfc3339_timestamp")]
    pub created_at: String,
    #[serde(deserialize_with = "deserialize_rfc3339_timestamp")]
    pub updated_at: String,
    pub messages: Vec<TaskMessage>,
    pub output: String,
    pub team_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskRuntimeMetadata {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifacts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskMessage {
    pub role: String,
    pub content: String,
    #[serde(deserialize_with = "deserialize_rfc3339_timestamp")]
    pub timestamp: String,
}

#[derive(Debug, Clone)]
pub struct TaskRegistry {
    workspace_root: PathBuf,
    inner: Arc<Mutex<RegistryInner>>,
}

#[derive(Debug, Default)]
struct RegistryInner;

struct FilesystemTaskLock {
    path: PathBuf,
}

impl Drop for FilesystemTaskLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct TaskIndex {
    counter: u64,
    task_ids: Vec<String>,
}

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

impl Default for TaskRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskRegistry {
    #[must_use]
    pub fn new() -> Self {
        let workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self::for_workspace(workspace_root)
    }

    #[must_use]
    pub fn for_workspace(workspace_root: impl Into<PathBuf>) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            inner: Arc::new(Mutex::new(RegistryInner)),
        }
    }

    #[must_use]
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn create(&self, prompt: &str, description: Option<&str>) -> Task {
        self.create_with_metadata(prompt, description, TaskRuntimeMetadata::default())
            .expect("task creation should persist")
    }

    pub fn create_with_metadata(
        &self,
        prompt: &str,
        description: Option<&str>,
        metadata: TaskRuntimeMetadata,
    ) -> Result<Task, String> {
        self.create_task(
            prompt.to_owned(),
            description.map(str::to_owned),
            None,
            metadata,
        )
    }

    pub fn create_from_packet(
        &self,
        packet: TaskPacket,
    ) -> Result<Task, TaskPacketValidationError> {
        self.create_from_packet_with_metadata(packet, TaskRuntimeMetadata::default())
    }

    pub fn try_create_from_packet(&self, packet: TaskPacket) -> Result<Task, String> {
        self.try_create_from_packet_with_metadata(packet, TaskRuntimeMetadata::default())
    }

    pub fn create_from_packet_with_metadata(
        &self,
        packet: TaskPacket,
        metadata: TaskRuntimeMetadata,
    ) -> Result<Task, TaskPacketValidationError> {
        let packet = validate_packet(packet)?.into_inner();
        Ok(self
            .create_task(
                packet.objective.clone(),
                Some(packet.scope.clone()),
                Some(packet),
                metadata,
            )
            .expect("packet-backed task should persist"))
    }

    pub fn try_create_from_packet_with_metadata(
        &self,
        packet: TaskPacket,
        metadata: TaskRuntimeMetadata,
    ) -> Result<Task, String> {
        let packet = validate_packet(packet).map_err(|error| error.to_string())?.into_inner();
        self.create_task(
            packet.objective.clone(),
            Some(packet.scope.clone()),
            Some(packet),
            metadata,
        )
    }

    fn create_task(
        &self,
        prompt: String,
        description: Option<String>,
        task_packet: Option<TaskPacket>,
        metadata: TaskRuntimeMetadata,
    ) -> Result<Task, String> {
        let _guard = self.inner.lock().expect("registry lock poisoned");
        let _fs_lock = self.acquire_filesystem_lock()?;
        let mut index = self.load_index()?;
        index.counter += 1;
        let ts = now_secs();
        let timestamp = format_rfc3339(ts);
        let task_id = format!("task_{:08x}_{}", ts, index.counter);
        let task = Task {
            task_id: task_id.clone(),
            prompt,
            description,
            task_packet,
            status: TaskStatus::Created,
            created_at: timestamp.clone(),
            updated_at: timestamp,
            messages: Vec::new(),
            output: String::new(),
            team_id: None,
            session_id: metadata.session_id,
            dependencies: metadata.dependencies,
            artifacts: metadata.artifacts,
        };
        self.save_task(&task)?;
        index.task_ids.push(task_id);
        self.save_index(&index)?;
        Ok(task)
    }

    pub fn get(&self, task_id: &str) -> Option<Task> {
        self.load_task(task_id).ok().flatten()
    }

    pub fn try_get(&self, task_id: &str) -> Result<Option<Task>, String> {
        self.load_task(task_id)
    }

    pub fn list(&self, status_filter: Option<TaskStatus>) -> Vec<Task> {
        let Ok(index) = self.load_index() else {
            return Vec::new();
        };
        index
            .task_ids
            .iter()
            .filter_map(|task_id| self.load_task(task_id).ok().flatten())
            .filter(|task| status_filter.map_or(true, |status| task.status == status))
            .collect()
    }

    pub fn try_list(&self, status_filter: Option<TaskStatus>) -> Result<Vec<Task>, String> {
        let index = self.load_index()?;
        Ok(index
            .task_ids
            .iter()
            .filter_map(|task_id| self.load_task(task_id).transpose())
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(|task| status_filter.map_or(true, |status| task.status == status))
            .collect())
    }

    pub fn stop(&self, task_id: &str) -> Result<Task, String> {
        self.update_task(task_id, |task| match task.status {
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Stopped => Err(format!(
                "task {task_id} is already in terminal state: {}",
                task.status
            )),
            _ => {
                task.status = TaskStatus::Stopped;
                Ok(())
            }
        })
    }

    pub fn update(&self, task_id: &str, message: &str) -> Result<Task, String> {
        let message = message.to_owned();
        self.update_task(task_id, |task| {
            task.messages.push(TaskMessage {
                role: String::from("user"),
                content: message.clone(),
                timestamp: now_rfc3339(),
            });
            Ok(())
        })
    }

    pub fn output(&self, task_id: &str) -> Result<String, String> {
        self.load_task(task_id)?
            .map(|task| task.output)
            .ok_or_else(|| format!("task not found: {task_id}"))
    }

    pub fn append_output(&self, task_id: &str, output: &str) -> Result<(), String> {
        let output = output.to_owned();
        self.update_task(task_id, |task| {
            task.output.push_str(&output);
            Ok(())
        })
        .map(|_| ())
    }

    pub fn set_status(&self, task_id: &str, status: TaskStatus) -> Result<(), String> {
        self.update_task(task_id, |task| {
            task.status = status;
            Ok(())
        })
        .map(|_| ())
    }

    pub fn assign_team(&self, task_id: &str, team_id: &str) -> Result<(), String> {
        let team_id = team_id.to_owned();
        self.update_task(task_id, |task| {
            if let Some(existing_team_id) = task.team_id.as_deref() {
                if existing_team_id != team_id {
                    return Err(format!(
                        "task {task_id} is already assigned to team {existing_team_id}"
                    ));
                }
                return Ok(());
            }
            task.team_id = Some(team_id.clone());
            Ok(())
        })
        .map(|_| ())
    }

    pub fn unassign_team(&self, task_id: &str) -> Result<(), String> {
        self.update_task(task_id, |task| {
            task.team_id = None;
            Ok(())
        })
        .map(|_| ())
    }

    pub fn update_metadata(
        &self,
        task_id: &str,
        metadata: TaskRuntimeMetadata,
    ) -> Result<Task, String> {
        self.update_task(task_id, |task| {
            task.session_id = metadata.session_id.clone();
            task.dependencies = metadata.dependencies.clone();
            task.artifacts = metadata.artifacts.clone();
            Ok(())
        })
    }

    pub fn remove(&self, task_id: &str) -> Option<Task> {
        let _guard = self.inner.lock().expect("registry lock poisoned");
        let _fs_lock = self.acquire_filesystem_lock().ok()?;
        let task = self.load_task(task_id).ok().flatten()?;
        let path = self.task_path(task_id).ok()?;
        fs::remove_file(path).ok()?;
        let mut index = self.load_index().ok()?;
        index.task_ids.retain(|id| id != task_id);
        self.save_index(&index).ok()?;
        Some(task)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.load_index().map(|index| index.task_ids.len()).unwrap_or(0)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn update_task<F>(&self, task_id: &str, update: F) -> Result<Task, String>
    where
        F: FnOnce(&mut Task) -> Result<(), String>,
    {
        let _guard = self.inner.lock().expect("registry lock poisoned");
        let _fs_lock = self.acquire_filesystem_lock()?;
        let mut task = self
            .load_task(task_id)?
            .ok_or_else(|| format!("task not found: {task_id}"))?;
        update(&mut task)?;
        task.updated_at = now_rfc3339();
        self.save_task(&task)?;
        Ok(task)
    }

    fn runtime_root(&self) -> PathBuf {
        self.workspace_root.join(".omx").join("runtime")
    }

    fn tasks_dir(&self) -> PathBuf {
        self.runtime_root().join("tasks")
    }

    fn indexes_dir(&self) -> PathBuf {
        self.runtime_root().join("indexes")
    }

    fn task_path(&self, task_id: &str) -> Result<PathBuf, String> {
        validate_task_id(task_id)?;
        Ok(self.tasks_dir().join(format!("{task_id}.json")))
    }

    fn task_index_path(&self) -> PathBuf {
        self.indexes_dir().join("tasks.json")
    }

    fn task_lock_path(&self) -> PathBuf {
        self.indexes_dir().join("tasks.lock")
    }

    fn load_index(&self) -> Result<TaskIndex, String> {
        let path = self.task_index_path();
        let index = if path.exists() {
            let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
            serde_json::from_str(&contents).map_err(|error| error.to_string())?
        } else {
            TaskIndex::default()
        };

        let recovered = self.recover_index(index.clone())?;
        if recovered != index {
            self.save_index(&recovered)?;
        }
        Ok(recovered)
    }

    fn save_index(&self, index: &TaskIndex) -> Result<(), String> {
        let rendered = serde_json::to_string_pretty(index).map_err(|error| error.to_string())?;
        write_atomic(&self.task_index_path(), &rendered).map_err(|error| error.to_string())
    }

    fn load_task(&self, task_id: &str) -> Result<Option<Task>, String> {
        let path = self.task_path(task_id)?;
        if !path.exists() {
            return Ok(None);
        }
        let contents = fs::read_to_string(path).map_err(|error| error.to_string())?;
        let task = serde_json::from_str(&contents).map_err(|error| error.to_string())?;
        Ok(Some(task))
    }

    fn save_task(&self, task: &Task) -> Result<(), String> {
        let rendered = serde_json::to_string_pretty(task).map_err(|error| error.to_string())?;
        let path = self.task_path(&task.task_id)?;
        write_atomic(&path, &rendered).map_err(|error| error.to_string())
    }

    fn acquire_filesystem_lock(&self) -> Result<FilesystemTaskLock, String> {
        let lock_path = self.task_lock_path();
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
                    let _ = writeln!(file, "pid={}", std::process::id());
                    return Ok(FilesystemTaskLock { path: lock_path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if lock_is_stale(&lock_path) {
                        let _ = fs::remove_file(&lock_path);
                    }
                    thread::sleep(std::time::Duration::from_millis(FILE_LOCK_RETRY_DELAY_MS));
                }
                Err(error) => return Err(error.to_string()),
            }
        }

        Err(format!(
            "timed out waiting for task registry filesystem lock: {}",
            lock_path.display()
        ))
    }

    fn recover_index(&self, index: TaskIndex) -> Result<TaskIndex, String> {
        let mut tasks = self.read_all_tasks_from_disk()?;
        if tasks.is_empty() {
            return Ok(TaskIndex {
                counter: index.counter,
                task_ids: Vec::new(),
            });
        }

        tasks.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.task_id.cmp(&right.task_id))
        });

        let disk_ids: Vec<String> = tasks.iter().map(|task| task.task_id.clone()).collect();
        let mut recovered_ids = Vec::with_capacity(disk_ids.len());

        for task_id in &index.task_ids {
            if disk_ids.iter().any(|disk_id| disk_id == task_id) && !recovered_ids.contains(task_id) {
                recovered_ids.push(task_id.clone());
            }
        }

        for task_id in disk_ids {
            if !recovered_ids.contains(&task_id) {
                recovered_ids.push(task_id);
            }
        }

        Ok(TaskIndex {
            counter: index
                .counter
                .max(recovered_ids.iter().map(|task_id| task_counter(task_id)).max().unwrap_or(0)),
            task_ids: recovered_ids,
        })
    }

    fn read_all_tasks_from_disk(&self) -> Result<Vec<Task>, String> {
        let entries = match fs::read_dir(self.tasks_dir()) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.to_string()),
        };

        let mut tasks = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|value| value.to_str()) != Some("json")
            {
                continue;
            }
            let contents = fs::read_to_string(&path).map_err(|error| error.to_string())?;
            let task: Task = serde_json::from_str(&contents).map_err(|error| error.to_string())?;
            tasks.push(task);
        }
        Ok(tasks)
    }
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
        .unwrap_or("task");
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

fn validate_task_id(task_id: &str) -> Result<(), String> {
    let Some(rest) = task_id.strip_prefix("task_") else {
        return Err(format!("invalid task id: {task_id}"));
    };
    let Some((hex_part, counter_part)) = rest.rsplit_once('_') else {
        return Err(format!("invalid task id: {task_id}"));
    };
    if hex_part.is_empty()
        || counter_part.is_empty()
        || !hex_part.chars().all(|ch| ch.is_ascii_hexdigit())
        || !counter_part.chars().all(|ch| ch.is_ascii_digit())
    {
        return Err(format!("invalid task id: {task_id}"));
    }
    Ok(())
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

fn task_counter(task_id: &str) -> u64 {
    task_id
        .rsplit_once('_')
        .and_then(|(_, suffix)| suffix.parse::<u64>().ok())
        .unwrap_or(0)
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

#[cfg(test)]
mod tests {
    use super::*;
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
            let root = std::env::temp_dir().join(format!("runtime-task-registry-{nanos}"));
            fs::create_dir_all(&root).expect("workspace root should exist");
            Self { root }
        }

        fn registry(&self) -> TaskRegistry {
            TaskRegistry::for_workspace(&self.root)
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn creates_and_retrieves_tasks() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        let task = registry.create("Do something", Some("A test task"));
        assert_eq!(task.status, TaskStatus::Created);
        assert_eq!(task.prompt, "Do something");
        assert_eq!(task.description.as_deref(), Some("A test task"));
        assert_eq!(task.task_packet, None);
        assert_eq!(task.session_id, None);
        assert!(task.dependencies.is_empty());
        assert!(task.artifacts.is_empty());
        assert!(looks_like_rfc3339(task.created_at.as_str()));
        assert!(looks_like_rfc3339(task.updated_at.as_str()));

        let fetched = registry.get(&task.task_id).expect("task should exist");
        assert_eq!(fetched.task_id, task.task_id);
    }

    #[test]
    fn creates_task_from_packet() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        let packet = TaskPacket {
            objective: "Ship task packet support".to_string(),
            scope: "runtime/task system".to_string(),
            repo: "claw-code-parity".to_string(),
            branch_policy: "origin/main only".to_string(),
            acceptance_tests: vec!["cargo test --workspace".to_string()],
            commit_policy: "single commit".to_string(),
            reporting_contract: "print commit sha".to_string(),
            escalation_policy: "manual escalation".to_string(),
        };

        let task = registry
            .create_from_packet(packet.clone())
            .expect("packet-backed task should be created");

        assert_eq!(task.prompt, packet.objective);
        assert_eq!(task.description.as_deref(), Some("runtime/task system"));
        assert_eq!(task.task_packet, Some(packet.clone()));

        let fetched = registry.get(&task.task_id).expect("task should exist");
        assert_eq!(fetched.task_packet, Some(packet));
        assert!(looks_like_rfc3339(fetched.created_at.as_str()));
    }

    #[test]
    fn creates_task_with_runtime_metadata() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        let task = registry
            .create_with_metadata(
                "Coordinate parallel review",
                Some("team runtime bootstrap"),
                TaskRuntimeMetadata {
                    session_id: Some("session-123".to_string()),
                    dependencies: vec!["task_root".to_string()],
                    artifacts: vec![".omx/specs/spec.md".to_string()],
                },
            )
            .expect("task with metadata should persist");

        assert_eq!(task.session_id.as_deref(), Some("session-123"));
        assert_eq!(task.dependencies, vec!["task_root".to_string()]);
        assert_eq!(task.artifacts, vec![".omx/specs/spec.md".to_string()]);

        let restored = registry.get(&task.task_id).expect("task should restore");
        assert_eq!(restored.session_id.as_deref(), Some("session-123"));
        assert_eq!(restored.dependencies, vec!["task_root".to_string()]);
        assert_eq!(restored.artifacts, vec![".omx/specs/spec.md".to_string()]);
    }

    #[test]
    fn persists_tasks_across_registry_instances() {
        let workspace = TestWorkspace::new();
        let first = workspace.registry();
        let task = first.create("Persist me", Some("durable task"));

        let second = workspace.registry();
        let restored = second.get(&task.task_id).expect("task should restore");
        assert_eq!(restored.prompt, "Persist me");
        assert_eq!(second.len(), 1);
    }

    #[test]
    fn recovers_index_when_tasks_index_file_is_missing() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        let task = registry.create("Persist me", Some("recoverable task"));
        fs::remove_file(
            workspace
                .root
                .join(".omx/runtime/indexes/tasks.json"),
        )
        .expect("index file should remove");

        let recovered = workspace.registry();
        let tasks = recovered.list(None);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].task_id, task.task_id);
        assert_eq!(recovered.len(), 1);
    }

    #[test]
    fn recovers_index_when_existing_index_is_stale() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        let task = registry.create("Persist me", Some("recoverable task"));
        fs::write(
            workspace.root.join(".omx/runtime/indexes/tasks.json"),
            "{\n  \"counter\": 0,\n  \"task_ids\": []\n}",
        )
        .expect("stale index should write");

        let recovered = workspace.registry();
        let tasks = recovered.list(None);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].task_id, task.task_id);

        let restored_index = fs::read_to_string(workspace.root.join(".omx/runtime/indexes/tasks.json"))
            .expect("index should be rewritten");
        assert!(restored_index.contains(&task.task_id));
    }

    #[test]
    fn lists_tasks_with_optional_filter() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        registry.create("Task A", None);
        let task_b = registry.create("Task B", None);
        registry
            .set_status(&task_b.task_id, TaskStatus::Running)
            .expect("set status should succeed");

        let all = registry.list(None);
        assert_eq!(all.len(), 2);

        let running = registry.list(Some(TaskStatus::Running));
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].task_id, task_b.task_id);

        let created = registry.list(Some(TaskStatus::Created));
        assert_eq!(created.len(), 1);
    }

    #[test]
    fn stops_running_task() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        let task = registry.create("Stoppable", None);
        registry
            .set_status(&task.task_id, TaskStatus::Running)
            .unwrap();

        let stopped = registry.stop(&task.task_id).expect("stop should succeed");
        assert_eq!(stopped.status, TaskStatus::Stopped);

        let result = registry.stop(&task.task_id);
        assert!(result.is_err());
    }

    #[test]
    fn updates_task_with_messages() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        let task = registry.create("Messageable", None);
        let updated = registry
            .update(&task.task_id, "Here's more context")
            .expect("update should succeed");
        assert_eq!(updated.messages.len(), 1);
        assert_eq!(updated.messages[0].content, "Here's more context");
        assert_eq!(updated.messages[0].role, "user");
        assert!(looks_like_rfc3339(updated.messages[0].timestamp.as_str()));
    }

    #[test]
    fn appends_and_retrieves_output() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        let task = registry.create("Output task", None);
        registry
            .append_output(&task.task_id, "line 1\n")
            .expect("append should succeed");
        registry
            .append_output(&task.task_id, "line 2\n")
            .expect("append should succeed");

        let output = registry.output(&task.task_id).expect("output should exist");
        assert_eq!(output, "line 1\nline 2\n");
    }

    #[test]
    fn assigns_team_and_removes_task() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        let task = registry.create("Team task", None);
        registry
            .assign_team(&task.task_id, "team_abc")
            .expect("assign should succeed");

        let fetched = registry.get(&task.task_id).unwrap();
        assert_eq!(fetched.team_id.as_deref(), Some("team_abc"));

        let removed = registry.remove(&task.task_id);
        assert!(removed.is_some());
        assert!(registry.get(&task.task_id).is_none());
        assert!(registry.is_empty());
    }

    #[test]
    fn rejects_reassigning_task_to_different_team() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        let task = registry.create("Team task", None);

        registry
            .assign_team(&task.task_id, "team_alpha")
            .expect("initial assignment should succeed");
        let error = registry
            .assign_team(&task.task_id, "team_beta")
            .expect_err("reassignment should be rejected");

        assert_eq!(
            error,
            format!("task {} is already assigned to team team_alpha", task.task_id)
        );
        assert_eq!(
            registry
                .get(&task.task_id)
                .expect("task should still exist")
                .team_id
                .as_deref(),
            Some("team_alpha")
        );
    }

    #[test]
    fn reassigning_task_to_same_team_is_idempotent() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        let task = registry.create("Team task", None);

        registry
            .assign_team(&task.task_id, "team_alpha")
            .expect("initial assignment should succeed");
        registry
            .assign_team(&task.task_id, "team_alpha")
            .expect("same-team reassignment should succeed");

        assert_eq!(
            registry
                .get(&task.task_id)
                .expect("task should still exist")
                .team_id
                .as_deref(),
            Some("team_alpha")
        );
    }

    #[test]
    fn updates_runtime_metadata() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        let task = registry.create("metadata", None);
        let updated = registry
            .update_metadata(
                &task.task_id,
                TaskRuntimeMetadata {
                    session_id: Some("session-789".to_string()),
                    dependencies: vec!["task_abc".to_string(), "task_xyz".to_string()],
                    artifacts: vec!["artifact.log".to_string()],
                },
            )
            .expect("metadata update should succeed");

        assert_eq!(updated.session_id.as_deref(), Some("session-789"));
        assert_eq!(
            updated.dependencies,
            vec!["task_abc".to_string(), "task_xyz".to_string()]
        );
        assert_eq!(updated.artifacts, vec!["artifact.log".to_string()]);
        assert!(looks_like_rfc3339(updated.updated_at.as_str()));
    }

    #[test]
    fn rejects_operations_on_missing_task() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        assert!(registry.stop("nonexistent").is_err());
        assert!(registry.update("nonexistent", "msg").is_err());
        assert!(registry.output("nonexistent").is_err());
        assert!(registry.append_output("nonexistent", "data").is_err());
        assert!(registry
            .set_status("nonexistent", TaskStatus::Running)
            .is_err());
    }

    #[test]
    fn task_status_display_all_variants() {
        let cases = [
            (TaskStatus::Created, "created"),
            (TaskStatus::Running, "running"),
            (TaskStatus::Completed, "completed"),
            (TaskStatus::Failed, "failed"),
            (TaskStatus::Stopped, "stopped"),
        ];

        let rendered: Vec<_> = cases
            .into_iter()
            .map(|(status, expected)| (status.to_string(), expected))
            .collect();

        assert_eq!(
            rendered,
            vec![
                ("created".to_string(), "created"),
                ("running".to_string(), "running"),
                ("completed".to_string(), "completed"),
                ("failed".to_string(), "failed"),
                ("stopped".to_string(), "stopped"),
            ]
        );
    }

    #[test]
    fn stop_rejects_completed_task() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        let task = registry.create("done", None);
        registry
            .set_status(&task.task_id, TaskStatus::Completed)
            .expect("set status should succeed");

        let result = registry.stop(&task.task_id);

        let error = result.expect_err("completed task should be rejected");
        assert!(error.contains("already in terminal state"));
        assert!(error.contains("completed"));
    }

    #[test]
    fn stop_rejects_failed_task() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        let task = registry.create("failed", None);
        registry
            .set_status(&task.task_id, TaskStatus::Failed)
            .expect("set status should succeed");

        let result = registry.stop(&task.task_id);

        let error = result.expect_err("failed task should be rejected");
        assert!(error.contains("already in terminal state"));
        assert!(error.contains("failed"));
    }

    #[test]
    fn stop_succeeds_from_created_state() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        let task = registry.create("created task", None);

        let stopped = registry.stop(&task.task_id).expect("stop should succeed");

        assert_eq!(stopped.status, TaskStatus::Stopped);
        assert!(stopped.updated_at >= task.updated_at);
        assert!(looks_like_rfc3339(stopped.updated_at.as_str()));
    }

    #[test]
    fn new_registry_is_empty() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();

        let all_tasks = registry.list(None);

        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
        assert!(all_tasks.is_empty());
    }

    #[test]
    fn create_without_description() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();

        let task = registry.create("Do the thing", None);

        assert!(task.task_id.starts_with("task_"));
        assert_eq!(task.description, None);
        assert_eq!(task.task_packet, None);
        assert!(task.messages.is_empty());
        assert!(task.output.is_empty());
        assert_eq!(task.team_id, None);
        assert_eq!(task.session_id, None);
    }

    #[test]
    fn remove_nonexistent_returns_none() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();

        let removed = registry.remove("missing");

        assert!(removed.is_none());
    }

    #[test]
    fn assign_team_rejects_missing_task() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();

        let result = registry.assign_team("missing", "team_123");

        let error = result.expect_err("missing task should be rejected");
        assert_eq!(error, "invalid task id: missing");
    }

    #[test]
    fn rejects_task_id_path_traversal() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();

        let error = registry
            .try_get("../escape")
            .expect_err("path traversal task id should be rejected");
        assert!(error.contains("invalid task id"));
    }

    #[test]
    fn serialized_task_file_uses_rfc3339_timestamp_strings() {
        let workspace = TestWorkspace::new();
        let registry = workspace.registry();
        let task = registry.create("persist timestamps", None);

        let serialized = fs::read_to_string(
            workspace
                .root
                .join(".omx/runtime/tasks")
                .join(format!("{}.json", task.task_id)),
        )
        .expect("task file should be readable");
        let json: serde_json::Value = serde_json::from_str(&serialized).expect("valid json");
        assert!(json["created_at"].is_string());
        assert!(json["updated_at"].is_string());
        assert!(looks_like_rfc3339(
            json["created_at"].as_str().expect("created_at string")
        ));
    }

    #[test]
    fn reads_legacy_numeric_task_timestamps_as_rfc3339_strings() {
        let workspace = TestWorkspace::new();
        let tasks_dir = workspace.root.join(".omx/runtime/tasks");
        fs::create_dir_all(&tasks_dir).expect("tasks dir should exist");
        fs::write(
            tasks_dir.join("task_63c3f6d0_1.json"),
            r#"{
  "task_id": "task_63c3f6d0_1",
  "prompt": "legacy task",
  "description": null,
  "task_packet": null,
  "status": "created",
  "created_at": 1673786096,
  "updated_at": 1673786096,
  "messages": [
    {"role": "user", "content": "hello", "timestamp": 1673786036}
  ],
  "output": "",
  "team_id": null
}"#,
        )
        .expect("legacy task file should write");

        let registry = workspace.registry();
        let task = registry
            .get("task_63c3f6d0_1")
            .expect("legacy task should deserialize");
        assert_eq!(task.created_at, "2023-01-15T12:34:56Z");
        assert_eq!(task.updated_at, "2023-01-15T12:34:56Z");
        assert_eq!(task.messages[0].timestamp, "2023-01-15T12:33:56Z");
    }

    #[test]
    fn try_list_reports_corrupt_index_instead_of_looking_empty() {
        let workspace = TestWorkspace::new();
        fs::create_dir_all(workspace.root.join(".omx/runtime/indexes"))
            .expect("indexes dir should exist");
        fs::write(
            workspace.root.join(".omx/runtime/indexes/tasks.json"),
            "{ not valid json",
        )
        .expect("corrupt index should write");

        let registry = workspace.registry();
        let error = registry
            .try_list(None)
            .expect_err("corrupt index should not look empty");
        assert!(
            error.contains("json")
                || error.contains("key")
                || error.contains("line")
                || error.contains("EOF")
        );
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
