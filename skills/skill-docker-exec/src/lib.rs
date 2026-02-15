//! Docker exec skill: dispatch commands to the toolbox sidecar container.
//!
//! The agent's runtime image is intentionally minimal (debian-slim + curl + chromium).
//! When the agent needs tools like git, python, wget, gcc, etc., this skill
//! delegates execution to a pre-built toolbox sidecar container over HTTP.
//!
//! Tools provided:
//!   - `toolbox_exec`    — run a shell command in the toolbox
//!   - `toolbox_install` — install packages in the toolbox via apt/pip/npm
//!   - `toolbox_status`  — check toolbox availability and list installed tools

use async_trait::async_trait;
use orion_skills::{
    CapabilityDescriptor, CostEstimate, ExecutionContext, HealthStatus, NetworkPermission,
    Permission, Skill, SkillConfig, SkillError, SkillHealth, SkillManifest, SkillResult,
    ToolDescriptor, ToolOutput, ToolParams, TriggerDescriptor,
};
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::collections::HashMap;
use std::env;
use std::time::Duration;

/// Default toolbox URL (Docker internal network).
const DEFAULT_TOOLBOX_URL: &str = "http://orion-toolbox:9090";

/// Default request timeout for toolbox HTTP calls.
const DEFAULT_HTTP_TIMEOUT_SECS: u64 = 130; // slightly above the server's 120s max

/// Default command execution timeout passed to the toolbox server.
const DEFAULT_EXEC_TIMEOUT_SECS: u64 = 120;

/// Commands or patterns that are always blocked (mirrors skill-shell).
const BLOCKED_PATTERNS: &[&str] = &[
    "rm -rf /",
    "rm -rf /*",
    "rm -rf ~",
    "mkfs.",
    "dd if=/dev/zero",
    "dd if=/dev/random",
    ":(){:|:&};:", // fork bomb
    "chmod -R 777 /",
    "chown -R",
    "shutdown",
    "reboot",
    "poweroff",
    "halt",
    "init 0",
    "init 6",
    "format c:",
    "> /dev/sda",
    "mv /* /dev/null",
];

/// Commands that require explicit allowlisting (blocked by default).
const ELEVATED_COMMANDS: &[&str] = &["sudo", "su ", "pkill", "kill -9", "killall"];

/// Supported package managers in the toolbox.
const TOOLBOX_PACKAGE_MANAGERS: &[&str] = &["apt-get", "apt", "pip", "pip3", "npm"];

/// Response from the toolbox /exec endpoint.
#[derive(Debug, Clone, Deserialize, Serialize)]
struct ExecResponse {
    success: bool,
    exit_code: i32,
    stdout: String,
    stderr: String,
    stdout_truncated: bool,
    stderr_truncated: bool,
    #[serde(default)]
    blocked: bool,
}

/// Response from the toolbox /tools endpoint.
#[derive(Debug, Clone, Deserialize, Serialize)]
struct ToolsResponse {
    available_tools: Vec<String>,
    package_managers: Vec<String>,
}

/// Docker exec skill — delegates to the toolbox sidecar.
pub struct DockerExecSkill {
    manifest: SkillManifest,
    toolbox_url: String,
    toolbox_secret: String,
}

impl DockerExecSkill {
    /// Parse the embedded skill.toml manifest.
    pub fn default_manifest() -> SkillManifest {
        let toml_str = include_str!("../skill.toml");
        SkillManifest::parse(toml_str).expect("Failed to parse docker-exec skill.toml")
    }

    /// Create a new docker exec skill. Reads TOOLBOX_URL and TOOLBOX_SECRET
    /// from environment, falling back to sensible defaults.
    pub fn new(manifest: SkillManifest) -> Self {
        Self {
            manifest,
            toolbox_url: env::var("TOOLBOX_URL")
                .unwrap_or_else(|_| DEFAULT_TOOLBOX_URL.to_string()),
            toolbox_secret: env::var("TOOLBOX_SECRET").unwrap_or_default(),
        }
    }

    /// Check if a command is blocked by the safety blocklist.
    fn check_safety(command: &str) -> Result<(), String> {
        let lower = command.to_lowercase();

        for pattern in BLOCKED_PATTERNS {
            if lower.contains(pattern) {
                return Err(format!(
                    "Command blocked: contains dangerous pattern '{}'",
                    pattern
                ));
            }
        }

        for pattern in ELEVATED_COMMANDS {
            if lower.starts_with(pattern) || lower.contains(&format!(" {}", pattern)) {
                return Err(format!(
                    "Elevated command '{}' is not allowed in toolbox",
                    pattern.trim()
                ));
            }
        }

        Ok(())
    }

