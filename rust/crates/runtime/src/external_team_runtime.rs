use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalTeamWorker {
    pub worker_id: String,
    pub cli: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_id: Option<String>,
    pub status: String,
}

impl ExternalTeamWorker {
    pub fn normalize(self) -> Result<Self, String> {
        Ok(Self {
            worker_id: normalize_required("worker_id", &self.worker_id)?,
            cli: normalize_required("cli", &self.cli)?,
            pane_id: normalize_optional(self.pane_id),
            status: normalize_required("status", &self.status)?,
        })
    }
}

pub fn normalize_external_workers(
    workers: Vec<ExternalTeamWorker>,
) -> Result<Vec<ExternalTeamWorker>, String> {
    let mut normalized = Vec::with_capacity(workers.len());
    let mut seen = std::collections::BTreeSet::new();

    for worker in workers {
        let worker = worker.normalize()?;
        if !seen.insert(worker.worker_id.clone()) {
            return Err(format!(
                "duplicate external worker_id is not allowed: {}",
                worker.worker_id
            ));
        }
        normalized.push(worker);
    }

    Ok(normalized)
}

fn normalize_required(field: &str, value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err(format!("{field} must not be empty"))
    } else {
        Ok(trimmed.to_string())
    }
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::{normalize_external_workers, ExternalTeamWorker};

    #[test]
    fn normalizes_and_deduplicates_external_workers() {
        let workers = normalize_external_workers(vec![ExternalTeamWorker {
            worker_id: " worker-1 ".to_string(),
            cli: " codex ".to_string(),
            pane_id: Some(" %12 ".to_string()),
            status: " running ".to_string(),
        }])
        .expect("workers should normalize");

        assert_eq!(
            workers,
            vec![ExternalTeamWorker {
                worker_id: "worker-1".to_string(),
                cli: "codex".to_string(),
                pane_id: Some("%12".to_string()),
                status: "running".to_string(),
            }]
        );
    }

    #[test]
    fn rejects_duplicate_worker_ids() {
        let error = normalize_external_workers(vec![
            ExternalTeamWorker {
                worker_id: "worker-1".to_string(),
                cli: "codex".to_string(),
                pane_id: None,
                status: "running".to_string(),
            },
            ExternalTeamWorker {
                worker_id: "worker-1".to_string(),
                cli: "claude".to_string(),
                pane_id: None,
                status: "idle".to_string(),
            },
        ])
        .expect_err("duplicate ids should fail");

        assert!(error.contains("duplicate external worker_id"));
    }
}
