use std::fs;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::file_ops::{write_file, WriteFileOutput};
use crate::mode_state::{ModeStateError, ModeStateRecord, ModeStateStore};

pub const DEEP_INTERVIEW_MODE: &str = "deep-interview";
const DEFAULT_INTERVIEW_ID_PREFIX: &str = "deep-interview";

#[derive(Debug)]
pub enum DeepInterviewError {
    Io(std::io::Error),
    Json(serde_json::Error),
    State(ModeStateError),
    Format(String),
}

impl Display for DeepInterviewError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Json(error) => write!(f, "{error}"),
            Self::State(error) => write!(f, "{error}"),
            Self::Format(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for DeepInterviewError {}

impl From<std::io::Error> for DeepInterviewError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for DeepInterviewError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<ModeStateError> for DeepInterviewError {
    fn from(value: ModeStateError) -> Self {
        Self::State(value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeepInterviewState {
    pub interview_id: String,
    pub initial_idea: String,
    #[serde(default)]
    pub rounds: Vec<DeepInterviewRound>,
    pub current_ambiguity: f64,
    pub threshold: f64,
    pub output_spec_path: String,
    pub handoff_path: String,
}

impl DeepInterviewState {
    #[must_use]
    pub fn phase(&self) -> &'static str {
        if self.current_ambiguity <= self.threshold {
            "handoff"
        } else {
            "interview"
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeepInterviewRound {
    pub question: String,
    pub answer: String,
    pub ambiguity: DeepInterviewAmbiguity,
    pub recorded_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeepInterviewAmbiguity {
    pub before: f64,
    pub after: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeepInterviewInitRequest {
    pub interview_id: Option<String>,
    pub initial_idea: String,
    pub current_ambiguity: f64,
    pub threshold: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeepInterviewAppendRequest {
    pub interview_id: String,
    pub question: String,
    pub answer: String,
    pub ambiguity_after: f64,
    pub ambiguity_before: Option<f64>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeepInterviewSessionArtifact {
    pub state: DeepInterviewState,
    pub spec_output: WriteFileOutput,
}

pub fn initialize_deep_interview_session(
    store: &ModeStateStore,
    request: DeepInterviewInitRequest,
) -> Result<DeepInterviewSessionArtifact, DeepInterviewError> {
    validate_non_negative("current_ambiguity", request.current_ambiguity)?;
    validate_non_negative("threshold", request.threshold)?;
    let initial_idea = normalize_required_text("initial_idea", &request.initial_idea)?;
    let interview_id = match request.interview_id.as_deref() {
        Some(value) => normalize_existing_identifier("interview_id", value)?,
        None => format!(
            "{}-{}",
            normalize_identifier(DEFAULT_INTERVIEW_ID_PREFIX),
            unique_timestamp_nanos()
        ),
    };
    let output_spec_path = build_output_spec_path(&initial_idea, &interview_id);
    let state = DeepInterviewState {
        interview_id,
        initial_idea,
        rounds: Vec::new(),
        current_ambiguity: request.current_ambiguity,
        threshold: request.threshold,
        handoff_path: output_spec_path.clone(),
        output_spec_path,
    };
    let spec_output = persist_deep_interview_session(store, &state)?;
    Ok(DeepInterviewSessionArtifact { state, spec_output })
}

pub fn append_deep_interview_round(
    store: &ModeStateStore,
    request: DeepInterviewAppendRequest,
) -> Result<DeepInterviewSessionArtifact, DeepInterviewError> {
    validate_non_negative("ambiguity_after", request.ambiguity_after)?;
    let interview_id = normalize_existing_identifier("interview_id", &request.interview_id)?;
    let mut state = read_deep_interview_state(store, Some(interview_id.as_str()))?.ok_or_else(|| {
        DeepInterviewError::Format(format!(
            "deep interview session `{interview_id}` does not exist"
        ))
    })?;
    let question = normalize_required_text("question", &request.question)?;
    let answer = normalize_required_text("answer", &request.answer)?;
    let ambiguity_before = request.ambiguity_before.unwrap_or(state.current_ambiguity);
    validate_non_negative("ambiguity_before", ambiguity_before)?;
    if !approximately_equal(ambiguity_before, state.current_ambiguity) {
        return Err(DeepInterviewError::Format(format!(
            "ambiguity_before mismatch: expected {:.6}, got {:.6}",
            state.current_ambiguity, ambiguity_before
        )));
    }
    state.rounds.push(DeepInterviewRound {
        question,
        answer,
        ambiguity: DeepInterviewAmbiguity {
            before: state.current_ambiguity,
            after: request.ambiguity_after,
            note: request.note.filter(|value| !value.trim().is_empty()),
        },
        recorded_at: iso8601_now(),
    });
    state.current_ambiguity = request.ambiguity_after;
    let spec_output = persist_deep_interview_session(store, &state)?;
    Ok(DeepInterviewSessionArtifact { state, spec_output })
}

pub fn read_deep_interview_state(
    store: &ModeStateStore,
    interview_id: Option<&str>,
) -> Result<Option<DeepInterviewState>, DeepInterviewError> {
    let normalized_interview_id = interview_id
        .map(|value| normalize_existing_identifier("interview_id", value))
        .transpose()?;
    store
        .read(DEEP_INTERVIEW_MODE, normalized_interview_id.as_deref())
        .map_err(DeepInterviewError::from)?
        .map(|record| serde_json::from_value(record.context).map_err(DeepInterviewError::from))
        .transpose()
}

pub fn materialize_deep_interview_spec(
    store: &ModeStateStore,
    state: &DeepInterviewState,
) -> Result<WriteFileOutput, DeepInterviewError> {
    let absolute_path = store.workspace_root().join(&state.output_spec_path);
    let rendered = render_deep_interview_spec(state);
    write_file(absolute_path.to_string_lossy().as_ref(), &rendered).map_err(Into::into)
}

fn persist_deep_interview_session(
    store: &ModeStateStore,
    state: &DeepInterviewState,
) -> Result<WriteFileOutput, DeepInterviewError> {
    let absolute_spec_path = store.workspace_root().join(&state.output_spec_path);
    let previous_spec_contents = fs::read_to_string(&absolute_spec_path).ok();
    let spec_output = materialize_deep_interview_spec(store, state)?;

    if let Err(error) = write_deep_interview_state(store, state) {
        rollback_spec_file(&absolute_spec_path, previous_spec_contents.as_deref()).map_err(
            |rollback_error| {
                DeepInterviewError::Format(format!(
                    "{error}; rollback failed: {rollback_error}"
                ))
            },
        )?;
        return Err(error);
    }

    Ok(spec_output)
}

fn write_deep_interview_state(
    store: &ModeStateStore,
    state: &DeepInterviewState,
) -> Result<PathBuf, DeepInterviewError> {
    let mut record = ModeStateRecord::new(DEEP_INTERVIEW_MODE, true);
    record.session_id = Some(state.interview_id.clone());
    record.iteration = Some(state.rounds.len() as u64);
    record.current_phase = Some(state.phase().to_string());
    record.context = serde_json::to_value(state)?;
    store.write(&record).map_err(Into::into)
}

fn render_deep_interview_spec(state: &DeepInterviewState) -> String {
    let mut lines = vec![
        String::from("# Deep Interview Handoff"),
        String::new(),
        format!("- Interview ID: `{}`", state.interview_id),
        format!("- Initial idea: {}", state.initial_idea),
        format!(
            "- Current ambiguity: {:.3} (threshold {:.3})",
            state.current_ambiguity, state.threshold
        ),
        format!("- Status: {}", state.phase()),
        String::from("- Question tool: `AskUserQuestion`"),
        format!("- Handoff path: `{}`", state.handoff_path),
        String::new(),
        String::from("## Interview Rounds"),
        String::new(),
    ];

    if state.rounds.is_empty() {
        lines.push(String::from("_No interview rounds recorded yet._"));
    } else {
        for (index, round) in state.rounds.iter().enumerate() {
            lines.push(format!("### Round {}", index + 1));
            lines.push(format!("- Question: {}", round.question));
            lines.push(format!("- Answer: {}", round.answer));
            lines.push(format!(
                "- Ambiguity: {:.3} -> {:.3}",
                round.ambiguity.before, round.ambiguity.after
            ));
            if let Some(note) = &round.ambiguity.note {
                lines.push(format!("- Note: {}", note));
            }
            lines.push(format!("- Recorded at: `{}`", round.recorded_at));
            lines.push(String::new());
        }
    }

    lines.join("\n")
}

fn normalize_required_text(field: &str, value: &str) -> Result<String, DeepInterviewError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(DeepInterviewError::Format(format!(
            "{field} must not be empty"
        )));
    }
    Ok(trimmed.to_string())
}

fn validate_non_negative(field: &str, value: f64) -> Result<(), DeepInterviewError> {
    if !value.is_finite() || value < 0.0 {
        return Err(DeepInterviewError::Format(format!(
            "{field} must be a non-negative finite number"
        )));
    }
    Ok(())
}

fn normalize_identifier(value: &str) -> String {
    let slug = slugify(value);
    if slug.is_empty() {
        format!("{DEFAULT_INTERVIEW_ID_PREFIX}-{}", unique_timestamp_nanos())
    } else {
        slug
    }
}

fn normalize_existing_identifier(field: &str, value: &str) -> Result<String, DeepInterviewError> {
    let slug = slugify(value);
    if slug.is_empty() {
        Err(DeepInterviewError::Format(format!(
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
    slug.trim_matches('-').chars().take(48).collect()
}

fn build_output_spec_path(initial_idea: &str, interview_id: &str) -> String {
    let idea_slug = slugify(initial_idea);
    let id_slug = slugify(interview_id);
    let suffix = if idea_slug.is_empty() {
        id_slug
    } else if id_slug.is_empty() {
        idea_slug
    } else {
        format!("{idea_slug}-{id_slug}")
    };
    format!(".omx/specs/deep-interview-{suffix}.md")
}

fn rollback_spec_file(
    path: &PathBuf,
    previous_spec_contents: Option<&str>,
) -> Result<(), std::io::Error> {
    match previous_spec_contents {
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

fn approximately_equal(left: f64, right: f64) -> bool {
    (left - right).abs() <= 1e-9
}

fn unique_timestamp_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn iso8601_now() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let seconds = now % 60;
    let minutes = (now / 60) % 60;
    let hours = (now / 3600) % 24;
    let days = now / 86_400;

    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };

    format!(
        "{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z"
    )
}

#[cfg(test)]
mod tests {
    use super::{
        append_deep_interview_round, initialize_deep_interview_session,
        materialize_deep_interview_spec, read_deep_interview_state, write_deep_interview_state,
        DeepInterviewAppendRequest, DeepInterviewInitRequest,
    };
    use crate::ModeStateStore;
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
            let root = std::env::temp_dir().join(format!("runtime-deep-interview-{nanos}"));
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
    fn persists_deep_interview_state_round_trip() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();

        let persisted = initialize_deep_interview_session(
            &store,
            DeepInterviewInitRequest {
                interview_id: Some("runtime-session".to_string()),
                initial_idea: "Milestone 2 runtime persistence".to_string(),
                current_ambiguity: 0.8,
                threshold: 0.25,
            },
        )
        .expect("session should initialize");

        let restored = read_deep_interview_state(&store, Some("runtime-session"))
            .expect("state should read")
            .expect("state should exist");
        assert_eq!(restored, persisted.state);
        assert_eq!(
            restored.output_spec_path,
            ".omx/specs/deep-interview-milestone-2-runtime-persistence-runtime-session.md"
        );

        let global_alias = read_deep_interview_state(&store, None)
            .expect("global alias should read")
            .expect("state should exist");
        assert_eq!(global_alias.interview_id, "runtime-session");
    }

    #[test]
    fn appending_round_updates_state_and_phase() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();

        initialize_deep_interview_session(
            &store,
            DeepInterviewInitRequest {
                interview_id: Some("append-session".to_string()),
                initial_idea: "Append rounds".to_string(),
                current_ambiguity: 0.9,
                threshold: 0.3,
            },
        )
        .expect("session should initialize");

        let persisted = append_deep_interview_round(
            &store,
            DeepInterviewAppendRequest {
                interview_id: "append-session".to_string(),
                question: "What scope should Phase 1 cover?".to_string(),
                answer: "Only mode state, round append, and spec materialization.".to_string(),
                ambiguity_after: 0.2,
                ambiguity_before: None,
                note: Some("Threshold crossed".to_string()),
            },
        )
        .expect("round should append");

        assert_eq!(persisted.state.rounds.len(), 1);
        assert_eq!(persisted.state.current_ambiguity, 0.2);
        assert_eq!(persisted.state.phase(), "handoff");
        assert_eq!(persisted.state.rounds[0].ambiguity.before, 0.9);
        assert_eq!(
            persisted.state.rounds[0].ambiguity.note.as_deref(),
            Some("Threshold crossed")
        );

        let raw_record = store
            .read("deep-interview", Some("append-session"))
            .expect("record should read")
            .expect("record should exist");
        assert_eq!(raw_record.current_phase.as_deref(), Some("handoff"));
        assert_eq!(raw_record.iteration, Some(1));
    }

    #[test]
    fn materialized_spec_uses_expected_path_and_content() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();

        initialize_deep_interview_session(
            &store,
            DeepInterviewInitRequest {
                interview_id: Some("artifact-session".to_string()),
                initial_idea: "Runtime handoff path".to_string(),
                current_ambiguity: 0.6,
                threshold: 0.25,
            },
        )
        .expect("session should initialize");

        let persisted = append_deep_interview_round(
            &store,
            DeepInterviewAppendRequest {
                interview_id: "artifact-session".to_string(),
                question: "What should the handoff include?".to_string(),
                answer: "The state path and the current ambiguity score.".to_string(),
                ambiguity_after: 0.4,
                ambiguity_before: Some(0.6),
                note: None,
            },
        )
        .expect("round should append");

        let spec = materialize_deep_interview_spec(&store, &persisted.state)
            .expect("spec should materialize");
        assert!(
            spec.file_path
                .ends_with(".omx/specs/deep-interview-runtime-handoff-path-artifact-session.md")
        );
        assert!(spec.content.contains("# Deep Interview Handoff"));
        assert!(spec.content.contains("Question tool: `AskUserQuestion`"));
        assert!(spec.content.contains("What should the handoff include?"));
        assert!(spec
            .content
            .contains("The state path and the current ambiguity score."));
    }

    #[test]
    fn duplicate_initial_ideas_produce_distinct_spec_paths() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();

        let first = initialize_deep_interview_session(
            &store,
            DeepInterviewInitRequest {
                interview_id: Some("session-a".to_string()),
                initial_idea: "Shared idea".to_string(),
                current_ambiguity: 0.8,
                threshold: 0.2,
            },
        )
        .expect("first session should initialize");
        let second = initialize_deep_interview_session(
            &store,
            DeepInterviewInitRequest {
                interview_id: Some("session-b".to_string()),
                initial_idea: "Shared idea".to_string(),
                current_ambiguity: 0.7,
                threshold: 0.2,
            },
        )
        .expect("second session should initialize");

        assert_ne!(first.state.output_spec_path, second.state.output_spec_path);
        assert!(first
            .state
            .output_spec_path
            .ends_with("deep-interview-shared-idea-session-a.md"));
        assert!(second
            .state
            .output_spec_path
            .ends_with("deep-interview-shared-idea-session-b.md"));
    }

    #[test]
    fn rejects_ambiguity_before_mismatch() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();

        initialize_deep_interview_session(
            &store,
            DeepInterviewInitRequest {
                interview_id: Some("mismatch-session".to_string()),
                initial_idea: "Validate ambiguity".to_string(),
                current_ambiguity: 0.9,
                threshold: 0.2,
            },
        )
        .expect("session should initialize");

        let error = append_deep_interview_round(
            &store,
            DeepInterviewAppendRequest {
                interview_id: "mismatch-session".to_string(),
                question: "Which score is authoritative?".to_string(),
                answer: "The persisted score.".to_string(),
                ambiguity_after: 0.5,
                ambiguity_before: Some(0.8),
                note: None,
            },
        )
        .expect_err("mismatched ambiguity_before should fail");

        assert!(error.to_string().contains("ambiguity_before mismatch"));
        let restored = read_deep_interview_state(&store, Some("mismatch-session"))
            .expect("state should read")
            .expect("state should exist");
        assert!(restored.rounds.is_empty());
        assert_eq!(restored.current_ambiguity, 0.9);
    }

    #[test]
    fn init_failure_does_not_persist_state() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();
        let omx_dir = workspace.root.join(".omx");
        fs::create_dir_all(&omx_dir).expect("omx dir should exist");
        fs::write(omx_dir.join("specs"), "blocked").expect("blocking file should exist");

        let error = initialize_deep_interview_session(
            &store,
            DeepInterviewInitRequest {
                interview_id: Some("blocked-session".to_string()),
                initial_idea: "Blocked init".to_string(),
                current_ambiguity: 0.8,
                threshold: 0.2,
            },
        )
        .expect_err("init should fail when spec path parent is blocked");

        assert!(error.to_string().contains("File exists") || error.to_string().contains("Not a directory"));
        assert!(read_deep_interview_state(&store, Some("blocked-session"))
            .expect("state lookup should succeed")
            .is_none());
    }

    #[test]
    fn append_failure_keeps_prior_state_intact() {
        let workspace = TestWorkspace::new();
        let store = workspace.store();

        initialize_deep_interview_session(
            &store,
            DeepInterviewInitRequest {
                interview_id: Some("rollback-session".to_string()),
                initial_idea: "Rollback append".to_string(),
                current_ambiguity: 0.75,
                threshold: 0.2,
            },
        )
        .expect("session should initialize");

        let mut state = read_deep_interview_state(&store, Some("rollback-session"))
            .expect("state should read")
            .expect("state should exist");
        state.output_spec_path = ".omx/specs-blocked/rollback.md".to_string();
        state.handoff_path = state.output_spec_path.clone();
        write_deep_interview_state(&store, &state).expect("state should rewrite");

        let blocked_parent = workspace.root.join(".omx").join("specs-blocked");
        fs::write(&blocked_parent, "blocked").expect("blocking file should exist");

        let error = append_deep_interview_round(
            &store,
            DeepInterviewAppendRequest {
                interview_id: "rollback-session".to_string(),
                question: "Will this round persist?".to_string(),
                answer: "It should not.".to_string(),
                ambiguity_after: 0.5,
                ambiguity_before: Some(0.75),
                note: None,
            },
        )
        .expect_err("append should fail when spec write fails");

        assert!(error.to_string().contains("File exists") || error.to_string().contains("Not a directory"));
        let restored = read_deep_interview_state(&store, Some("rollback-session"))
            .expect("state should read")
            .expect("state should exist");
        assert!(restored.rounds.is_empty());
        assert_eq!(restored.current_ambiguity, 0.75);
    }
}
