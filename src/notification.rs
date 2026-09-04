use std::env;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::CONTENT_LENGTH;
use reqwest::{Client, StatusCode};
use serde::Deserialize;
use thiserror::Error;

use crate::quote_security::is_valid_e164;

const MAX_RESPONSE_BYTES: u64 = 65_536;

#[derive(Clone)]
pub struct TwilioMessagingClient {
    account_sid: Arc<str>,
    auth_token: Arc<str>,
    client: Client,
    from: Arc<str>,
}

impl TwilioMessagingClient {
    pub fn from_env() -> Result<Option<Self>, TwilioConfigError> {
        let account_sid = env::var("TWILIO_ACCOUNT_SID").ok();
        let auth_token = env::var("TWILIO_AUTH_TOKEN").ok();
        let from = env::var("TWILIO_MESSAGING_FROM").ok();
        if account_sid.is_none() && auth_token.is_none() && from.is_none() {
            return Ok(None);
        }
        Self::new(
            account_sid.ok_or(TwilioConfigError::Incomplete)?,
            auth_token.ok_or(TwilioConfigError::Incomplete)?,
            from.ok_or(TwilioConfigError::Incomplete)?,
        )
        .map(Some)
    }

    pub fn new(
        account_sid: impl Into<Arc<str>>,
        auth_token: impl Into<Arc<str>>,
        from: impl Into<Arc<str>>,
    ) -> Result<Self, TwilioConfigError> {
        let account_sid = account_sid.into();
        let auth_token = auth_token.into();
        let from = from.into();
        if account_sid.len() != 34
            || !account_sid.starts_with("AC")
            || !account_sid.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return Err(TwilioConfigError::InvalidAccountSid);
        }
        if auth_token.trim().len() < 20 {
            return Err(TwilioConfigError::InvalidAuthToken);
        }
        if !is_valid_e164(&from) {
            return Err(TwilioConfigError::InvalidFromNumber);
        }
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| TwilioConfigError::ClientBuild)?;
        Ok(Self {
            account_sid,
            auth_token,
            client,
            from,
        })
    }

    pub(crate) async fn send_quote_link(
        &self,
        phone: &str,
        link: &str,
    ) -> Result<String, TwilioSendError> {
        if !is_valid_e164(phone) || !link.starts_with("https://") || link.len() > 2_048 {
            return Err(TwilioSendError::InvalidMessage);
        }
        let body = format!(
            "Your Canonical quote is ready to track and edit: {link} This private link expires in 25 days."
        );
        let endpoint = format!(
            "https://api.twilio.com/2010-04-01/Accounts/{}/Messages.json",
            self.account_sid
        );
        let response = self
            .client
            .post(endpoint)
            .basic_auth(self.account_sid.as_ref(), Some(self.auth_token.as_ref()))
            .form(&[
                ("To", phone),
                ("From", self.from.as_ref()),
                ("Body", body.as_str()),
            ])
            .send()
            .await
            .map_err(|_| TwilioSendError::Transport)?;
        let status = response.status();
        if response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .is_some_and(|length| length > MAX_RESPONSE_BYTES)
        {
            return Err(TwilioSendError::InvalidResponse);
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|_| TwilioSendError::Transport)?;
        if bytes.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(TwilioSendError::InvalidResponse);
        }
        if !status.is_success() {
            return Err(TwilioSendError::Rejected(status));
        }
        let response: TwilioMessageResponse =
            serde_json::from_slice(&bytes).map_err(|_| TwilioSendError::InvalidResponse)?;
        if response.sid.len() != 34
            || !response.sid.starts_with('S')
            || !response
                .sid
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric())
        {
            return Err(TwilioSendError::InvalidResponse);
        }
        Ok(response.sid)
    }
}

impl fmt::Debug for TwilioMessagingClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TwilioMessagingClient")
            .field("account_sid", &"[redacted]")
            .field("auth_token", &"[redacted]")
            .field("from", &"[redacted]")
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Error)]
pub enum TwilioConfigError {
    #[error("Twilio HTTP client could not be built")]
    ClientBuild,
    #[error(
        "TWILIO_ACCOUNT_SID, TWILIO_AUTH_TOKEN, and TWILIO_MESSAGING_FROM must be set together"
    )]
    Incomplete,
    #[error("TWILIO_ACCOUNT_SID is invalid")]
    InvalidAccountSid,
    #[error("TWILIO_AUTH_TOKEN is invalid")]
    InvalidAuthToken,
    #[error("TWILIO_MESSAGING_FROM must be an E.164 number")]
    InvalidFromNumber,
}

#[derive(Debug, Error)]
pub(crate) enum TwilioSendError {
    #[error("Twilio returned an invalid response")]
    InvalidResponse,
    #[error("the outbound SMS is invalid")]
    InvalidMessage,
    #[error("Twilio rejected the request")]
    Rejected(StatusCode),
    #[error("Twilio transport failed")]
    Transport,
}

impl TwilioSendError {
    pub(crate) const fn code(&self) -> &'static str {
        match self {
            Self::InvalidMessage => "invalid_message",
            Self::InvalidResponse => "invalid_provider_response",
            Self::Rejected(status) if status.as_u16() == 429 => "provider_rate_limited",
            Self::Rejected(_) => "provider_rejected",
            Self::Transport => "provider_unavailable",
        }
    }
}

#[derive(Deserialize)]
struct TwilioMessageResponse {
    sid: String,
}

#[cfg(test)]
mod tests {
    use super::TwilioMessagingClient;

    #[test]
    fn secrets_are_redacted_from_debug_output() {
        let client = TwilioMessagingClient::new(
            format!("AC{}", "0".repeat(32)),
            "twilio-secret-token-placeholder",
            "+14155551212",
        )
        .unwrap();
        let debug = format!("{client:?}");
        assert!(!debug.contains("twilio-secret"));
        assert!(!debug.contains("+1415"));
    }
}
