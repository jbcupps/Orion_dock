//! Email auth abstraction: OAuth2, SMTP token, app-password with secure token persistence.
//!
//! **Security:** Access tokens, refresh tokens, and passwords must never be logged or
//! included in error messages. Use redaction or opaque error types when surfacing failures.
//!
//! Tokens are stored in SecretsVault under per-account keys. Key scheme:
//! - `email:{account_id}:refresh_token` — OAuth2 refresh token
//! - `email:{account_id}:access_token` — OAuth2 access token (short-lived)
//! - `email:{account_id}:access_expires_at` — ISO 8601 expiry
//! - `email:{account_id}:password` — app password or SMTP token (opaque)

use crate::config::{EmailAccountConfig, EmailAccountStatus, EmailAuthType};
use crate::secrets::SecretsVault;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const PREFIX: &str = "email:";

fn key_refresh(account_id: &str) -> String {
    format!("{}{}:refresh_token", PREFIX, account_id)
}
fn key_access(account_id: &str) -> String {
    format!("{}{}:access_token", PREFIX, account_id)
}
fn key_access_expires(account_id: &str) -> String {
    format!("{}{}:access_expires_at", PREFIX, account_id)
}
fn key_password(account_id: &str) -> String {
    format!("{}{}:password", PREFIX, account_id)
}

/// All vault key names used for a given email account (for allow-listing).
pub fn vault_keys_for_account(account_id: &str) -> Vec<String> {
    vec![
        key_refresh(account_id),
        key_access(account_id),
        key_access_expires(account_id),
        key_password(account_id),
    ]
}

/// OAuth2 token set for persistence and refresh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuth2Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
}

/// Persist OAuth2 tokens for an account. Caller must call vault.save() after.
pub fn store_oauth_tokens(
    vault: &mut SecretsVault,
    account_id: &str,
    access_token: &str,
    refresh_token: &str,
    expires_at: DateTime<Utc>,
) {
    vault.set_secret(&key_access(account_id), access_token);
    vault.set_secret(&key_refresh(account_id), refresh_token);
    vault.set_secret(&key_access_expires(account_id), &expires_at.to_rfc3339());
}

/// Load OAuth2 tokens for an account. Returns None if any required key is missing.
pub fn get_oauth_tokens(vault: &SecretsVault, account_id: &str) -> Option<OAuth2Tokens> {
    let access = vault.get_secret(&key_access(account_id))?;
    let refresh = vault.get_secret(&key_refresh(account_id))?;
    let expires_str = vault.get_secret(&key_access_expires(account_id))?;
    let expires_at = DateTime::parse_from_rfc3339(expires_str)
        .ok()?
        .with_timezone(&Utc);
    Some(OAuth2Tokens {
        access_token: access.to_string(),
        refresh_token: refresh.to_string(),
        expires_at,
    })
}

/// Store app password or SMTP token. Caller must call vault.save() after.
pub fn store_password(vault: &mut SecretsVault, account_id: &str, password: &str) {
    vault.set_secret(&key_password(account_id), password);
}

/// Get app password or SMTP token.
pub fn get_password(vault: &SecretsVault, account_id: &str) -> Option<String> {
    vault
        .get_secret(&key_password(account_id))
        .map(String::from)
}

/// Remove all stored tokens/password for an account. Caller must call vault.save() after.
pub fn revoke_account(vault: &mut SecretsVault, account_id: &str) {
    vault.remove_secret(&key_refresh(account_id));
    vault.remove_secret(&key_access(account_id));
    vault.remove_secret(&key_access_expires(account_id));
    vault.remove_secret(&key_password(account_id));
}

/// True if access token is still valid (with 60s buffer). Does not check refresh token presence.
pub fn access_token_valid(vault: &SecretsVault, account_id: &str) -> bool {
    let Some(expires_str) = vault.get_secret(&key_access_expires(account_id)) else {
        return false;
    };
    let Ok(expires_at) = DateTime::parse_from_rfc3339(expires_str) else {
        return false;
    };
    let expires_at = expires_at.with_timezone(&Utc);
    Utc::now() < expires_at - chrono::Duration::seconds(60)
}

