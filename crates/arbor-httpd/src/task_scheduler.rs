use {
    arbor_core::task::{AgentKind, TaskExecution, TaskInfo, TaskStatus},
    chrono::Utc,
    cron::Schedule,
    serde::{Deserialize, Serialize},
    std::{
        collections::HashMap,
        path::{Path, PathBuf},
        str::FromStr,
        time::{Duration, SystemTime, UNIX_EPOCH},
    },
    tokio::sync::broadcast,
};

const MAX_HISTORY_PER_TASK: usize = 50;
const MAX_STDOUT_TAIL_BYTES: usize = 8192;

/// Deserialized from the `[[tasks]]` array in `arbor.toml`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TaskConfig {
    pub name: String,
    pub schedule: String,
    pub command: String,
    pub working_dir: Option<String>,
    pub enabled: Option<bool>,
    #[serde(default)]
    pub trigger: Option<TriggerConfig>,
}

/// Trigger configuration: when/how to spawn an AI agent after task execution.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TriggerConfig {
    pub on_exit_code: Option<i32>,
    pub on_stdout: Option<bool>,
    pub agent: Option<AgentKind>,
    pub prompt_template: Option<String>,
}

/// Internal state for each scheduled task.
struct ManagedTask {
    config: TaskConfig,
    schedule: Option<Schedule>,
    status: TaskStatus,
    last_run_unix_ms: Option<u64>,
    last_exit_code: Option<i32>,
    run_count: u32,
    history: Vec<TaskExecution>,
}

impl ManagedTask {
    fn from_config(config: TaskConfig) -> Self {
        let schedule = Schedule::from_str(&config.schedule).ok();
        let status = if config.enabled.unwrap_or(true) {
            TaskStatus::Idle
        } else {
            TaskStatus::Disabled
        };
        Self {
            config,
            schedule,
            status,
            last_run_unix_ms: None,
            last_exit_code: None,
            run_count: 0,
            history: Vec::new(),
        }
    }

    fn info(&self) -> TaskInfo {
        TaskInfo {
            name: self.config.name.clone(),
            schedule: self.config.schedule.clone(),
            command: self.config.command.clone(),
            status: self.status,
            has_trigger: self.config.trigger.is_some(),
            last_run_unix_ms: self.last_run_unix_ms,
            last_exit_code: self.last_exit_code,
            next_run_unix_ms: self.next_run_unix_ms(),
            run_count: self.run_count,
        }
    }

    fn next_run_unix_ms(&self) -> Option<u64> {
        let schedule = self.schedule.as_ref()?;
        let next = schedule.upcoming(Utc).next()?;
        Some(next.timestamp_millis() as u64)
    }

    fn is_due(&self) -> bool {
        if self.status != TaskStatus::Idle {
            return false;
        }
        let Some(ref schedule) = self.schedule else {
            return false;
        };
        let now = Utc::now();
        // Check if the most recent scheduled time is after our last run
        let Some(upcoming) = schedule.upcoming(Utc).next() else {
            return false;
        };
        // Task is due if next run is within 60 seconds from now
        // (we check every 30 seconds, so this gives a safe window)
        let until_next = upcoming.signed_duration_since(now);
        if until_next.num_seconds() > 60 {
            return false;
        }
        // Don't re-run if we already ran within this schedule window
        if let Some(last_run_ms) = self.last_run_unix_ms {
            let last_run_secs = last_run_ms / 1000;
            let now_secs = now.timestamp() as u64;
            // If we ran less than 60 seconds ago, skip
            if now_secs.saturating_sub(last_run_secs) < 60 {
                return false;
            }
        }
        true
    }

    fn push_execution(&mut self, execution: TaskExecution) {
        if self.history.len() >= MAX_HISTORY_PER_TASK {
            self.history.remove(0);
        }
        self.history.push(execution);
    }
}

/// Real-time task status event.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum TaskEvent {
    Snapshot { tasks: Vec<TaskInfo> },
    Update { task: TaskInfo },
    Execution { execution: TaskExecution },
}

/// Result of checking which tasks are due to run.
pub struct TaskRunRequest {
    pub name: String,
    pub command: String,
    pub working_dir: PathBuf,
    pub trigger: Option<TriggerConfig>,
}

/// Manages `[[tasks]]` from `arbor.toml`, scheduling periodic script execution
/// and conditionally spawning AI agents based on trigger configuration.
pub struct TaskScheduler {
    tasks: HashMap<String, ManagedTask>,
    repo_root: PathBuf,
    broadcast: broadcast::Sender<TaskEvent>,
}

