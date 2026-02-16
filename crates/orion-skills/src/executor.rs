//! Skill execution engine.
//!
//! Enforces per-call timeouts and global concurrency limits from `ResourceLimits`. File/network
//! I/O must go through capability layers that call `SkillSandbox::check_permission`.
//! Any agent-authored/self-improvement tool path should execute through this engine so trust-tier
//! limits and sandbox filtering cannot be bypassed.

use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use tokio::sync::Semaphore;

use crate::manifest::{NetworkPermission, Permission, SkillId, TrustTier};
use crate::registry::SkillRegistry;
use crate::sandbox::{AuditAction, AuditActionKind, ResourceLimits, SkillSandbox};
use crate::skill::{ExecutionContext, SkillError, SkillResult, ToolOutput, ToolParams};

pub struct SkillExecutor {
    pub registry: Arc<SkillRegistry>,
    /// Limits concurrent tool executions across all skills (from ResourceLimits::max_concurrency).
    concurrency_limiter: Arc<Semaphore>,
    /// Optional override for resource limits (used in tests or custom configurations).
    /// When set, these limits take precedence over tier-based limits.
    limits_override: Option<ResourceLimits>,
}

impl SkillExecutor {
    pub fn new(registry: Arc<SkillRegistry>) -> Self {
        let limits = ResourceLimits::default();
        Self {
            registry,
            concurrency_limiter: Arc::new(Semaphore::new(limits.max_concurrency as usize)),
            limits_override: None,
        }
    }

    /// Build executor with custom resource limits that override tier-based limits
    /// (e.g. for tests with short timeouts).
    pub fn with_limits(registry: Arc<SkillRegistry>, limits: ResourceLimits) -> Self {
        Self {
            registry,
            concurrency_limiter: Arc::new(Semaphore::new(limits.max_concurrency as usize)),
            limits_override: Some(limits),
        }
    }

    /// Build the appropriate audit action for a tool based on its required_permissions (e.g. network domain).
    fn audit_action_for_tool(
        _tool_name: &str,
        required_permissions: &[Permission],
    ) -> Option<AuditAction> {
        for p in required_permissions {
            if let Permission::Network(np) = p {
                let domain = match np {
                    NetworkPermission::Full => "any".to_string(),
                    NetworkPermission::LocalOnly => "localhost".to_string(),
                    NetworkPermission::Domains(domains) => {
                        domains.first().cloned().unwrap_or_else(|| "unknown".into())
                    }
                };
                return Some(AuditAction {
                    kind: AuditActionKind::NetworkRequest { domain },
                });
            }
        }
        None
    }

    /// Check whether a tool requires mentor confirmation before execution.
    pub fn requires_confirmation(&self, skill_id: &SkillId, tool_name: &str) -> bool {
        if let Ok((skill, _, _)) = self.registry.get_skill(skill_id) {
            skill
                .tools()
                .iter()
                .find(|t| t.name == tool_name)
                .map(|t| t.requires_confirmation)
                .unwrap_or(false)
        } else {
            false
        }
    }

    /// Check whether a tool is marked as safe for autonomous execution.
    pub fn is_autonomous(&self, skill_id: &SkillId, tool_name: &str) -> bool {
        if let Ok((skill, _, _)) = self.registry.get_skill(skill_id) {
            skill
                .tools()
                .iter()
                .find(|t| t.name == tool_name)
                .map(|t| t.autonomous)
                .unwrap_or(false)
        } else {
            false
        }
    }

