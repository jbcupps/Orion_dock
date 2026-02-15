//! Cooperative install skill: perform direct installs when possible, otherwise
//! try the toolbox sidecar, and finally generate mentor-run host scripts for
//! bash or PowerShell.

use async_trait::async_trait;
use orion_skills::{
    CapabilityDescriptor, CostEstimate, ExecutionContext, FileSystemPermission, HealthStatus,
    Permission, Skill, SkillConfig, SkillError, SkillHealth, SkillManifest, SkillResult,
    ToolDescriptor, ToolOutput, ToolParams, TriggerDescriptor,
};
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::collections::{BTreeSet, HashMap};
use std::env;
use std::fs;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;

/// Response from the toolbox /exec endpoint (subset of fields we need).
#[derive(Debug, Clone, Deserialize)]
struct ToolboxExecResponse {
    success: bool,
    exit_code: i32,
    stdout: String,
    stderr: String,
}

/// Maximum captured stdout/stderr size per stream.
const MAX_OUTPUT_BYTES: usize = 65_536;

/// Default timeout for direct install attempts.
const DEFAULT_TIMEOUT_SECS: u64 = 120;

/// Supported package managers.
const PACKAGE_MANAGERS: &[&str] = &[
    "apt-get", "apt", "dnf", "yum", "apk", "pacman", "zypper", "brew", "winget", "choco", "scoop",
    "pip", "pip3", "npm", "cargo",
];