    /// Build an HTTP client with the toolbox bearer token.
    fn http_client(&self) -> Result<reqwest::Client, String> {
        let mut headers = reqwest::header::HeaderMap::new();
        if !self.toolbox_secret.is_empty() {
            let val = reqwest::header::HeaderValue::from_str(&format!(
                "Bearer {}",
                self.toolbox_secret
            ))
            .map_err(|e| format!("Invalid toolbox secret: {}", e))?;
            headers.insert(reqwest::header::AUTHORIZATION, val);
        }
        reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(DEFAULT_HTTP_TIMEOUT_SECS))
            .build()
            .map_err(|e| format!("Failed to build HTTP client: {}", e))
    }

    /// Execute a command in the toolbox via HTTP.
    async fn exec_in_toolbox(
        &self,
        command: &str,
        working_directory: Option<&str>,
        timeout_secs: Option<u64>,
    ) -> SkillResult<ToolOutput> {
        // Client-side safety check first
        if let Err(reason) = Self::check_safety(command) {
            return Ok(ToolOutput::error(reason));
        }

        let client = self.http_client().map_err(SkillError::ToolFailed)?;
        let url = format!("{}/exec", self.toolbox_url);

        let mut body = serde_json::json!({
            "command": command,
            "timeout": timeout_secs.unwrap_or(DEFAULT_EXEC_TIMEOUT_SECS),
        });
        if let Some(dir) = working_directory {
            body["working_directory"] = serde_json::Value::String(dir.to_string());
        }

        let response = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| SkillError::ToolFailed(format!("Toolbox request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response
                .text()
                .await
                .unwrap_or_else(|_| "unknown error".to_string());
            return Err(SkillError::ToolFailed(format!(
                "Toolbox returned HTTP {}: {}",
                status, text
            )));
        }

        let exec: ExecResponse = response
            .json()
            .await
            .map_err(|e| SkillError::ToolFailed(format!("Invalid toolbox response: {}", e)))?;

        let formatted = if exec.success {
            if exec.stdout.is_empty() {
                format!("Command completed successfully (exit code {})", exec.exit_code)
            } else {
                exec.stdout.clone()
            }
        } else {
            let mut msg = format!("Command failed (exit code {})", exec.exit_code);
            if !exec.stderr.is_empty() {
                msg.push_str(&format!("\nStderr: {}", exec.stderr));
            }
            if !exec.stdout.is_empty() {
                msg.push_str(&format!("\nStdout: {}", exec.stdout));
            }
            msg
        };

        Ok(ToolOutput::success(serde_json::json!({
            "formatted": formatted,
            "exit_code": exec.exit_code,
            "success": exec.success,
            "stdout": exec.stdout,
            "stderr": exec.stderr,
            "stdout_truncated": exec.stdout_truncated,
            "stderr_truncated": exec.stderr_truncated,
            "source": "toolbox",
        })))
    }

    /// Install packages in the toolbox via the specified package manager.
    async fn install_in_toolbox(
        &self,
        packages: Vec<String>,
        package_manager: Option<String>,
    ) -> SkillResult<ToolOutput> {
        if packages.is_empty() {
            return Ok(ToolOutput::error("At least one package is required"));
        }

        // Validate package names
        for pkg in &packages {
            let trimmed = pkg.trim();
            if trimmed.is_empty() {
                return Ok(ToolOutput::error("Package names cannot be empty"));
            }
            if !trimmed.chars().all(|c| {
                c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+' | ':' | '/' | '@')
            }) {
                return Ok(ToolOutput::error(format!(
                    "Invalid package name '{}'. Allowed chars: letters, numbers, . - _ + : / @",
                    pkg
                )));
            }
        }

        let manager = package_manager.unwrap_or_else(|| "apt-get".to_string());
        let manager_lower = manager.to_ascii_lowercase();
        if !TOOLBOX_PACKAGE_MANAGERS.contains(&manager_lower.as_str()) {
            return Ok(ToolOutput::error(format!(
                "Unsupported package manager '{}'. Supported: {}",
                manager,
                TOOLBOX_PACKAGE_MANAGERS.join(", ")
            )));
        }

        let package_list = packages.join(" ");
        let install_cmd = match manager_lower.as_str() {
            "apt-get" => format!("apt-get update && apt-get install -y {}", package_list),
            "apt" => format!("apt update && apt install -y {}", package_list),
            "pip" | "pip3" => format!("{} install {}", manager_lower, package_list),
            "npm" => format!("npm install -g {}", package_list),
            _ => {
                return Ok(ToolOutput::error(format!(
                    "Unsupported package manager '{}'",
                    manager
                )));
            }
        };

        let result = self.exec_in_toolbox(&install_cmd, None, Some(180)).await?;

        // Wrap result with install-specific metadata
        if let Some(data) = &result.data {
            let mut enriched = data.clone();
            if let Some(obj) = enriched.as_object_mut() {
                obj.insert(
                    "action".to_string(),
                    serde_json::Value::String("toolbox_install".to_string()),
                );
                obj.insert(
                    "packages".to_string(),
                    serde_json::to_value(&packages).unwrap_or_default(),
                );
                obj.insert(
                    "package_manager".to_string(),
                    serde_json::Value::String(manager_lower),
                );
            }
            return Ok(ToolOutput::success(enriched));
        }

        Ok(result)
    }

    /// Check toolbox availability and list installed tools.
    async fn toolbox_status(&self) -> SkillResult<ToolOutput> {
        let client = self.http_client().map_err(SkillError::ToolFailed)?;

        // Health check
        let health_url = format!("{}/health", self.toolbox_url);
        let health_ok = match client.get(&health_url).send().await {
            Ok(resp) => resp.status().is_success(),
            Err(_) => false,
        };

        if !health_ok {
            return Ok(ToolOutput::success(serde_json::json!({
                "formatted": "Toolbox sidecar is not available.",
                "available": false,
                "toolbox_url": self.toolbox_url,
            })));
        }

        // Tools list
        let tools_url = format!("{}/tools", self.toolbox_url);
        let tools_resp = client
            .get(&tools_url)
            .send()
            .await
            .map_err(|e| SkillError::ToolFailed(format!("Failed to query toolbox: {}", e)))?;

        if !tools_resp.status().is_success() {
            return Ok(ToolOutput::success(serde_json::json!({
                "formatted": "Toolbox is healthy but /tools endpoint failed.",
                "available": true,
                "toolbox_url": self.toolbox_url,
            })));
        }

        let tools: ToolsResponse = tools_resp
            .json()
            .await
            .map_err(|e| SkillError::ToolFailed(format!("Invalid tools response: {}", e)))?;

        let tools_str = if tools.available_tools.is_empty() {
            "none".to_string()
        } else {
            tools.available_tools.join(", ")
        };
        let managers_str = if tools.package_managers.is_empty() {
            "none".to_string()
        } else {
            tools.package_managers.join(", ")
        };

        Ok(ToolOutput::success(serde_json::json!({
            "formatted": format!(
                "Toolbox available. Tools: {}. Package managers: {}.",
                tools_str, managers_str
            ),
            "available": true,
            "toolbox_url": self.toolbox_url,
            "available_tools": tools.available_tools,
            "package_managers": tools.package_managers,
        })))
    }
}