    pub async fn execute(
        &self,
        skill_id: &SkillId,
        tool_name: &str,
        params: ToolParams,
    ) -> SkillResult<ToolOutput> {
        let (skill, manifest, trust_tier) = self.registry.get_skill(skill_id)?;

        let tool = skill
            .tools()
            .into_iter()
            .find(|t| t.name == tool_name)
            .ok_or_else(|| SkillError::ToolFailed(format!("Unknown tool: {}", tool_name)))?;

        // Use explicit override if set, otherwise tier-aware resource limits
        let limits = self
            .limits_override
            .clone()
            .unwrap_or_else(|| ResourceLimits::for_tier(trust_tier));
        let filtered_permissions =
            SkillSandbox::filter_permissions(manifest.permissions.clone(), trust_tier);
        let mut sandbox =
            SkillSandbox::new(manifest.id.clone(), filtered_permissions, limits.clone());
        if let Some(action) = Self::audit_action_for_tool(tool_name, &tool.required_permissions) {
            if !sandbox.check_permission(&action) {
                return Err(SkillError::PermissionDenied(format!(
                    "Tool {} requires permission that is not granted for {} skill",
                    tool_name, trust_tier
                )));
            }
        }

        let _permit = self
            .concurrency_limiter
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| SkillError::ToolFailed("concurrency limiter closed".into()))?;

        let context = ExecutionContext {
            request_id: Uuid::new_v4().to_string(),
            user_id: None,
        };

        // Use the tier-specific timeout
        let timeout_ms = limits.max_cpu_ms;
        let fut = skill.execute_tool(tool_name, params, &context);
        let result = match tokio::time::timeout(Duration::from_millis(timeout_ms), fut).await {
            Ok(Ok(out)) => Ok(out),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(SkillError::ToolFailed(format!(
                "Tool {} exceeded timeout ({} ms)",
                tool_name, timeout_ms
            ))),
        };

