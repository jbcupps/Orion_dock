//! Transport layer — protocol-level IMAP, SMTP, and service discovery clients.
//!
//! These are **capabilities** (protocol implementations), not agent-facing tools.
//! Skills in `skills/` compose these transports into tool interfaces.

pub mod discovery;
pub mod imap;
pub mod smtp;

pub use imap::{EmailSummary, FolderInfo, ImapClient, SearchCriteria};
pub use orion_core::config::TlsMode;
pub use smtp::SmtpClient;