/// True if we have a refresh token (so we can try to refresh without user).
pub fn has_refresh_token(vault: &SecretsVault, account_id: &str) -> bool {
    vault.get_secret(&key_refresh(account_id)).is_some()
}

/// Whether this account needs interactive reauth (no refresh token or auth type is app-password and password missing).
pub fn needs_reauth(account: &EmailAccountConfig, vault: &SecretsVault) -> bool {
    if account.status != EmailAccountStatus::Active {
        return true;
    }
    match account.auth_type {
        EmailAuthType::OAuth2 => {
            if !has_refresh_token(vault, &account.id) {
                return true;
            }
            !access_token_valid(vault, &account.id)
        }
        EmailAuthType::SmtpToken | EmailAuthType::AppPassword => {
            get_password(vault, &account.id).is_none()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EmailAccountConfig, EmailProvider};

    #[test]
    fn test_vault_keys_for_account() {
        let keys = vault_keys_for_account("acc1");
        assert!(keys.iter().any(|k| k.ends_with("refresh_token")));
        assert!(keys.iter().any(|k| k.ends_with("access_token")));
        assert!(keys.iter().any(|k| k.ends_with("password")));
    }

    #[test]
    fn test_oauth_roundtrip() {
        let tmp = std::env::temp_dir().join("orion_email_auth_test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let mut vault = SecretsVault::new(tmp.clone());
        let expires = Utc::now() + chrono::Duration::hours(1);
        store_oauth_tokens(&mut vault, "gmail1", "access_abc", "refresh_xyz", expires);
        vault.save().unwrap();
        let loaded = SecretsVault::load(tmp.clone()).unwrap();
        let tokens = get_oauth_tokens(&loaded, "gmail1").unwrap();
        assert_eq!(tokens.access_token, "access_abc");
        assert_eq!(tokens.refresh_token, "refresh_xyz");
        assert!(access_token_valid(&loaded, "gmail1"));
        assert!(has_refresh_token(&loaded, "gmail1"));
        let mut vault2 = SecretsVault::load(tmp.clone()).unwrap();
        revoke_account(&mut vault2, "gmail1");
        vault2.save().unwrap();
        let after = SecretsVault::load(tmp.clone()).unwrap();
        assert!(get_oauth_tokens(&after, "gmail1").is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_needs_reauth_oauth_no_refresh() {
        let tmp = std::env::temp_dir().join("orion_email_reauth_test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let vault = SecretsVault::new(tmp.clone());
        let account = EmailAccountConfig {
            id: "acc1".to_string(),
            provider: EmailProvider::Gmail,
            auth_type: EmailAuthType::OAuth2,
            address: "u@gmail.com".to_string(),
            imap_host: None,
            imap_port: None,
            smtp_host: None,
            smtp_port: None,
            scopes_granted: vec![],
            status: EmailAccountStatus::Active,
            last_verified_at: None,
        };
        assert!(needs_reauth(&account, &vault));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_needs_reauth_app_password_missing() {
        let tmp = std::env::temp_dir().join("orion_email_apppw_test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let vault = SecretsVault::new(tmp.clone());
        let account = EmailAccountConfig {
            id: "acc1".to_string(),
            provider: EmailProvider::ImapFallback,
            auth_type: EmailAuthType::AppPassword,
            address: "u@proton.me".to_string(),
            imap_host: Some("mail.proton.me".into()),
            imap_port: Some(993),
            smtp_host: Some("mail.proton.me".into()),
            smtp_port: Some(587),
            scopes_granted: vec![],
            status: EmailAccountStatus::Active,
            last_verified_at: None,
        };
        assert!(needs_reauth(&account, &vault));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_revoke_removes_all_keys() {
        let tmp = std::env::temp_dir().join("orion_email_revoke_test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let tmp_path = tmp.clone();
        let mut vault = SecretsVault::new(tmp);
        store_password(&mut vault, "rev_acc", "secret123");
        vault.save().unwrap();
        assert!(get_password(&vault, "rev_acc").is_some());
        revoke_account(&mut vault, "rev_acc");
        vault.save().unwrap();
        let v2 = SecretsVault::load(tmp_path.clone()).unwrap();
        assert!(get_password(&v2, "rev_acc").is_none());
        let _ = std::fs::remove_dir_all(&tmp_path);
    }
}
