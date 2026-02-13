//! Orchestration domain: scheduled jobs, significance policy, and persistence.

use chrono::{DateTime, Utc};
use cron::Schedule;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use uuid::Uuid;

const DEFAULT_HIGH_KEYWORDS: &[&str] = &[
    "security alert",
    "unauthorized",
    "compromised",
    "breach",
    "critical",
    "incident",
    "fraud",
    "malware",
    "ransomware",
    "account locked",
];

const DEFAULT_MEDIUM_KEYWORDS: &[&str] = &[
    "warning",
    "verify",
    "new sign-in",
    "new login",
    "failed login",
    "quota",
    "rate limit",
    "degraded",
    "error",
    "retry",
];

fn now_utc() -> DateTime<Utc> {
    Utc::now()
}

fn default_enabled() -> bool {
    true
}

fn default_flag_high_to_mentor() -> bool {
    true
}

fn jobs_file_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join("orchestration_jobs.json")
}

fn logs_file_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join("orchestration_job_logs.json")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationJobMode {
    #[default]
    IdCheck,
    AgenticRun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SignificanceLevel {
    #[default]
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationDecision {
    SilentLog,
    SpawnAgentic,
    FlagMentor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignificancePolicy {
    #[serde(default)]
    pub medium_keywords: Vec<String>,
    #[serde(default)]
    pub high_keywords: Vec<String>,
    #[serde(default)]
    pub escalate_medium: bool,
    #[serde(default = "default_flag_high_to_mentor")]
    pub flag_high_to_mentor: bool,
}

impl Default for SignificancePolicy {
    fn default() -> Self {
        Self {
            medium_keywords: Vec::new(),
            high_keywords: Vec::new(),
            escalate_medium: false,
            flag_high_to_mentor: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestrationJob {
    pub job_id: String,
    pub name: String,
    pub cron: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub mode: OrchestrationJobMode,
    pub goal_template: String,
    #[serde(default)]
    pub significance_policy: SignificancePolicy,
    #[serde(default = "now_utc")]
    pub created_at: DateTime<Utc>,
    #[serde(default = "now_utc")]
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub last_run_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub next_run_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub last_status: Option<String>,
    #[serde(default)]
    pub last_significance: Option<SignificanceLevel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestrationJobLogEntry {
    pub entry_id: String,
    pub job_id: String,
    pub job_name: String,
    pub mode: OrchestrationJobMode,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub significance: SignificanceLevel,
    pub decision: OrchestrationDecision,
    pub status: String,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateOrchestrationJobRequest {
    pub name: String,
    pub cron: String,
    pub mode: OrchestrationJobMode,
    pub goal_template: String,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub significance_policy: SignificancePolicy,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateOrchestrationJobRequest {
    pub name: Option<String>,
    pub cron: Option<String>,
    pub mode: Option<OrchestrationJobMode>,
    pub goal_template: Option<String>,
    pub enabled: Option<bool>,
    pub significance_policy: Option<SignificancePolicy>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SetOrchestrationJobEnabledRequest {
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JobLogsQuery {
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
pub struct JobRunNowResponse {
    pub job_id: String,
    pub status: String,
    pub decision: OrchestrationDecision,
    pub significance: SignificanceLevel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrchestrationJobsResponse {
    pub jobs: Vec<OrchestrationJob>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrchestrationLogsResponse {
    pub logs: Vec<OrchestrationJobLogEntry>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct JobsFile {
    jobs: Vec<OrchestrationJob>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct LogsFile {
    logs: Vec<OrchestrationJobLogEntry>,
}

pub fn list_jobs(agent_dir: &Path) -> Result<Vec<OrchestrationJob>, String> {
    let path = jobs_file_path(agent_dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&path).map_err(|e| format!("Read jobs file: {}", e))?;
    if content.trim().is_empty() {
        return Ok(Vec::new());
    }
    if let Ok(file) = serde_json::from_str::<JobsFile>(&content) {
        return Ok(file.jobs);
    }
    serde_json::from_str::<Vec<OrchestrationJob>>(&content)
        .map_err(|e| format!("Parse jobs file: {}", e))
}

pub fn save_jobs(agent_dir: &Path, jobs: &[OrchestrationJob]) -> Result<(), String> {
    let path = jobs_file_path(agent_dir);
    let payload = JobsFile {
        jobs: jobs.to_vec(),
    };
    let content =
        serde_json::to_string_pretty(&payload).map_err(|e| format!("Serialize jobs: {}", e))?;
    std::fs::write(path, content).map_err(|e| format!("Write jobs file: {}", e))
}

pub fn load_logs(agent_dir: &Path, limit: Option<usize>) -> Result<Vec<OrchestrationJobLogEntry>, String> {
    let path = logs_file_path(agent_dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(&path).map_err(|e| format!("Read logs file: {}", e))?;
    if content.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut logs = if let Ok(file) = serde_json::from_str::<LogsFile>(&content) {
        file.logs
    } else {
        serde_json::from_str::<Vec<OrchestrationJobLogEntry>>(&content)
            .map_err(|e| format!("Parse logs file: {}", e))?
    };
    if let Some(limit) = limit {
        logs.truncate(limit);
    }
    Ok(logs)
}

pub fn append_log_entry(
    agent_dir: &Path,
    entry: OrchestrationJobLogEntry,
    keep_last: usize,
) -> Result<(), String> {
    let mut logs = load_logs(agent_dir, None)?;
    logs.insert(0, entry);
    if logs.len() > keep_last {
        logs.truncate(keep_last);
    }
    let payload = LogsFile { logs };
    let content =
        serde_json::to_string_pretty(&payload).map_err(|e| format!("Serialize logs: {}", e))?;
    std::fs::write(logs_file_path(agent_dir), content).map_err(|e| format!("Write logs file: {}", e))
}

pub fn validate_cron(expr: &str) -> Result<(), String> {
    Schedule::from_str(expr)
        .map(|_| ())
        .map_err(|e| format!("Invalid cron expression '{}': {}", expr, e))
}

pub fn next_run_at(expr: &str, from: DateTime<Utc>) -> Result<DateTime<Utc>, String> {
    let schedule = Schedule::from_str(expr)
        .map_err(|e| format!("Invalid cron expression '{}': {}", expr, e))?;
    schedule
        .after(&from)
        .next()
        .map(|next| next.with_timezone(&Utc))
        .ok_or_else(|| "Cron expression does not yield a future run".to_string())
}

pub fn is_job_due(job: &OrchestrationJob, now: DateTime<Utc>) -> Result<bool, String> {
    let schedule = Schedule::from_str(&job.cron)
        .map_err(|e| format!("Invalid cron expression '{}': {}", job.cron, e))?;
    let reference = job.last_run_at.unwrap_or(job.created_at);
    let next = schedule
        .after(&reference)
        .next()
        .map(|dt| dt.with_timezone(&Utc));
    Ok(next.map(|run_at| run_at <= now).unwrap_or(false))
}

pub fn assess_significance(text: &str, policy: &SignificancePolicy) -> SignificanceLevel {
    let normalized = text.to_lowercase();

    let high = if policy.high_keywords.is_empty() {
        DEFAULT_HIGH_KEYWORDS
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
    } else {
        policy.high_keywords.clone()
    };
    if high
        .iter()
        .map(|k| k.to_lowercase())
        .any(|k| normalized.contains(&k))
    {
        return SignificanceLevel::High;
    }

    let medium = if policy.medium_keywords.is_empty() {
        DEFAULT_MEDIUM_KEYWORDS
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
    } else {
        policy.medium_keywords.clone()
    };
    if medium
        .iter()
        .map(|k| k.to_lowercase())
        .any(|k| normalized.contains(&k))
    {
        return SignificanceLevel::Medium;
    }

    SignificanceLevel::Low
}

pub fn decide_action(
    mode: OrchestrationJobMode,
    significance: SignificanceLevel,
    policy: &SignificancePolicy,
) -> OrchestrationDecision {
    match mode {
        OrchestrationJobMode::AgenticRun => OrchestrationDecision::SpawnAgentic,
        OrchestrationJobMode::IdCheck => match significance {
            SignificanceLevel::Low => OrchestrationDecision::SilentLog,
            SignificanceLevel::Medium => {
                if policy.escalate_medium {
                    OrchestrationDecision::SpawnAgentic
                } else {
                    OrchestrationDecision::SilentLog
                }
            }
            SignificanceLevel::High => {
                if policy.flag_high_to_mentor {
                    OrchestrationDecision::FlagMentor
                } else {
                    OrchestrationDecision::SpawnAgentic
                }
            }
        },
    }
}

pub fn create_job(
    jobs: &mut Vec<OrchestrationJob>,
    request: CreateOrchestrationJobRequest,
    now: DateTime<Utc>,
) -> Result<OrchestrationJob, String> {
    if request.name.trim().is_empty() {
        return Err("Job name is required".to_string());
    }
    if request.goal_template.trim().is_empty() {
        return Err("Job goal_template is required".to_string());
    }
    validate_cron(&request.cron)?;

    let mut job = OrchestrationJob {
        job_id: Uuid::new_v4().to_string(),
        name: request.name.trim().to_string(),
        cron: request.cron.trim().to_string(),
        enabled: request.enabled,
        mode: request.mode,
        goal_template: request.goal_template.trim().to_string(),
        significance_policy: request.significance_policy,
        created_at: now,
        updated_at: now,
        last_run_at: None,
        next_run_at: None,
        last_status: None,
        last_significance: None,
    };
    job.next_run_at = next_run_at(&job.cron, now).ok();
    jobs.push(job.clone());
    Ok(job)
}

pub fn update_job(
    jobs: &mut [OrchestrationJob],
    job_id: &str,
    request: UpdateOrchestrationJobRequest,
    now: DateTime<Utc>,
) -> Result<OrchestrationJob, String> {
    let job = jobs
        .iter_mut()
        .find(|j| j.job_id == job_id)
        .ok_or_else(|| "Job not found".to_string())?;

    if let Some(name) = request.name {
        let name = name.trim();
        if name.is_empty() {
            return Err("Job name cannot be empty".to_string());
        }
        job.name = name.to_string();
    }
    if let Some(cron) = request.cron {
        let cron = cron.trim();
        validate_cron(cron)?;
        job.cron = cron.to_string();
    }
    if let Some(mode) = request.mode {
        job.mode = mode;
    }
    if let Some(goal_template) = request.goal_template {
        let goal_template = goal_template.trim();
        if goal_template.is_empty() {
            return Err("Job goal_template cannot be empty".to_string());
        }
        job.goal_template = goal_template.to_string();
    }
    if let Some(enabled) = request.enabled {
        job.enabled = enabled;
    }
    if let Some(policy) = request.significance_policy {
        job.significance_policy = policy;
    }

    job.updated_at = now;
    job.next_run_at = next_run_at(&job.cron, now).ok();
    Ok(job.clone())
}

pub fn set_job_enabled(
    jobs: &mut [OrchestrationJob],
    job_id: &str,
    enabled: bool,
    now: DateTime<Utc>,
) -> Result<OrchestrationJob, String> {
    let job = jobs
        .iter_mut()
        .find(|j| j.job_id == job_id)
        .ok_or_else(|| "Job not found".to_string())?;
    job.enabled = enabled;
    job.updated_at = now;
    job.next_run_at = next_run_at(&job.cron, now).ok();
    Ok(job.clone())
}

pub fn delete_job(jobs: &mut Vec<OrchestrationJob>, job_id: &str) -> bool {
    let before = jobs.len();
    jobs.retain(|j| j.job_id != job_id);
    jobs.len() != before
}

pub fn update_job_after_execution(
    job: &mut OrchestrationJob,
    now: DateTime<Utc>,
    significance: SignificanceLevel,
    status: String,
) {
    job.last_run_at = Some(now);
    job.last_significance = Some(significance);
    job.last_status = Some(status);
    job.updated_at = now;
    job.next_run_at = next_run_at(&job.cron, now).ok();
}

pub fn build_id_check_prompt(job: &OrchestrationJob) -> String {
    format!(
        "You are Id running a lightweight scheduled background check.\n\
         Job: {}\n\
         Task: {}\n\n\
         Keep this short and concrete:\n\
         1) what you checked,\n\
         2) notable signals,\n\
         3) risk level clues.\n\
         Do not ask the mentor questions.",
        job.name, job.goal_template
    )
}

pub fn make_mentor_attention_message(
    job: &OrchestrationJob,
    significance: SignificanceLevel,
    summary: &str,
) -> String {
    format!(
        "[Orchestration Alert] Job '{}' detected {:?} significance.\n{}",
        job.name, significance, summary
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    #[test]
    fn cron_due_time_matching() {
        let now = Utc.with_ymd_and_hms(2026, 1, 1, 12, 0, 0).unwrap();
        let mut job = OrchestrationJob {
            job_id: "j1".to_string(),
            name: "Periodic check".to_string(),
            cron: "0 * * * * * *".to_string(),
            enabled: true,
            mode: OrchestrationJobMode::IdCheck,
            goal_template: "check inbox".to_string(),
            significance_policy: SignificancePolicy::default(),
            created_at: now - Duration::hours(1),
            updated_at: now - Duration::hours(1),
            last_run_at: Some(now - Duration::minutes(2)),
            next_run_at: None,
            last_status: None,
            last_significance: None,
        };
        assert!(is_job_due(&job, now).unwrap());
        update_job_after_execution(&mut job, now, SignificanceLevel::Low, "ok".to_string());
        assert!(!is_job_due(&job, now).unwrap());
    }

    #[test]
    fn significance_mapping_defaults() {
        let policy = SignificancePolicy::default();
        assert_eq!(
            assess_significance("security alert: unauthorized sign-in", &policy),
            SignificanceLevel::High
        );
        assert_eq!(
            assess_significance("warning: retry later due to rate limit", &policy),
            SignificanceLevel::Medium
        );
        assert_eq!(
            assess_significance("all quiet, no changes", &policy),
            SignificanceLevel::Low
        );
    }

    #[test]
    fn escalation_decision_for_id_check() {
        let mut policy = SignificancePolicy::default();
        policy.flag_high_to_mentor = false;
        assert_eq!(
            decide_action(OrchestrationJobMode::IdCheck, SignificanceLevel::High, &policy),
            OrchestrationDecision::SpawnAgentic
        );
        assert_eq!(
            decide_action(OrchestrationJobMode::IdCheck, SignificanceLevel::Low, &policy),
            OrchestrationDecision::SilentLog
        );
    }

    #[test]
    fn api_serialization_shape() {
        let raw = serde_json::json!({
            "name": "Daily watch",
            "cron": "0 0/30 * * * * *",
            "mode": "id_check",
            "goal_template": "check alerts",
            "enabled": true,
            "significance_policy": {
                "escalate_medium": true,
                "flag_high_to_mentor": true
            }
        });
        let parsed: CreateOrchestrationJobRequest = serde_json::from_value(raw).unwrap();
        assert_eq!(parsed.mode, OrchestrationJobMode::IdCheck);
        assert!(parsed.significance_policy.escalate_medium);
    }
}