        // For Untrusted skills, sanitize output to prevent prompt injection chains
        if trust_tier == TrustTier::Untrusted {
            if let Ok(ref output) = result {
                if let Some(ref data) = output.data {
                    let data_str = data.to_string();
                    if data_str.contains("tool_request") {
                        let sanitized = serde_json::json!({
                            "result": data_str.replace("tool_request", "[filtered]")
                        });
                        return Ok(crate::skill::ToolOutput {
                            success: output.success,
                            data: Some(sanitized),
                            error: output.error.clone(),
                            structured_failure: output.structured_failure.clone(),
                            metadata: output.metadata.clone(),
                        });
                    }
                }
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{NetworkPermission, Permission};
    use crate::skill::{HealthStatus, SkillHealth, ToolDescriptor};
    use std::collections::HashMap;

    /// Skill that sleeps longer than the test timeout so executor returns timeout error.
    struct SleepSkill {
        manifest: crate::manifest::SkillManifest,
        sleep_ms: u64,
    }

    #[async_trait::async_trait]
    impl crate::skill::Skill for SleepSkill {
        fn manifest(&self) -> &crate::manifest::SkillManifest {
            &self.manifest
        }
        async fn initialize(&mut self, _: crate::skill::SkillConfig) -> SkillResult<()> {
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
            vec![ToolDescriptor {
                name: "sleep".to_string(),
                description: "Sleep".to_string(),
                parameters: serde_json::json!({}),
                returns: serde_json::json!({}),
                cost_estimate: crate::skill::CostEstimate::default(),
                required_permissions: vec![],
                autonomous: true,
                requires_confirmation: false,
            }]
        }
        async fn execute_tool(
            &self,
            _: &str,
            _: ToolParams,
            _: &ExecutionContext,
        ) -> SkillResult<crate::skill::ToolOutput> {
            tokio::time::sleep(Duration::from_millis(self.sleep_ms)).await;
            Ok(crate::skill::ToolOutput::success(serde_json::json!({})))
        }
        fn capabilities(&self) -> Vec<crate::manifest::CapabilityDescriptor> {
            vec![]
        }
        fn get_capability(&self, _: &str) -> Option<&dyn std::any::Any> {
            None
        }
        fn triggers(&self) -> Vec<crate::channel::TriggerDescriptor> {
            vec![]
        }
    }

    /// Skill that declares network permission but manifest has no network permission (sandbox denies).
    struct NetworkToolNoPermissionSkill {
        manifest: crate::manifest::SkillManifest,
    }

    #[async_trait::async_trait]
    impl crate::skill::Skill for NetworkToolNoPermissionSkill {
        fn manifest(&self) -> &crate::manifest::SkillManifest {
            &self.manifest
        }
        async fn initialize(&mut self, _: crate::skill::SkillConfig) -> SkillResult<()> {
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
            vec![ToolDescriptor {
                name: "fetch".to_string(),
                description: "Fetch".to_string(),
                parameters: serde_json::json!({}),
                returns: serde_json::json!({}),
                cost_estimate: crate::skill::CostEstimate::default(),
                required_permissions: vec![Permission::Network(NetworkPermission::Full)],
                autonomous: false,
                requires_confirmation: true,
            }]
        }
        async fn execute_tool(
            &self,
            _: &str,
            _: ToolParams,
            _: &ExecutionContext,
        ) -> SkillResult<crate::skill::ToolOutput> {
            Ok(crate::skill::ToolOutput::success(serde_json::json!({})))
        }
        fn capabilities(&self) -> Vec<crate::manifest::CapabilityDescriptor> {
            vec![]
        }
        fn get_capability(&self, _: &str) -> Option<&dyn std::any::Any> {
            None
        }
        fn triggers(&self) -> Vec<crate::channel::TriggerDescriptor> {
            vec![]
        }
    }

    #[tokio::test]
    async fn executor_enforces_timeout() {
        let registry = Arc::new(SkillRegistry::new());
        let skill_id = SkillId("test.sleep".to_string());
        let manifest = crate::manifest::SkillManifest {
            id: skill_id.clone(),
            name: "Sleep".to_string(),
            version: "1.0".to_string(),
            description: "Test".to_string(),
            license: None,
            category: "Test".to_string(),
            keywords: vec![],
            runtime: "Native".to_string(),
            min_orion_version: "0.1.0".to_string(),
            platforms: vec!["All".to_string()],
            capabilities: vec![],
            permissions: vec![],
            secrets: vec![],
            config_defaults: HashMap::new(),
        };
        let skill = SleepSkill {
            manifest,
            sleep_ms: 2000, // well above timeout so CI reliably hits timeout
        };
        registry
            .register(skill_id.clone(), Arc::new(skill))
            .unwrap();
        let limits = ResourceLimits {
            max_cpu_ms: 100, // short so test completes quickly; 2s sleep >> 100ms
            max_concurrency: 2,
            ..ResourceLimits::default()
        };
        let executor = SkillExecutor::with_limits(registry, limits);
        let result = executor
            .execute(&skill_id, "sleep", ToolParams::new())
            .await;
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("exceeded timeout"),
            "expected timeout error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn executor_denies_network_when_not_granted() {
        let registry = Arc::new(SkillRegistry::new());
        let skill_id = SkillId("test.fetch".to_string());
        let manifest = crate::manifest::SkillManifest {
            id: skill_id.clone(),
            name: "Fetch".to_string(),
            version: "1.0".to_string(),
            description: "Test".to_string(),
            license: None,
            category: "Test".to_string(),
            keywords: vec![],
            runtime: "Native".to_string(),
            min_orion_version: "0.1.0".to_string(),
            platforms: vec!["All".to_string()],
            capabilities: vec![],
            permissions: vec![], // no network permission
            secrets: vec![],
            config_defaults: HashMap::new(),
        };
        let skill = NetworkToolNoPermissionSkill { manifest };
        registry
            .register(skill_id.clone(), Arc::new(skill))
            .unwrap();
        let executor = SkillExecutor::new(registry);
        let result = executor
            .execute(&skill_id, "fetch", ToolParams::new())
            .await;
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.to_lowercase().contains("permission") && msg.to_lowercase().contains("denied"),
            "expected permission denied, got: {}",
            err
        );
    }
}
