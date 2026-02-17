//! Structured failure types for skill execution.
//!
//! Skills return these instead of opaque error strings so the Execution Governor
//! can make programmatic recovery decisions without LLM parsing.

use serde::{Deserialize, Serialize};

/// Structured failure data returned by skills instead of error strings.
/// Each variant carries actionable information the Governor can use
/// to modify the execution strategy without asking the LLM to parse text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StructuredFailure {
    PermissionDenied {
        path: String,
        /// Paths that ARE writable — populated by the filesystem skill.
        allowed_paths: Vec<String>,
    },
    ResourceNotFound {
        resource: String,
        /// What needs to happen before this resource exists.
        setup_steps: Vec<String>,
    },
    ConnectionFailed {
        host: String,
        port: u16,
        error_detail: String,
        /// Alternative endpoints to try.
        alternatives: Vec<(String, u16)>,
    },
    AuthenticationFailed {
        service: String,
        /// What the user might need to do.
        user_action_required: Option<String>,
    },
    InvalidInput {
        field: String,
        reason: String,
        suggestion: Option<String>,
    },
    Timeout {
        elapsed_ms: u64,
        operation: String,
    },
    /// Catch-all for errors we haven't structured yet.
    /// Every time an Unknown shows up in production logs,
    /// it's a signal to add a proper variant.
    Unknown(String),
}

impl StructuredFailure {
    /// Returns a machine-comparable key for loop detection.
    /// Two failures are "the same" if they have the same kind_key.
    pub fn kind_key(&self) -> String {
        match self {
            Self::PermissionDenied { path, .. } => format!("permission_denied:{}", path),
            Self::ResourceNotFound { resource, .. } => format!("not_found:{}", resource),
            Self::ConnectionFailed { host, port, .. } => {
                format!("conn_failed:{}:{}", host, port)
            }
            Self::AuthenticationFailed { service, .. } => format!("auth_failed:{}", service),
            Self::InvalidInput { field, .. } => format!("invalid_input:{}", field),
            Self::Timeout { operation, .. } => format!("timeout:{}", operation),
            Self::Unknown(msg) => format!("unknown:{}", &msg[..msg.len().min(50)]),
        }
    }

    /// Extracts a Constraint that the Governor adds to execution state.
    pub fn to_constraint(&self) -> Option<Constraint> {
        match self {
            Self::PermissionDenied {
                path,
                allowed_paths,
            } => Some(Constraint::PathBlocked {
                blocked: path.clone(),
                use_instead: allowed_paths.clone(),
            }),
            Self::ConnectionFailed {
                host,
                port,
                alternatives,
                ..
            } => Some(Constraint::HostUnreachable {
                host: format!("{}:{}", host, port),
                try_instead: alternatives.first().map(|(h, p)| format!("{}:{}", h, p)),
            }),
            Self::ResourceNotFound {
                resource,
                setup_steps,
            } => Some(Constraint::ResourceRequiresSetup {
                resource: resource.clone(),
                steps: setup_steps.clone(),
            }),
            Self::AuthenticationFailed {
                service,
                user_action_required,
            } => user_action_required
                .as_ref()
                .map(|action| Constraint::AuthenticationRequired {
                    service: service.clone(),
                    user_action: action.clone(),
                }),
            _ => None,
        }
    }
}

/// A lesson learned from a failed attempt.
/// These accumulate across iterations and are injected into executor prompts
/// so the LLM never has to re-discover a constraint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Constraint {
    PathBlocked {
        blocked: String,
        use_instead: Vec<String>,
    },
    HostUnreachable {
        host: String,
        try_instead: Option<String>,
    },
    ResourceRequiresSetup {
        resource: String,
        steps: Vec<String>,
    },
    ToolUnavailable {
        tool: String,
        reason: String,
    },
    AuthenticationRequired {
        service: String,
        user_action: String,
    },
}

