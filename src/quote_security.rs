use std::fmt;

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};
use thiserror::Error;

const KEY_BYTES: usize = 32;
const NONCE_BYTES: usize = 12;
const CAPABILITY_BYTES: usize = 32;

#[derive(Clone)]
pub(crate) struct QuoteCipher {
    cipher: Aes256Gcm,
}

impl QuoteCipher {
    pub(crate) fn from_base64(value: &str) -> Result<Self, CryptoError> {
        let key = URL_SAFE_NO_PAD
            .decode(value.trim())
            .map_err(|_| CryptoError::InvalidKey)?;
        if key.len() != KEY_BYTES {
            return Err(CryptoError::InvalidKey);
        }
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| CryptoError::InvalidKey)?;
        Ok(Self { cipher })
    }

    #[cfg(test)]
    pub(crate) fn for_tests() -> Self {
        Self {
            cipher: Aes256Gcm::new_from_slice(&[7_u8; KEY_BYTES]).expect("fixed key is valid"),
        }
    }

    pub(crate) fn encrypt(&self, plaintext: &str, aad: &[u8]) -> Result<String, CryptoError> {
        let mut nonce = [0_u8; NONCE_BYTES];
        getrandom::fill(&mut nonce).map_err(|_| CryptoError::RandomnessUnavailable)?;
        let nonce = Nonce::from(nonce);
        let encrypted = self
            .cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext.as_bytes(),
                    aad,
                },
            )
            .map_err(|_| CryptoError::EncryptionFailed)?;
        let mut envelope = Vec::with_capacity(NONCE_BYTES + encrypted.len());
        envelope.extend_from_slice(&nonce);
        envelope.extend_from_slice(&encrypted);
        Ok(URL_SAFE_NO_PAD.encode(envelope))
    }

    pub(crate) fn decrypt(&self, envelope: &str, aad: &[u8]) -> Result<String, CryptoError> {
        let envelope = URL_SAFE_NO_PAD
            .decode(envelope)
            .map_err(|_| CryptoError::DecryptionFailed)?;
        if envelope.len() <= NONCE_BYTES {
            return Err(CryptoError::DecryptionFailed);
        }
        let (nonce, ciphertext) = envelope.split_at(NONCE_BYTES);
        let nonce: [u8; NONCE_BYTES] = nonce
            .try_into()
            .map_err(|_| CryptoError::DecryptionFailed)?;
        let nonce = Nonce::from(nonce);
        let plaintext = self
            .cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|_| CryptoError::DecryptionFailed)?;
        String::from_utf8(plaintext).map_err(|_| CryptoError::DecryptionFailed)
    }

    pub(crate) fn new_capability(&self, aad: &[u8]) -> Result<AccessCapability, CryptoError> {
        let mut random = [0_u8; CAPABILITY_BYTES];
        getrandom::fill(&mut random).map_err(|_| CryptoError::RandomnessUnavailable)?;
        let token = URL_SAFE_NO_PAD.encode(random);
        let token_digest = capability_digest(&token);
        let token_ciphertext = self.encrypt(&token, aad)?;
        Ok(AccessCapability {
            token_ciphertext,
            token_digest,
        })
    }
}

impl fmt::Debug for QuoteCipher {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("QuoteCipher([redacted])")
    }
}

#[derive(Clone)]
pub(crate) struct AccessCapability {
    pub(crate) token_ciphertext: String,
    pub(crate) token_digest: String,
}

impl fmt::Debug for AccessCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccessCapability")
            .field("token_ciphertext", &"[redacted]")
            .field("token_digest", &"[redacted]")
            .finish()
    }
}

pub(crate) fn capability_digest(token: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(token.as_bytes()))
}

pub(crate) fn is_valid_email(value: &str) -> bool {
    let value = value.trim();
    value.len() <= 320
        && !value.contains(char::is_whitespace)
        && value
            .split_once('@')
            .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'))
}

pub(crate) fn is_valid_e164(value: &str) -> bool {
    let bytes = value.trim().as_bytes();
    (8..=16).contains(&bytes.len())
        && bytes.first() == Some(&b'+')
        && bytes[1..].iter().all(u8::is_ascii_digit)
}

pub(crate) fn mask_email(value: &str) -> String {
    let (local, domain) = value.split_once('@').unwrap_or(("", value));
    let visible = local.chars().next().unwrap_or('*');
    format!("{visible}***@{domain}")
}

pub(crate) fn mask_phone(value: &str) -> String {
    let suffix: String = value
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("+******{suffix}")
}

#[derive(Debug, Error)]
pub(crate) enum CryptoError {
    #[error("quote data could not be decrypted")]
    DecryptionFailed,
    #[error("quote data could not be encrypted")]
    EncryptionFailed,
    #[error("quote data encryption key is invalid")]
    InvalidKey,
    #[error("secure randomness is unavailable")]
    RandomnessUnavailable,
}

#[cfg(test)]
mod tests {
    use super::{
        capability_digest, is_valid_e164, is_valid_email, mask_email, mask_phone, QuoteCipher,
    };

    #[test]
    fn encrypted_values_are_bound_to_their_context() {
        let cipher = QuoteCipher::for_tests();
        let encrypted = cipher
            .encrypt("person@example.invalid", b"contact-a")
            .unwrap();
        assert_eq!(
            cipher.decrypt(&encrypted, b"contact-a").unwrap(),
            "person@example.invalid"
        );
        assert!(cipher.decrypt(&encrypted, b"contact-b").is_err());
        assert!(!format!("{cipher:?}").contains("person"));
    }

    #[test]
    fn capabilities_are_random_hashable_and_redacted() {
        let cipher = QuoteCipher::for_tests();
        let capability = cipher.new_capability(b"quote-link").unwrap();
        let token = cipher
            .decrypt(&capability.token_ciphertext, b"quote-link")
            .unwrap();
        assert_eq!(token.len(), 43);
        assert_eq!(capability.token_digest, capability_digest(&token));
        assert!(!format!("{capability:?}").contains(&token));
    }

    #[test]
    fn contact_validation_and_masking_are_bounded() {
        assert!(is_valid_email("casey@example.invalid"));
        assert!(!is_valid_email("not-an-email"));
        assert!(is_valid_e164("+14155551212"));
        assert!(!is_valid_e164("415-555-1212"));
        assert_eq!(mask_email("casey@example.invalid"), "c***@example.invalid");
        assert_eq!(mask_phone("+14155551212"), "+******1212");
    }
}