#[async_trait]
impl Skill for DockerExecSkill {
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
                name: "toolbox_exec".to_string(),
                description: "Execute a shell command in the toolbox sidecar container. Use this when the agent needs tools not available in its lean runtime (git, python, wget, gcc, node, etc.).".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "command": {
                            "type": "string",
                            "description": "The shell command to execute in the toolbox container"
                        },
                        "working_directory": {
                            "type": "string",
                            "description": "Optional working directory (default: /workspace/data, the shared volume)"
                        },
                        "timeout_secs": {
                            "type": "integer",
                            "description": "Optional timeout in seconds (default 120, max 300)"
                        }
                    },
                    "required": ["command"]
                }),
                returns: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "formatted": { "type": "string" },
                        "exit_code": { "type": "integer" },
                        "success": { "type": "boolean" },
                        "stdout": { "type": "string" },
                        "stderr": { "type": "string" }
                    }
                }),
                cost_estimate: CostEstimate {
                    latency_ms: 5000,
                    network_bound: true,
                    token_cost: None,
                },
                required_permissions: vec![
                    Permission::ShellExecute,
                    Permission::Network(NetworkPermission::Domains(vec![
                        "orion-toolbox".to_string(),
                    ])),
                ],
                autonomous: false,
                requires_confirmation: true,
            },
            ToolDescriptor {
                name: "toolbox_install".to_string(),
                description: "Install packages in the toolbox sidecar container via apt-get, pip, pip3, or npm. Packages persist in the toolbox for the session.".to_string(),
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
                            "description": "Package manager to use: apt-get (default), apt, pip, pip3, or npm"
                        }
                    },
                    "required": ["packages"]
                }),
                returns: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "formatted": { "type": "string" },
                        "exit_code": { "type": "integer" },
                        "success": { "type": "boolean" },
                        "packages": { "type": "array" },
                        "package_manager": { "type": "string" }
                    }
                }),
                cost_estimate: CostEstimate {
                    latency_ms: 15000,
                    network_bound: true,
                    token_cost: None,
                },
                required_permissions: vec![
                    Permission::ShellExecute,
                    Permission::Network(NetworkPermission::Domains(vec![
                        "orion-toolbox".to_string(),
                    ])),
                ],
                autonomous: false,
                requires_confirmation: true,
            },
            ToolDescriptor {
                name: "toolbox_status".to_string(),
                description: "Check if the toolbox sidecar is available and list installed tools and package managers.".to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
                returns: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "available": { "type": "boolean" },
                        "available_tools": { "type": "array" },
                        "package_managers": { "type": "array" }
                    }
                }),
                cost_estimate: CostEstimate {
                    latency_ms: 500,
                    network_bound: true,
                    token_cost: None,
                },
                required_permissions: vec![Permission::Network(NetworkPermission::Domains(vec![
                    "orion-toolbox".to_string(),
                ]))],
                autonomous: true,
                requires_confirmation: false,
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
            "toolbox_exec" => {
                let command: String = params.get("command").ok_or_else(|| {
                    SkillError::ToolFailed("Missing required parameter: command".to_string())
                })?;
                let working_dir: Option<String> = params.get("working_directory");
                let timeout_secs: Option<u64> = params.get("timeout_secs");
                self.exec_in_toolbox(&command, working_dir.as_deref(), timeout_secs)
                    .await
            }
            "toolbox_install" => {
                let packages: Vec<String> = params.get("packages").ok_or_else(|| {
                    SkillError::ToolFailed("Missing required parameter: packages".to_string())
                })?;
                let package_manager: Option<String> = params.get("package_manager");
                self.install_in_toolbox(packages, package_manager).await
            }
            "toolbox_status" => self.toolbox_status().await,
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

    fn test_skill() -> DockerExecSkill {
        DockerExecSkill::new(DockerExecSkill::default_manifest())
    }

    #[test]
    fn test_manifest_parses() {
        let manifest = DockerExecSkill::default_manifest();
        assert_eq!(manifest.name, "Docker Exec");
        assert_eq!(manifest.id.0, "com.orion.skills.docker-exec");
    }

    #[test]
    fn test_tools_list_and_flags() {
        let skill = test_skill();
        let tools = skill.tools();
        assert_eq!(tools.len(), 3);

        let exec = tools
            .iter()
            .find(|t| t.name == "toolbox_exec")
            .unwrap();
        assert!(!exec.autonomous);
        assert!(exec.requires_confirmation);

        let install = tools
            .iter()
            .find(|t| t.name == "toolbox_install")
            .unwrap();
        assert!(!install.autonomous);
        assert!(install.requires_confirmation);

        let status = tools
            .iter()
            .find(|t| t.name == "toolbox_status")
            .unwrap();
        assert!(status.autonomous);
        assert!(!status.requires_confirmation);
    }

    #[test]
    fn test_safety_blocks_dangerous_commands() {
        assert!(DockerExecSkill::check_safety("rm -rf /").is_err());
        assert!(DockerExecSkill::check_safety("shutdown -h now").is_err());
        assert!(DockerExecSkill::check_safety("sudo apt update").is_err());
        assert!(DockerExecSkill::check_safety(":(){:|:&};:").is_err());
    }

    #[test]
    fn test_safety_allows_normal_commands() {
        assert!(DockerExecSkill::check_safety("ls -la").is_ok());
        assert!(DockerExecSkill::check_safety("git clone https://example.com/repo").is_ok());
        assert!(DockerExecSkill::check_safety("python3 script.py").is_ok());
        assert!(DockerExecSkill::check_safety("wget https://example.com/file").is_ok());
        assert!(DockerExecSkill::check_safety("gcc -o main main.c").is_ok());
        assert!(DockerExecSkill::check_safety("npm install express").is_ok());
    }

    #[test]
    fn test_default_toolbox_url() {
        // When TOOLBOX_URL is not set, should use default
        let skill = test_skill();
        // In test env without TOOLBOX_URL set, it falls back to default
        assert!(!skill.toolbox_url.is_empty());
    }

    #[test]
    fn test_package_name_validation() {
        // Valid names
        let valid = ["curl", "python3-dev", "express", "@types/node", "gcc"];
        for name in valid {
            assert!(
                name.trim()
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric()
                        || matches!(c, '.' | '-' | '_' | '+' | ':' | '/' | '@')),
                "Expected '{}' to be valid",
                name
            );
        }
    }
}