impl TaskScheduler {
    pub fn new(repo_root: PathBuf) -> Self {
        let (broadcast, _) = broadcast::channel(64);
        Self {
            tasks: HashMap::new(),
            repo_root,
            broadcast,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TaskEvent> {
        self.broadcast.subscribe()
    }

    /// Load task configs from parsed `arbor.toml`.
    pub fn load_configs(&mut self, configs: Vec<TaskConfig>) {
        let new_names: std::collections::HashSet<String> =
            configs.iter().map(|c| c.name.clone()).collect();
        self.tasks.retain(|name, _| new_names.contains(name));

        for config in configs {
            let name = config.name.clone();
            self.tasks
                .entry(name)
                .and_modify(|t| {
                    t.schedule = Schedule::from_str(&config.schedule).ok();
                    t.config = config.clone();
                })
                .or_insert_with(|| ManagedTask::from_config(config));
        }
    }

    /// List all scheduled tasks with their current status.
    pub fn list_tasks(&self) -> Vec<TaskInfo> {
        self.tasks.values().map(|t| t.info()).collect()
    }

    /// Get execution history for a specific task.
    pub fn task_history(&self, name: &str) -> Result<Vec<TaskExecution>, String> {
        let task = self
            .tasks
            .get(name)
            .ok_or_else(|| format!("task `{name}` not found"))?;
        Ok(task.history.clone())
    }

    /// Check which tasks are due and return run requests.
    pub fn collect_due_tasks(&mut self) -> Vec<TaskRunRequest> {
        let mut due = Vec::new();

        for task in self.tasks.values_mut() {
            if !task.is_due() {
                continue;
            }

            task.status = TaskStatus::Running;
            let _ = self.broadcast.send(TaskEvent::Update { task: task.info() });

            let working_dir = task
                .config
                .working_dir
                .as_ref()
                .map(|dir| self.repo_root.join(dir))
                .unwrap_or_else(|| self.repo_root.clone());

            due.push(TaskRunRequest {
                name: task.config.name.clone(),
                command: task.config.command.clone(),
                working_dir,
                trigger: task.config.trigger.clone(),
            });
        }

        due
    }

    /// Record completion of a task execution.
    pub fn record_completion(
        &mut self,
        name: &str,
        exit_code: i32,
        stdout: Option<String>,
        started_at_unix_ms: u64,
        agent_spawned: bool,
    ) {
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;

        if let Some(task) = self.tasks.get_mut(name) {
            task.status = if task.config.enabled.unwrap_or(true) {
                TaskStatus::Idle
            } else {
                TaskStatus::Disabled
            };
            task.last_run_unix_ms = Some(now_ms);
            task.last_exit_code = Some(exit_code);
            task.run_count += 1;

            let execution = TaskExecution {
                task_name: name.to_owned(),
                started_at_unix_ms,
                finished_at_unix_ms: Some(now_ms),
                exit_code: Some(exit_code),
                stdout_tail: stdout,
                agent_spawned,
            };

            task.push_execution(execution.clone());

            let _ = self.broadcast.send(TaskEvent::Update { task: task.info() });
            let _ = self.broadcast.send(TaskEvent::Execution { execution });
        }
    }

    /// Manually trigger a task by name (ignoring schedule).
    pub fn mark_running(&mut self, name: &str) -> Result<TaskRunRequest, String> {
        let task = self
            .tasks
            .get_mut(name)
            .ok_or_else(|| format!("task `{name}` not found"))?;

        if task.status == TaskStatus::Running {
            return Err(format!("task `{name}` is already running"));
        }

        task.status = TaskStatus::Running;
        let _ = self.broadcast.send(TaskEvent::Update { task: task.info() });

        let working_dir = task
            .config
            .working_dir
            .as_ref()
            .map(|dir| self.repo_root.join(dir))
            .unwrap_or_else(|| self.repo_root.clone());

        Ok(TaskRunRequest {
            name: task.config.name.clone(),
            command: task.config.command.clone(),
            working_dir,
            trigger: task.config.trigger.clone(),
        })
    }

    /// Get a snapshot event for broadcasting to WebSocket clients.
    pub fn snapshot_event(&self) -> TaskEvent {
        TaskEvent::Snapshot {
            tasks: self.list_tasks(),
        }
    }
}

/// Execute a task command, returning (exit_code, stdout).
pub async fn execute_task(command: &str, working_dir: &Path) -> (i32, String) {
    let result = tokio::process::Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(working_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await;

    match result {
        Ok(output) => {
            let exit_code = output.status.code().unwrap_or(-1);
            let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
            if stdout.len() > MAX_STDOUT_TAIL_BYTES {
                stdout = stdout[stdout.len() - MAX_STDOUT_TAIL_BYTES..].to_owned();
            }
            (exit_code, stdout)
        },
        Err(error) => (-1, format!("failed to execute command: {error}")),
    }
}

/// Check if a trigger should fire based on the execution results.
pub fn should_trigger(trigger: &TriggerConfig, exit_code: i32, stdout: &str) -> bool {
    let exit_ok = match trigger.on_exit_code {
        Some(expected) => exit_code == expected,
        None => true,
    };

    let stdout_ok = match trigger.on_stdout {
        Some(true) => !stdout.trim().is_empty(),
        Some(false) => stdout.trim().is_empty(),
        None => true,
    };

    exit_ok && stdout_ok
}

/// Build the agent command line for spawning.
pub fn build_agent_command(
    trigger: &TriggerConfig,
    stdout: &str,
    repo_root: &Path,
) -> Option<(String, Vec<String>)> {
    let agent = trigger.agent.as_ref()?;
    let prompt_template = trigger.prompt_template.as_deref()?;

    let prompt = prompt_template
        .replace("{stdout}", stdout)
        .replace("{repo_root}", &repo_root.display().to_string());

    match agent {
        AgentKind::Claude => Some(("claude".to_owned(), vec![
            "--print".to_owned(),
            "--dangerously-skip-permissions".to_owned(),
            "-p".to_owned(),
            prompt,
        ])),
        AgentKind::Codex => Some(("codex".to_owned(), vec!["--prompt".to_owned(), prompt])),
    }
}

/// Spawn an AI agent as a background process.
pub async fn spawn_agent(program: &str, args: &[String], working_dir: &Path) -> Result<(), String> {
    tracing::info!(
        program,
        working_dir = %working_dir.display(),
        "spawning agent"
    );

    let result = tokio::process::Command::new(program)
        .args(args)
        .current_dir(working_dir)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .spawn();

    match result {
        Ok(_child) => {
            tracing::info!(program, "agent process spawned");
            Ok(())
        },
        Err(error) => {
            let msg = format!("failed to spawn agent `{program}`: {error}");
            tracing::error!("{msg}");
            Err(msg)
        },
    }
}

/// Load `[[tasks]]` from an `arbor.toml` file.
pub fn load_task_configs(repo_root: &Path) -> Vec<TaskConfig> {
    let path = repo_root.join("arbor.toml");
    if !path.exists() {
        return Vec::new();
    }

    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    #[derive(Deserialize)]
    struct ArborToml {
        #[serde(default)]
        tasks: Vec<TaskConfig>,
    }

    match toml::from_str::<ArborToml>(&content) {
        Ok(parsed) => parsed.tasks,
        Err(_) => Vec::new(),
    }
}

/// Full background loop: check schedule, execute, trigger agent.
pub async fn run_task_loop(
    scheduler: std::sync::Arc<tokio::sync::Mutex<TaskScheduler>>,
    repo_root: PathBuf,
) {
    let mut interval = tokio::time::interval(Duration::from_secs(30));

    loop {
        interval.tick().await;

        let due_tasks = {
            let mut sched = scheduler.lock().await;
            sched.collect_due_tasks()
        };

        for task_request in due_tasks {
            let scheduler = scheduler.clone();
            let repo_root = repo_root.clone();

            tokio::spawn(async move {
                let started_at_ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;

                tracing::info!(
                    task = task_request.name,
                    command = task_request.command,
                    "executing scheduled task"
                );

                let (exit_code, stdout) =
                    execute_task(&task_request.command, &task_request.working_dir).await;

                tracing::info!(
                    task = task_request.name,
                    exit_code,
                    stdout_len = stdout.len(),
                    "task execution completed"
                );

                let mut agent_spawned = false;

                if let Some(ref trigger) = task_request.trigger
                    && should_trigger(trigger, exit_code, &stdout)
                    && let Some((program, args)) = build_agent_command(trigger, &stdout, &repo_root)
                    && spawn_agent(&program, &args, &task_request.working_dir)
                        .await
                        .is_ok()
                {
                    agent_spawned = true;
                }

                let stdout_tail = if stdout.is_empty() {
                    None
                } else {
                    Some(stdout)
                };

                let mut sched = scheduler.lock().await;
                sched.record_completion(
                    &task_request.name,
                    exit_code,
                    stdout_tail,
                    started_at_ms,
                    agent_spawned,
                );
            });
        }
    }
}