/// Command patterns blocked for generated scripts.
const BLOCKED_SCRIPT_PATTERNS: &[&str] = &[
    "rm -rf /",
    "rm -rf /*",
    "rm -rf ~",
    "mkfs.",
    "dd if=/dev/zero",
    ":(){:|:&};:",
    "chmod -r 777 /",
    "chown -r",
    "shutdown",
    "reboot",
    "poweroff",
    "halt",
    "format c:",
    "remove-item -recurse -force c:\\",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScriptPlatform {
    Bash,
    PowerShell,
}

impl ScriptPlatform {
    fn as_str(self) -> &'static str {
        match self {
            ScriptPlatform::Bash => "bash",
            ScriptPlatform::PowerShell => "powershell",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            ScriptPlatform::Bash => "sh",
            ScriptPlatform::PowerShell => "ps1",
        }
    }

    fn default_from_host_hint() -> Self {
        let host_os = env::var("HOST_OS")
            .unwrap_or_else(|_| "windows".to_string())
            .to_ascii_lowercase();
        match host_os.as_str() {
            "linux" | "macos" | "darwin" => ScriptPlatform::Bash,
            _ => ScriptPlatform::PowerShell,
        }
    }

    fn from_requested(value: Option<&str>) -> Result<Self, String> {
        let Some(raw) = value else {
            return Ok(Self::default_from_host_hint());
        };
        let normalized = raw.trim().to_ascii_lowercase();
        if normalized.is_empty() || normalized == "auto" {
            return Ok(Self::default_from_host_hint());
        }
        match normalized.as_str() {
            "bash" | "sh" | "linux" | "macos" | "darwin" => Ok(ScriptPlatform::Bash),
            "powershell" | "pwsh" | "windows" | "ps" => Ok(ScriptPlatform::PowerShell),
            _ => Err(format!(
                "Unsupported platform '{}'. Use bash, powershell, or auto.",
                raw
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct EnvironmentSnapshot {
    is_containerized: bool,
    os: String,
    available_package_managers: Vec<String>,
    writable_paths: Vec<String>,
    has_network: bool,
    host_os_hint: String,
}

#[derive(Debug, Clone, Deserialize)]
struct ScriptStepInput {
    command: String,
    explanation: String,
}

#[derive(Debug, Clone, Serialize)]
struct CommandExecution {
    command: String,
    success: bool,
    exit_code: i32,
    stdout: String,
    stderr: String,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

/// Cooperative install skill.
pub struct CooperativeInstallSkill {
    manifest: SkillManifest,
    max_timeout_secs: u64,
}

impl CooperativeInstallSkill {
    /// Parse the embedded skill.toml manifest.
    pub fn default_manifest() -> SkillManifest {
        let toml_str = include_str!("../skill.toml");
        SkillManifest::parse(toml_str).expect("Failed to parse cooperative install skill.toml")
    }

    /// Create a new cooperative install skill.
    pub fn new(manifest: SkillManifest) -> Self {
        Self {
            manifest,
            max_timeout_secs: 300,
        }
    }

    fn normalize_os(raw: &str) -> String {
        match raw {
            "macos" | "darwin" => "macos".to_string(),
            "windows" => "windows".to_string(),
            "linux" => "linux".to_string(),
            other => other.to_string(),
        }
    }

    fn detect_environment_snapshot(&self) -> EnvironmentSnapshot {
        EnvironmentSnapshot {
            is_containerized: Self::detect_containerized(),
            os: Self::normalize_os(std::env::consts::OS),
            available_package_managers: Self::discover_package_managers(),
            writable_paths: Self::detect_writable_paths(),
            has_network: Self::detect_network_connectivity(),
            host_os_hint: env::var("HOST_OS")
                .unwrap_or_else(|_| "windows".to_string())
                .to_ascii_lowercase(),
        }
    }

    fn detect_containerized() -> bool {
        if Path::new("/.dockerenv").exists() {
            return true;
        }

        if let Ok(cgroup) = fs::read_to_string("/proc/1/cgroup") {
            let cgroup_lc = cgroup.to_ascii_lowercase();
            if cgroup_lc.contains("docker")
                || cgroup_lc.contains("containerd")
                || cgroup_lc.contains("kubepods")
                || cgroup_lc.contains("podman")
            {
                return true;
            }
        }

        env::var("KUBERNETES_SERVICE_HOST").is_ok()
            || env::var("DOTNET_RUNNING_IN_CONTAINER")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false)
    }

    fn command_exists(command: &str) -> bool {
        if command.is_empty() {
            return false;
        }

        let Some(path_var) = env::var_os("PATH") else {
            return false;
        };

        if cfg!(target_os = "windows") {
            let pathext = env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into());
            let extensions: Vec<String> = pathext
                .split(';')
                .filter(|v| !v.trim().is_empty())
                .map(|v| v.trim().trim_start_matches('.').to_ascii_lowercase())
                .collect();

            for dir in env::split_paths(&path_var) {
                let direct = dir.join(command);
                if direct.is_file() {
                    return true;
                }
                if !command.contains('.') {
                    for ext in &extensions {
                        let candidate = dir.join(format!("{}.{}", command, ext));
                        if candidate.is_file() {
                            return true;
                        }
                    }
                }
            }
            return false;
        }

        for dir in env::split_paths(&path_var) {
            if dir.join(command).is_file() {
                return true;
            }
        }
        false
    }

    fn discover_package_managers() -> Vec<String> {
        PACKAGE_MANAGERS
            .iter()
            .filter(|mgr| Self::command_exists(mgr))
            .map(|mgr| (*mgr).to_string())
            .collect()
    }

    fn is_path_writable(path: &Path) -> bool {
        if fs::create_dir_all(path).is_err() {
            return false;
        }

        let probe_name = format!(
            ".orion-write-probe-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        let probe = path.join(probe_name);
        match fs::write(&probe, b"probe") {
            Ok(()) => {
                let _ = fs::remove_file(&probe);
                true
            }
            Err(_) => false,
        }
    }

    fn detect_writable_paths() -> Vec<String> {
        let mut candidates: BTreeSet<PathBuf> = BTreeSet::new();
        if let Ok(root) = env::var("ORION_DATA_DIR") {
            if !root.trim().is_empty() {
                candidates.insert(PathBuf::from(root));
            }
        }
        if let Ok(cwd) = env::current_dir() {
            candidates.insert(cwd);
        }
        candidates.insert(env::temp_dir());

        candidates
            .into_iter()
            .filter(|path| Self::is_path_writable(path))
            .map(|path| path.display().to_string())
            .collect()
    }

    fn detect_network_connectivity() -> bool {
        for endpoint in ["1.1.1.1:53", "8.8.8.8:53", "9.9.9.9:53"] {
            let Ok(addr) = endpoint.parse::<SocketAddr>() else {
                continue;
            };
            if TcpStream::connect_timeout(&addr, Duration::from_millis(700)).is_ok() {
                return true;
            }
        }
        false
    }

    fn normalize_package_manager(raw: &str) -> Result<String, String> {
        let normalized = raw.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            return Err("package_manager cannot be empty".to_string());
        }
        if PACKAGE_MANAGERS.contains(&normalized.as_str()) {
            Ok(normalized)
        } else {
            Err(format!(
                "Unsupported package_manager '{}'. Supported: {}",
                raw,
                PACKAGE_MANAGERS.join(", ")
            ))
        }
    }

    fn validate_package_name(name: &str) -> Result<String, String> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err("Package names cannot be empty".to_string());
        }
        if !trimmed.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+' | ':' | '/' | '@')
        }) {
            return Err(format!(
                "Invalid package name '{}'. Allowed chars: letters, numbers, . - _ + : / @",
                name
            ));
        }
        Ok(trimmed.to_string())
    }

    fn select_runtime_manager(
        env_snapshot: &EnvironmentSnapshot,
        requested_manager: Option<&str>,
    ) -> Option<String> {
        if let Some(requested) = requested_manager {
            if env_snapshot
                .available_package_managers
                .iter()
                .any(|mgr| mgr == requested)
            {
                return Some(requested.to_string());
            }
            return None;
        }

        let preference: &[&str] = if env_snapshot.os == "windows" {
            &["winget", "choco", "scoop", "pip", "npm", "cargo"]
        } else {
            &[
                "apt-get", "apt", "dnf", "yum", "apk", "pacman", "zypper", "brew", "pip3", "pip",
                "npm", "cargo",
            ]
        };

        for manager in preference {
            if env_snapshot
                .available_package_managers
                .iter()
                .any(|available| available == manager)
            {
                return Some((*manager).to_string());
            }
        }
        None
    }

    fn default_host_manager(platform: ScriptPlatform) -> &'static str {
        match platform {
            ScriptPlatform::Bash => "apt-get",
            ScriptPlatform::PowerShell => "winget",
        }
    }

    fn build_direct_install_command(manager: &str, packages: &[String]) -> Result<String, String> {
        let package_list = packages.join(" ");
        let cmd = match manager {
            "apt-get" => format!("apt-get update && apt-get install -y {}", package_list),
            "apt" => format!("apt update && apt install -y {}", package_list),
            "dnf" => format!("dnf install -y {}", package_list),
            "yum" => format!("yum install -y {}", package_list),
            "apk" => format!("apk add --no-cache {}", package_list),
            "pacman" => format!("pacman -S --noconfirm {}", package_list),
            "zypper" => format!("zypper --non-interactive install {}", package_list),
            "brew" => format!("brew install {}", package_list),
            "choco" => format!("choco install -y {}", package_list),
            "scoop" => format!("scoop install {}", package_list),
            "pip" | "pip3" => format!("{} install {}", manager, package_list),
            "npm" => format!("npm install -g {}", package_list),
            "cargo" => format!("cargo install {}", package_list),
            "winget" => packages
                .iter()
                .map(|pkg| {
                    format!(
                        "winget install --id {} --exact --accept-package-agreements --accept-source-agreements",
                        pkg
                    )
                })
                .collect::<Vec<_>>()
                .join(" && "),
            _ => {
                return Err(format!(
                    "Unsupported package manager '{}' for direct install",
                    manager
                ));
            }
        };
        Ok(cmd)
    }

    async fn execute_shell_command(&self, command: &str) -> Result<CommandExecution, String> {
        let timeout = Duration::from_secs(DEFAULT_TIMEOUT_SECS.min(self.max_timeout_secs));
        let shell = if cfg!(target_os = "windows") {
            "cmd"
        } else {
            "sh"
        };
        let shell_arg = if cfg!(target_os = "windows") {
            "/C"
        } else {
            "-c"
        };

        let mut cmd = Command::new(shell);
        cmd.arg(shell_arg).arg(command);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        let output = match tokio::time::timeout(timeout, cmd.output()).await {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => return Err(format!("Failed to execute command: {}", e)),
            Err(_) => {
                return Err(format!(
                    "Direct install timed out after {} seconds",
                    timeout.as_secs()
                ));
            }
        };

        let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();

        let stdout_truncated = Self::truncate_output(&mut stdout);
        let stderr_truncated = Self::truncate_output(&mut stderr);

        Ok(CommandExecution {
            command: command.to_string(),
            success: output.status.success(),
            exit_code: output.status.code().unwrap_or(-1),
            stdout,
            stderr,
            stdout_truncated,
            stderr_truncated,
        })
    }

    fn truncate_output(content: &mut String) -> bool {
        if content.len() <= MAX_OUTPUT_BYTES {
            return false;
        }
        content.truncate(MAX_OUTPUT_BYTES);
        content.push_str("\n... [output truncated]");
        true
    }

    fn validate_safe_command(command: &str) -> Result<(), String> {
        let lower = command.to_ascii_lowercase();
        for blocked in BLOCKED_SCRIPT_PATTERNS {
            if lower.contains(blocked) {
                return Err(format!(
                    "Command contains blocked pattern '{}': {}",
                    blocked, command
                ));
            }
        }
        Ok(())
    }

    fn sanitize_name(raw: &str) -> String {
        let mut out = String::new();
        let mut previous_dash = false;

        for ch in raw.chars() {
            if ch.is_ascii_alphanumeric() {
                out.push(ch.to_ascii_lowercase());
                previous_dash = false;
            } else if !previous_dash {
                out.push('-');
                previous_dash = true;
            }
        }

        let cleaned = out.trim_matches('-').to_string();
        if cleaned.is_empty() {
            "script".to_string()
        } else {
            cleaned
        }
    }

    fn mentor_scripts_root() -> PathBuf {
        if let Ok(root) = env::var("ORION_DATA_DIR") {
            return PathBuf::from(root).join("mentor-scripts");
        }
        env::temp_dir().join("orion-data").join("mentor-scripts")
    }

    fn persist_script_with_root(
        &self,
        root_override: Option<&Path>,
        base_name: &str,
        platform: ScriptPlatform,
        content: &str,
    ) -> SkillResult<PathBuf> {
        let dir = root_override
            .map(|p| p.to_path_buf())
            .unwrap_or_else(Self::mentor_scripts_root);

        fs::create_dir_all(&dir).map_err(|e| {
            SkillError::ToolFailed(format!("Failed to create script directory: {}", e))
        })?;

        let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let file_name = format!(
            "{}-{}.{}",
            timestamp,
            Self::sanitize_name(base_name),
            platform.extension()
        );
        let path = dir.join(file_name);

        fs::write(&path, content)
            .map_err(|e| SkillError::ToolFailed(format!("Failed to write script file: {}", e)))?;

        Ok(path)
    }

    fn persist_script(
        &self,
        base_name: &str,
        platform: ScriptPlatform,
        content: &str,
    ) -> SkillResult<PathBuf> {
        self.persist_script_with_root(None, base_name, platform, content)
    }

    fn build_mentor_instructions(path: &Path, platform: ScriptPlatform, reason: &str) -> String {
        let run_command = match platform {
            ScriptPlatform::Bash => format!("bash \"{}\"", path.display()),
            ScriptPlatform::PowerShell => {
                format!(
                    "powershell.exe -ExecutionPolicy Bypass -File \"{}\"",
                    path.display()
                )
            }
        };

        format!(
            "Orion could not complete this directly: {}.\n\
1) Retrieve the generated script at: {}\n\
2) Run it on the host machine:\n   {}\n\
3) Review output and confirm verification lines succeeded.\n\
If the path is not directly accessible from the host, copy `script_content` into a local file and run it.",
            reason,
            path.display(),
            run_command
        )
    }

    fn shell_single_quote(value: &str) -> String {
        value.replace('\'', "'\\''")
    }

    fn ps_escape_double_quoted(value: &str) -> String {
        value.replace('`', "``").replace('"', "`\"")
    }

    fn escape_for_double_quotes(value: &str) -> String {
        value.replace('\\', "\\\\").replace('"', "\\\"")
    }

    fn build_bash_script(
        description: &str,
        steps: &[ScriptStepInput],
        requires_admin: bool,
        idempotent_checks: &[String],
    ) -> String {
        let mut lines = Vec::<String>::new();
        lines.push("#!/usr/bin/env bash".to_string());
        lines.push("set -euo pipefail".to_string());
        lines.push(String::new());
        lines.push("# Cooperative host script generated by Orion".to_string());
        lines.push(format!("# Description: {}", description));
        lines.push(format!(
            "# Generated UTC: {}",
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        ));
        lines.push(String::new());
        lines.push(format!(
            "echo \"[orion] Starting: {}\"",
            Self::escape_for_double_quotes(description)
        ));

        if requires_admin {
            lines.push("if [ \"${EUID:-$(id -u)}\" -ne 0 ]; then".to_string());
            lines.push(
                "  echo \"[orion] This script requires root privileges. Re-run with sudo.\""
                    .to_string(),
            );
            lines.push("  exit 1".to_string());
            lines.push("fi".to_string());
            lines.push(String::new());
        }

        if !idempotent_checks.is_empty() {
            lines.push("echo \"[orion] Preflight idempotency checks\"".to_string());
            for check in idempotent_checks {
                let escaped = Self::shell_single_quote(check);
                lines.push(format!("if bash -lc '{}' >/dev/null 2>&1; then", escaped));
                lines.push("  echo \"[orion] Check already satisfied.\"".to_string());
                lines.push("else".to_string());
                lines.push(
                    "  echo \"[orion] Check not yet satisfied; steps will run.\"".to_string(),
                );
                lines.push("fi".to_string());
            }
            lines.push(String::new());
        }

        for (idx, step) in steps.iter().enumerate() {
            lines.push(format!("# Step {}: {}", idx + 1, step.explanation));
            lines.push(format!(
                "echo \"[orion] Step {}/{}: {}\"",
                idx + 1,
                steps.len(),
                Self::escape_for_double_quotes(&step.explanation)
            ));
            for cmd_line in step.command.lines() {
                lines.push(cmd_line.to_string());
            }
            lines.push(String::new());
        }

        if !idempotent_checks.is_empty() {
            lines.push("echo \"[orion] Verification checks\"".to_string());
            for check in idempotent_checks {
                let escaped = Self::shell_single_quote(check);
                lines.push(format!("if bash -lc '{}' >/dev/null 2>&1; then", escaped));
                lines.push("  echo \"[orion] Verification passed.\"".to_string());
                lines.push("else".to_string());
                lines.push("  echo \"[orion] Verification failed.\"".to_string());
                lines.push("  exit 1".to_string());
                lines.push("fi".to_string());
            }
            lines.push(String::new());
        } else {
            lines.push(
                "echo \"[orion] No explicit verification checks were provided.\"".to_string(),
            );
        }

        lines.push("echo \"[orion] Script completed successfully.\"".to_string());
        lines.join("\n")
    }

    fn build_powershell_script(
        description: &str,
        steps: &[ScriptStepInput],
        requires_admin: bool,
        idempotent_checks: &[String],
    ) -> String {
        let mut lines = Vec::<String>::new();
        if requires_admin {
            lines.push("#Requires -RunAsAdministrator".to_string());
        }
        lines.push("$ErrorActionPreference = 'Stop'".to_string());
        lines.push("Set-StrictMode -Version Latest".to_string());
        lines.push(String::new());
        lines.push("# Cooperative host script generated by Orion".to_string());
        lines.push(format!("# Description: {}", description));
        lines.push(format!(
            "# Generated UTC: {}",
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        ));
        lines.push(String::new());
        lines.push(format!(
            "Write-Host \"[orion] Starting: {}\"",
            Self::ps_escape_double_quoted(description)
        ));

        if requires_admin {
            lines.push(
                "$isAdmin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)".to_string(),
            );
            lines.push("if (-not $isAdmin) {".to_string());
            lines.push("  throw \"[orion] Administrator privileges are required.\"".to_string());
            lines.push("}".to_string());
            lines.push(String::new());
        }

        if !idempotent_checks.is_empty() {
            lines.push("function Test-OrionCommand {".to_string());
            lines.push("  param([string]$Command)".to_string());
            lines.push("  try {".to_string());
            lines.push("    Invoke-Expression $Command | Out-Null".to_string());
            lines.push("    if ($LASTEXITCODE -ne $null -and $LASTEXITCODE -ne 0) {".to_string());
            lines.push("      return $false".to_string());
            lines.push("    }".to_string());
            lines.push("    return $true".to_string());
            lines.push("  } catch {".to_string());
            lines.push("    return $false".to_string());
            lines.push("  }".to_string());
            lines.push("}".to_string());
            lines.push(String::new());
            lines.push("Write-Host \"[orion] Preflight idempotency checks\"".to_string());
            for check in idempotent_checks {
                let escaped = Self::ps_escape_double_quoted(check);
                lines.push(format!("if (Test-OrionCommand \"{}\") {{", escaped));
                lines.push("  Write-Host \"[orion] Check already satisfied.\"".to_string());
                lines.push("} else {".to_string());
                lines.push(
                    "  Write-Host \"[orion] Check not yet satisfied; steps will run.\"".to_string(),
                );
                lines.push("}".to_string());
            }
            lines.push(String::new());
        }

        for (idx, step) in steps.iter().enumerate() {
            lines.push(format!("# Step {}: {}", idx + 1, step.explanation));
            lines.push(format!(
                "Write-Host \"[orion] Step {}/{}: {}\"",
                idx + 1,
                steps.len(),
                Self::ps_escape_double_quoted(&step.explanation)
            ));
            for cmd_line in step.command.lines() {
                lines.push(cmd_line.to_string());
            }
            lines.push(String::new());
        }

        if !idempotent_checks.is_empty() {
            lines.push("Write-Host \"[orion] Verification checks\"".to_string());
            for check in idempotent_checks {
                let escaped = Self::ps_escape_double_quoted(check);
                lines.push(format!("if (Test-OrionCommand \"{}\") {{", escaped));
                lines.push("  Write-Host \"[orion] Verification passed.\"".to_string());
                lines.push("} else {".to_string());
                lines.push("  throw \"[orion] Verification failed.\"".to_string());
                lines.push("}".to_string());
            }
            lines.push(String::new());
        } else {
            lines.push(
                "Write-Host \"[orion] No explicit verification checks were provided.\"".to_string(),
            );
        }

        lines.push("Write-Host \"[orion] Script completed successfully.\"".to_string());
        lines.join("\n")
    }

    fn manager_requires_admin(manager: &str, platform: ScriptPlatform) -> bool {
        match platform {
            ScriptPlatform::Bash => {
                matches!(
                    manager,
                    "apt-get" | "apt" | "dnf" | "yum" | "apk" | "pacman" | "zypper"
                )
            }
            ScriptPlatform::PowerShell => matches!(manager, "winget" | "choco"),
        }
    }

    fn bash_install_step(manager: &str, package: &str) -> ScriptStepInput {
        let (check_cmd, install_cmd) = match manager {
            "apt-get" | "apt" => (
                format!("dpkg -s \"{}\" >/dev/null 2>&1", package),
                format!("{} install -y \"{}\"", manager, package),
            ),
            "dnf" | "yum" | "zypper" => (
                format!("rpm -q \"{}\" >/dev/null 2>&1", package),
                format!("{} install -y \"{}\"", manager, package),
            ),
            "apk" => (
                format!("apk info -e \"{}\" >/dev/null 2>&1", package),
                format!("apk add --no-cache \"{}\"", package),
            ),
            "pacman" => (
                format!("pacman -Qi \"{}\" >/dev/null 2>&1", package),
                format!("pacman -S --noconfirm \"{}\"", package),
            ),
            "brew" => (
                format!("brew list --versions \"{}\" >/dev/null 2>&1", package),
                format!("brew install \"{}\"", package),
            ),
            "pip" | "pip3" => (
                format!("{} show \"{}\" >/dev/null 2>&1", manager, package),
                format!("{} install \"{}\"", manager, package),
            ),
            "npm" => (
                format!("npm list -g \"{}\" --depth=0 >/dev/null 2>&1", package),
                format!("npm install -g \"{}\"", package),
            ),
            "cargo" => (
                format!(
                    "cargo install --list | grep -F '{} v' >/dev/null 2>&1",
                    package
                ),
                format!("cargo install \"{}\"", package),
            ),
            _ => (
                format!("command -v \"{}\" >/dev/null 2>&1", package),
                format!("{} install \"{}\"", manager, package),
            ),
        };

        let command = format!(
            "if {check_cmd}; then
  echo \"[orion] {package} already installed.\"
else
  {install_cmd}
fi
if ! {check_cmd}; then
  echo \"[orion] Verification failed for {package}.\"
  exit 1
fi"
        );

        ScriptStepInput {
            command,
            explanation: format!("Ensure package '{}' is installed", package),
        }
    }

    fn powershell_install_step(manager: &str, package: &str) -> ScriptStepInput {
        let (check_expr, install_cmd) = match manager {
            "winget" => (
                format!(
                    "winget list --exact --id '{}' 2>$null | Select-String -SimpleMatch '{}' -Quiet",
                    package, package
                ),
                format!(
                    "winget install --id '{}' --exact --accept-package-agreements --accept-source-agreements",
                    package
                ),
            ),
            "choco" => (
                format!(
                    "choco list --local-only --exact '{}' 2>$null | Select-String -SimpleMatch '{}' -Quiet",
                    package, package
                ),
                format!("choco install -y '{}'", package),
            ),
            "scoop" => (
                format!(
                    "scoop list 2>$null | Select-String -Pattern '^{}\\s' -Quiet",
                    package
                ),
                format!("scoop install '{}'", package),
            ),
            "pip" | "pip3" => (
                format!(
                    "{} show '{}' 2>$null | Select-String -Pattern '^Name:' -Quiet",
                    manager, package
                ),
                format!("{} install '{}'", manager, package),
            ),
            "npm" => (
                format!(
                    "npm list -g '{}' --depth=0 2>$null | Select-String -SimpleMatch '{}' -Quiet",
                    package, package
                ),
                format!("npm install -g '{}'", package),
            ),
            "cargo" => (
                format!(
                    "cargo install --list 2>$null | Select-String -SimpleMatch '{} v' -Quiet",
                    package
                ),
                format!("cargo install '{}'", package),
            ),
            "brew" => (
                format!(
                    "brew list --versions '{}' 2>$null | Select-String -SimpleMatch '{}' -Quiet",
                    package, package
                ),
                format!("brew install '{}'", package),
            ),
            _ => (
                format!("(Get-Command '{}' -ErrorAction SilentlyContinue) -ne $null", package),
                format!("{} install '{}'", manager, package),
            ),
        };

        let command = format!(
            "if ({check_expr}) {{
  Write-Host \"[orion] {package} already installed.\"
}} else {{
  {install_cmd}
}}
if (-not ({check_expr})) {{
  throw \"[orion] Verification failed for {package}.\"
}}"
        );

        ScriptStepInput {
            command,
            explanation: format!("Ensure package '{}' is installed", package),
        }
    }

    fn build_install_script(
        &self,
        platform: ScriptPlatform,
        manager: &str,
        packages: &[String],
    ) -> (String, bool) {
        let requires_admin = Self::manager_requires_admin(manager, platform);
        let mut steps = Vec::<ScriptStepInput>::new();

        match platform {
            ScriptPlatform::Bash => {
                if manager == "apt-get" || manager == "apt" {
                    let update_cmd = if manager == "apt" {
                        "apt update"
                    } else {
                        "apt-get update"
                    };
                    steps.push(ScriptStepInput {
                        explanation: "Refresh package metadata".to_string(),
                        command: update_cmd.to_string(),
                    });
                }
                for package in packages {
                    steps.push(Self::bash_install_step(manager, package));
                }
                (
                    Self::build_bash_script(
                        &format!("Install packages via {}: {}", manager, packages.join(", ")),
                        &steps,
                        requires_admin,
                        &[],
                    ),
                    requires_admin,
                )
            }
            ScriptPlatform::PowerShell => {
                for package in packages {
                    steps.push(Self::powershell_install_step(manager, package));
                }
                (
                    Self::build_powershell_script(
                        &format!("Install packages via {}: {}", manager, packages.join(", ")),
                        &steps,
                        requires_admin,
                        &[],
                    ),
                    requires_admin,
                )
            }
        }
    }

    /// Detect whether a toolbox sidecar URL is configured via TOOLBOX_URL env.
    fn detect_toolbox_url() -> Option<String> {
        env::var("TOOLBOX_URL")
            .ok()
            .filter(|url| !url.trim().is_empty())
    }

    /// Attempt to install packages via the toolbox sidecar HTTP API.
    /// Returns Ok(ToolboxExecResponse) on HTTP success, Err on connection failure.
    async fn toolbox_install(
        toolbox_url: &str,
        packages: &[String],
        manager: &str,
    ) -> Result<ToolboxExecResponse, String> {
        let secret = env::var("TOOLBOX_SECRET").unwrap_or_default();
        let package_list = packages.join(" ");
        let install_cmd = match manager {
            "apt-get" => format!("apt-get update && apt-get install -y {}", package_list),
            "apt" => format!("apt update && apt install -y {}", package_list),
            "pip" | "pip3" => format!("{} install {}", manager, package_list),
            "npm" => format!("npm install -g {}", package_list),
            _ => format!("apt-get update && apt-get install -y {}", package_list),
        };

        let url = format!("{}/exec", toolbox_url);
        let body = serde_json::json!({
            "command": install_cmd,
            "timeout": 180,
        });

        let mut request = reqwest::Client::new()
            .post(&url)
            .json(&body)
            .timeout(Duration::from_secs(190));

        if !secret.is_empty() {
            request = request.header("Authorization", format!("Bearer {}", secret));
        }

        let response = request
            .send()
            .await
            .map_err(|e| format!("Toolbox HTTP request failed: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("Toolbox returned HTTP {}", response.status()));
        }

        response
            .json::<ToolboxExecResponse>()
            .await
            .map_err(|e| format!("Invalid toolbox response: {}", e))
    }

    async fn cooperative_install(
        &self,
        packages_raw: Vec<String>,
        package_manager_raw: Option<String>,
        host_platform_raw: Option<String>,
    ) -> SkillResult<ToolOutput> {
        if packages_raw.is_empty() {
            return Ok(ToolOutput::error(
                "At least one package is required in 'packages'",
            ));
        }

        let mut packages = Vec::<String>::with_capacity(packages_raw.len());
        for package in &packages_raw {
            packages.push(
                Self::validate_package_name(package)
                    .map_err(|e| SkillError::ToolFailed(format!("Invalid package: {}", e)))?,
            );
        }

        let requested_manager = match package_manager_raw {
            Some(ref raw) => {
                Some(Self::normalize_package_manager(raw).map_err(SkillError::ToolFailed)?)
            }
            None => None,
        };
        let host_platform = ScriptPlatform::from_requested(host_platform_raw.as_deref())
            .map_err(SkillError::ToolFailed)?;
        let env_snapshot = self.detect_environment_snapshot();
        let mut direct_install: Option<CommandExecution> = None;
        let mut runtime_manager_used: Option<String> = None;

        let fallback_reason = if !env_snapshot.is_containerized {
            let runtime_manager =
                Self::select_runtime_manager(&env_snapshot, requested_manager.as_deref());
            if let Some(manager) = runtime_manager {
                runtime_manager_used = Some(manager.clone());
                let install_cmd = Self::build_direct_install_command(&manager, &packages)
                    .map_err(SkillError::ToolFailed)?;
                match self.execute_shell_command(&install_cmd).await {
                    Ok(execution) => {
                        direct_install = Some(execution.clone());
                        if execution.success {
                            return Ok(ToolOutput::success(serde_json::json!({
                                "formatted": format!("Installed {} package(s) directly using {}.", packages.len(), manager),
                                "action_taken": "installed_directly",
                                "packages": packages,
                                "package_manager": manager,
                                "environment": env_snapshot,
                                "direct_install": execution,
                            })));
                        }
                        format!(
                            "Direct install command failed (exit code {}).",
                            execution.exit_code
                        )
                    }
                    Err(err) => format!("Direct install execution failed: {}", err),
                }
            } else {
                "No compatible runtime package manager was available for direct install."
                    .to_string()
            }
        } else if let Some(toolbox_url) = Self::detect_toolbox_url() {
            // Containerized but toolbox sidecar is configured — try installing there.
            let toolbox_manager = requested_manager
                .as_deref()
                .unwrap_or("apt-get")
                .to_string();
            match Self::toolbox_install(&toolbox_url, &packages, &toolbox_manager).await {
                Ok(exec) if exec.success => {
                    return Ok(ToolOutput::success(serde_json::json!({
                        "formatted": format!(
                            "Installed {} package(s) in the toolbox sidecar using {}.",
                            packages.len(),
                            toolbox_manager
                        ),
                        "action_taken": "installed_via_toolbox",
                        "packages": packages,
                        "package_manager": toolbox_manager,
                        "environment": env_snapshot,
                        "toolbox_url": toolbox_url,
                        "toolbox_stdout": exec.stdout,
                        "toolbox_stderr": exec.stderr,
                    })));
                }
                Ok(exec) => format!(
                    "Toolbox install failed (exit code {}): {}",
                    exec.exit_code,
                    exec.stderr.chars().take(200).collect::<String>()
                ),
                Err(e) => format!("Toolbox sidecar unavailable: {}", e),
            }
        } else {
            "Orion is running in a containerized environment and cannot modify the host directly."
                .to_string()
        };

        let script_manager = requested_manager
            .clone()
            .unwrap_or_else(|| Self::default_host_manager(host_platform).to_string());
        let (script_content, requires_admin) =
            self.build_install_script(host_platform, &script_manager, &packages);
        let file_label = format!("install-{}", packages.join("-"));
        let script_path = self.persist_script(&file_label, host_platform, &script_content)?;
        let instructions =
            Self::build_mentor_instructions(&script_path, host_platform, &fallback_reason);

        Ok(ToolOutput::success(serde_json::json!({
            "formatted": format!(
                "Generated {} script for mentor-run install because direct execution was not completed.",
                host_platform.as_str()
            ),
            "action_taken": "script_generated",
            "reason": fallback_reason,
            "packages": packages,
            "runtime_package_manager": runtime_manager_used,
            "package_manager": script_manager,
            "host_platform": host_platform.as_str(),
            "requires_admin": requires_admin,
            "script_content": script_content,
            "script_path": script_path.display().to_string(),
            "instructions": instructions,
            "environment": env_snapshot,
            "direct_install": direct_install,
        })))
    }

    fn generate_host_script(
        &self,
        description: String,
        steps: Vec<ScriptStepInput>,
        platform_raw: Option<String>,
        requires_admin: bool,
        idempotent_checks: Vec<String>,
    ) -> SkillResult<ToolOutput> {
        if description.trim().is_empty() {
            return Ok(ToolOutput::error("description must not be empty"));
        }
        if steps.is_empty() {
            return Ok(ToolOutput::error("steps must include at least one entry"));
        }

        for step in &steps {
            if step.command.trim().is_empty() {
                return Ok(ToolOutput::error("Each step.command must be non-empty"));
            }
            if step.explanation.trim().is_empty() {
                return Ok(ToolOutput::error("Each step.explanation must be non-empty"));
            }
            Self::validate_safe_command(&step.command).map_err(SkillError::ToolFailed)?;
        }
        for check in &idempotent_checks {
            Self::validate_safe_command(check).map_err(SkillError::ToolFailed)?;
        }

        let platform = ScriptPlatform::from_requested(platform_raw.as_deref())
            .map_err(SkillError::ToolFailed)?;
        let script_content = match platform {
            ScriptPlatform::Bash => {
                Self::build_bash_script(&description, &steps, requires_admin, &idempotent_checks)
            }
            ScriptPlatform::PowerShell => Self::build_powershell_script(
                &description,
                &steps,
                requires_admin,
                &idempotent_checks,
            ),
        };

        let script_path = self.persist_script(&description, platform, &script_content)?;
        let instructions = Self::build_mentor_instructions(
            &script_path,
            platform,
            "Host-level changes require mentor execution",
        );

        Ok(ToolOutput::success(serde_json::json!({
            "formatted": format!(
                "Generated {} host script for mentor execution.",
                platform.as_str()
            ),
            "action_taken": "script_generated",
            "description": description,
            "host_platform": platform.as_str(),
            "requires_admin": requires_admin,
            "script_content": script_content,
            "script_path": script_path.display().to_string(),
            "instructions": instructions,
        })))
    }
}