impl std::fmt::Display for Constraint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PathBlocked {
                blocked,
                use_instead,
            } => {
                write!(
                    f,
                    "PATH BLOCKED: '{}' is not writable. Use instead: {:?}",
                    blocked, use_instead
                )
            }
            Self::HostUnreachable { host, try_instead } => {
                write!(f, "HOST UNREACHABLE: '{}'", host)?;
                if let Some(alt) = try_instead {
                    write!(f, ". Try instead: {}", alt)?;
                }
                Ok(())
            }
            Self::ResourceRequiresSetup { resource, steps } => {
                write!(
                    f,
                    "RESOURCE REQUIRES SETUP: '{}'. Steps: {}",
                    resource,
                    steps.join("; ")
                )
            }
            Self::ToolUnavailable { tool, reason } => {
                write!(f, "TOOL UNAVAILABLE: '{}' — {}", tool, reason)
            }
            Self::AuthenticationRequired {
                service,
                user_action,
            } => {
                write!(
                    f,
                    "AUTH REQUIRED for '{}': user must {}",
                    service, user_action
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_key_permission_denied() {
        let f = StructuredFailure::PermissionDenied {
            path: "/app/data/vault/config.json".to_string(),
            allowed_paths: vec!["/app/agent-data".to_string()],
        };
        assert_eq!(
            f.kind_key(),
            "permission_denied:/app/data/vault/config.json"
        );
    }

    #[test]
    fn kind_key_connection_failed() {
        let f = StructuredFailure::ConnectionFailed {
            host: "localhost".to_string(),
            port: 1143,
            error_detail: "refused".to_string(),
            alternatives: vec![("host.docker.internal".to_string(), 1143)],
        };
        assert_eq!(f.kind_key(), "conn_failed:localhost:1143");
    }

    #[test]
    fn kind_key_distinct_for_different_variants() {
        let perm = StructuredFailure::PermissionDenied {
            path: "/tmp/x".to_string(),
            allowed_paths: vec![],
        };
        let not_found = StructuredFailure::ResourceNotFound {
            resource: "/tmp/x".to_string(),
            setup_steps: vec![],
        };
        assert_ne!(perm.kind_key(), not_found.kind_key());
    }

    #[test]
    fn kind_key_unknown_truncates() {
        let long_msg = "a".repeat(200);
        let f = StructuredFailure::Unknown(long_msg);
        let key = f.kind_key();
        assert!(key.len() <= 58); // "unknown:" + 50 chars max
    }

    #[test]
    fn to_constraint_permission_denied() {
        let f = StructuredFailure::PermissionDenied {
            path: "/app/vault".to_string(),
            allowed_paths: vec!["/app/agent-data".to_string(), "/tmp".to_string()],
        };
        let c = f.to_constraint().unwrap();
        match c {
            Constraint::PathBlocked {
                blocked,
                use_instead,
            } => {
                assert_eq!(blocked, "/app/vault");
                assert_eq!(use_instead.len(), 2);
            }
            _ => panic!("Expected PathBlocked"),
        }
    }

    #[test]
    fn to_constraint_connection_failed() {
        let f = StructuredFailure::ConnectionFailed {
            host: "localhost".to_string(),
            port: 993,
            error_detail: "refused".to_string(),
            alternatives: vec![("host.docker.internal".to_string(), 993)],
        };
        let c = f.to_constraint().unwrap();
        match c {
            Constraint::HostUnreachable { host, try_instead } => {
                assert_eq!(host, "localhost:993");
                assert_eq!(try_instead, Some("host.docker.internal:993".to_string()));
            }
            _ => panic!("Expected HostUnreachable"),
        }
    }

    #[test]
    fn to_constraint_resource_not_found() {
        let f = StructuredFailure::ResourceNotFound {
            resource: "email_accounts".to_string(),
            setup_steps: vec!["Write config file".to_string()],
        };
        let c = f.to_constraint().unwrap();
        match c {
            Constraint::ResourceRequiresSetup { resource, steps } => {
                assert_eq!(resource, "email_accounts");
                assert_eq!(steps.len(), 1);
            }
            _ => panic!("Expected ResourceRequiresSetup"),
        }
    }

    #[test]
    fn to_constraint_timeout_returns_none() {
        let f = StructuredFailure::Timeout {
            elapsed_ms: 5000,
            operation: "fetch".to_string(),
        };
        assert!(f.to_constraint().is_none());
    }

    #[test]
    fn to_constraint_auth_with_action() {
        let f = StructuredFailure::AuthenticationFailed {
            service: "proton-bridge".to_string(),
            user_action_required: Some("Regenerate bridge password".to_string()),
        };
        let c = f.to_constraint().unwrap();
        match c {
            Constraint::AuthenticationRequired {
                service,
                user_action,
            } => {
                assert_eq!(service, "proton-bridge");
                assert_eq!(user_action, "Regenerate bridge password");
            }
            _ => panic!("Expected AuthenticationRequired"),
        }
    }

    #[test]
    fn to_constraint_auth_without_action_returns_none() {
        let f = StructuredFailure::AuthenticationFailed {
            service: "gmail".to_string(),
            user_action_required: None,
        };
        assert!(f.to_constraint().is_none());
    }
}
