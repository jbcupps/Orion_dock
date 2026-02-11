//! Email provider adapters: Gmail and Outlook (Microsoft Graph) with OAuth2.

pub mod gmail;
pub mod outlook;
pub mod pkce;

pub use gmail::{
    GmailAdapter, GmailMessage, GmailMessageId, OAuth2TokenResponse, SCOPES as GMAIL_SCOPES,
};
pub use outlook::{
    GraphMessage, GraphSendMailRequest, GraphSendMessage, OutlookAdapter, SCOPES as OUTLOOK_SCOPES,
};
pub use pkce::{code_challenge_s256, code_verifier, PkcePair};