#[async_trait]
impl Skill for CooperativeInstallSkill {
    fn manifest(&self) -> &SkillManifest {
        &self.manifest
    }

    async fn initialize(&mut self, _config: SkillConfig) -> SkillResult<()> {
        Ok(())
    }

    async fn shutdown(&mut self) -> SkillResult<()> {
        Ok(())
    }

    fn health(&self) -> SkillHealth {
        SkillHealth {
            status: HealthStatus::Healthy,
            message: None,
            last_check: chrono::Utc::now(),
            metrics: HashMap::new(),
        }
    }

    fn tools(&self) -> Vec<ToolDescriptor> {
        vec![
            ToolDescriptor {
                name: "detect_environment".to_string(),
                description: "Detect runtime environment capabilities (containerization, OS, package managers, writable paths, and network availability).".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
                returns: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "is_containerized": { "type": "boolean" },
                        "os": { "type": "string" },
                        "available_package_managers": { "type": "array" },
                        "writable_paths": { "type": "array" },
                        "has_network": { "type": "boolean" }
                    }
                }),
                cost_estimate: CostEstimate {
                    latency_ms: 150,
                    network_bound: false,
                    token_cost: None,
                },
                required_permissions: vec![],
                autonomous: true,
                requires_confirmation: false,
            },
            ToolDescriptor {
                name: "cooperative_install".to_string(),
                description: "Try package installation directly when possible; if not possible or unsuccessful, generate a mentor-run host script (bash or PowerShell).".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "packages": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Package names to install"
                        },
                        "package_manager": {
                            "type": "string",
                            "description": "Optional explicit package manager override (apt-get, apt, dnf, yum, apk, pacman, zypper, brew, winget, choco, scoop, pip, pip3, npm, cargo)"
                        },
                        "host_platform": {
                            "type": "string",
                            "description": "Optional platform override: bash, powershell, or auto"
                        }
                    },
                    "required": ["packages"]
                }),
                returns: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "action_taken": { "type": "string" },
                        "script_content": { "type": "string" },
                        "script_path": { "type": "string" },
                        "instructions": { "type": "string" }
                    }
                }),
                cost_estimate: CostEstimate {
                    latency_ms: 5000,
                    network_bound: false,
                    token_cost: None,
                },
                required_permissions: vec![
                    Permission::ShellExecute,
                    Permission::FileSystem(FileSystemPermission::Write(vec!["~".to_string()])),
                ],
                autonomous: false,
                requires_confirmation: true,
            },
            ToolDescriptor {
                name: "generate_host_script".to_string(),
                description: "Generate a mentor-run host script from structured steps. Useful for host-level actions Orion cannot apply directly.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "description": { "type": "string" },
                        "steps": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "command": { "type": "string" },
                                    "explanation": { "type": "string" }
                                },
                                "required": ["command", "explanation"]
                            }
                        },
                        "platform": {
                            "type": "string",
                            "description": "bash, powershell, or auto"
                        },
                        "requires_admin": {
                            "type": "boolean",
                            "description": "Whether the script should enforce admin/root execution"
                        },
                        "idempotent_checks": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Optional commands used for preflight and verification checks"
                        }
                    },
                    "required": ["description", "steps"]
                }),
                returns: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "script_content": { "type": "string" },
                        "script_path": { "type": "string" },
                        "instructions": { "type": "string" }
                    }
                }),
                cost_estimate: CostEstimate {
                    latency_ms: 500,
                    network_bound: false,
                    token_cost: None,
                },
                required_permissions: vec![Permission::FileSystem(FileSystemPermission::Write(vec![
                    "~".to_string(),
                ]))],
                autonomous: false,
                requires_confirmation: true,
            },
        ]
    }

    async fn execute_tool(
        &self,
        tool_name: &str,
        params: ToolParams,
        _context: &ExecutionContext,
    ) -> SkillResult<ToolOutput> {
        match tool_name {
            "detect_environment" => {
                let env = self.detect_environment_snapshot();
                let managers = if env.available_package_managers.is_empty() {
                    "none".to_string()
                } else {
                    env.available_package_managers.join(", ")
                };
                Ok(ToolOutput::success(serde_json::json!({
                    "formatted": format!(
                        "Runtime OS: {}; containerized: {}; package managers: {}.",
                        env.os, env.is_containerized, managers
                    ),
                    "is_containerized": env.is_containerized,
                    "os": env.os,
                    "available_package_managers": env.available_package_managers,
                    "writable_paths": env.writable_paths,
                    "has_network": env.has_network,
                    "host_os_hint": env.host_os_hint,
                })))
            }
            "cooperative_install" => {
                let packages: Vec<String> = params.get("packages").ok_or_else(|| {
                    SkillError::ToolFailed("Missing required parameter: packages".to_string())
                })?;
                let package_manager: Option<String> = params.get("package_manager");
                let host_platform: Option<String> = params.get("host_platform");
                self.cooperative_install(packages, package_manager, host_platform)
                    .await
            }
            "generate_host_script" => {
                let description: String = params.get("description").ok_or_else(|| {
                    SkillError::ToolFailed("Missing required parameter: description".to_string())
                })?;
                let steps: Vec<ScriptStepInput> = params.get("steps").ok_or_else(|| {
                    SkillError::ToolFailed("Missing required parameter: steps".to_string())
                })?;
                let platform: Option<String> = params.get("platform");
                let requires_admin: bool = params.get("requires_admin").unwrap_or(false);
                let idempotent_checks: Vec<String> =
                    params.get("idempotent_checks").unwrap_or_default();
                self.generate_host_script(
                    description,
                    steps,
                    platform,
                    requires_admin,
                    idempotent_checks,
                )
            }
            other => Err(SkillError::ToolFailed(format!("Unknown tool: {}", other))),
        }
    }

    fn capabilities(&self) -> Vec<CapabilityDescriptor> {
        vec![]
    }

    fn get_capability(&self, _cap_type: &str) -> Option<&dyn Any> {
        None
    }

    fn triggers(&self) -> Vec<TriggerDescriptor> {
        vec![]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_skill() -> CooperativeInstallSkill {
        CooperativeInstallSkill::new(CooperativeInstallSkill::default_manifest())
    }

    #[test]
    fn test_manifest_parses() {
        let manifest = CooperativeInstallSkill::default_manifest();
        assert_eq!(manifest.name, "Cooperative Install");
        assert_eq!(manifest.id.0, "com.orion.skills.cooperative-install");
    }

    #[test]
    fn test_tools_list_and_flags() {
        let skill = test_skill();
        let tools = skill.tools();
        assert_eq!(tools.len(), 3);

        let detect = tools
            .iter()
            .find(|t| t.name == "detect_environment")
            .unwrap();
        assert!(detect.autonomous);
        assert!(!detect.requires_confirmation);

        let install = tools
            .iter()
            .find(|t| t.name == "cooperative_install")
            .unwrap();
        assert!(!install.autonomous);
        assert!(install.requires_confirmation);

        let generate = tools
            .iter()
            .find(|t| t.name == "generate_host_script")
            .unwrap();
        assert!(!generate.autonomous);
        assert!(generate.requires_confirmation);
    }

    #[test]
    fn test_detect_environment_snapshot_has_core_fields() {
        let skill = test_skill();
        let env = skill.detect_environment_snapshot();

        assert!(!env.os.is_empty());
        assert!(!env.host_os_hint.is_empty());
        assert!(env
            .available_package_managers
            .iter()
            .all(|mgr| !mgr.trim().is_empty()));
    }

    #[test]
    fn test_bash_script_template_structure() {
        let steps = vec![ScriptStepInput {
            command: "echo hello".to_string(),
            explanation: "Print greeting".to_string(),
        }];
        let checks = vec!["command -v echo".to_string()];

        let script =
            CooperativeInstallSkill::build_bash_script("Install helper", &steps, true, &checks);
        assert!(script.contains("#!/usr/bin/env bash"));
        assert!(script.contains("set -euo pipefail"));
        assert!(script.contains("This script requires root privileges"));
        assert!(script.contains("Preflight idempotency checks"));
        assert!(script.contains("Verification checks"));
        assert!(script.contains("Step 1/1: Print greeting"));
    }

    #[test]
    fn test_powershell_script_template_structure() {
        let steps = vec![ScriptStepInput {
            command: "Write-Host 'ok'".to_string(),
            explanation: "Print status".to_string(),
        }];
        let checks = vec!["Get-Command powershell".to_string()];

        let script = CooperativeInstallSkill::build_powershell_script(
            "Install helper",
            &steps,
            true,
            &checks,
        );
        assert!(script.contains("#Requires -RunAsAdministrator"));
        assert!(script.contains("$ErrorActionPreference = 'Stop'"));
        assert!(script.contains("Set-StrictMode -Version Latest"));
        assert!(script.contains("Preflight idempotency checks"));
        assert!(script.contains("Verification checks"));
        assert!(script.contains("Step 1/1: Print status"));
    }

    #[test]
    fn test_safety_check_blocks_dangerous_commands() {
        assert!(CooperativeInstallSkill::validate_safe_command("rm -rf /").is_err());
        assert!(CooperativeInstallSkill::validate_safe_command("shutdown -h now").is_err());
        assert!(CooperativeInstallSkill::validate_safe_command("echo safe").is_ok());
    }

    #[test]
    fn test_bash_install_script_includes_idempotency_and_verification() {
        let skill = test_skill();
        let packages = vec!["curl".to_string()];
        let (script, requires_admin) =
            skill.build_install_script(ScriptPlatform::Bash, "apt-get", &packages);

        assert!(requires_admin);
        assert!(script.contains("apt-get update"));
        assert!(script.contains("dpkg -s \"curl\""));
        assert!(script.contains("apt-get install -y \"curl\""));
        assert!(script.contains("Verification failed for curl"));
    }

    #[test]
    fn test_powershell_install_script_includes_idempotency_and_verification() {
        let skill = test_skill();
        let packages = vec!["Git.Git".to_string()];
        let (script, requires_admin) =
            skill.build_install_script(ScriptPlatform::PowerShell, "winget", &packages);

        assert!(requires_admin);
        assert!(script.contains("winget list --exact --id 'Git.Git'"));
        assert!(script.contains("winget install --id 'Git.Git'"));
        assert!(script.contains("Verification failed for Git.Git"));
    }

    #[test]
    fn test_persist_script_to_explicit_root() {
        let skill = test_skill();
        let root = std::env::temp_dir().join(format!(
            "orion-coop-install-test-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&root).unwrap();

        let path = skill
            .persist_script_with_root(Some(&root), "demo install", ScriptPlatform::Bash, "echo ok")
            .unwrap();
        assert!(path.exists());
        assert_eq!(path.extension().unwrap().to_string_lossy(), "sh");
        let content = std::fs::read_to_string(&path).unwrap();
        assert_eq!(content, "echo ok");

        let _ = std::fs::remove_dir_all(&root);
    }
}
