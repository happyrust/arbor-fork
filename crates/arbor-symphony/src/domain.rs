use {
    serde::{Deserialize, Serialize},
    std::{collections::HashMap, path::PathBuf},
};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct IssueBlocker {
    pub id: Option<String>,
    pub identifier: Option<String>,
    pub state: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Issue {
    pub id: String,
    pub identifier: String,
    pub title: String,
    pub description: Option<String>,
    pub priority: Option<i64>,
    pub state: String,
    pub branch_name: Option<String>,
    pub url: Option<String>,
    pub labels: Vec<String>,
    pub blocked_by: Vec<IssueBlocker>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

impl Issue {
    pub fn normalized_state(&self) -> String {
        self.state.to_ascii_lowercase()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodexTotals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub seconds_running: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodexRateLimits {
    pub values: HashMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RunningSnapshot {
    pub issue_id: String,
    pub issue_identifier: String,
    pub state: String,
    pub session_id: Option<String>,
    pub turn_count: u32,
    pub last_event: Option<String>,
    pub last_message: Option<String>,
    pub started_at: String,
    pub last_event_at: Option<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub workspace_path: PathBuf,
    pub retry_attempt: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetrySnapshot {
    pub issue_id: String,
    pub issue_identifier: String,
    pub attempt: u32,
    pub due_at: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeSnapshot {
    pub generated_at: String,
    pub running: Vec<RunningSnapshot>,
    pub retrying: Vec<RetrySnapshot>,
    pub codex_totals: CodexTotals,
    pub rate_limits: Option<CodexRateLimits>,
    pub workflow_path: PathBuf,
    pub last_error: Option<String>,
    pub service_status: ServiceStatus,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServiceStatus {
    #[default]
    Running,
    Degraded,
    Stopped,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SymphonyHttpConfig {
    pub port: Option<u16>,
}
