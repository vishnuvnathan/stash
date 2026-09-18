//! Passphrase-encrypted archive container.
//!
//! This is the one place the project does not hand-roll its primitives. The
//! rest of the tree decodes DIBs and percent-encoding by hand quite happily,
//! but an AEAD or a KDF written here would be the weakest thing in it.
//! Argon2id stretches the passphrase, AES-256-GCM seals the bytes.
//!
//! Why a passphrase and not DPAPI, which already protects the live database:
//! DPAPI keys are tied to the Windows account. An archive sealed with one is
//! unreadable on the machine you are migrating *to*, which is the only reason
//! this file exists.
//!
//! Container layout, all little-endian-free fixed-width fields:
//!
//! ```text
//! magic    8 bytes   "STASHBK1"
//! salt    16 bytes   Argon2id salt, fresh per archive
//! nonce   12 bytes   AES-GCM nonce, fresh per archive
//! body     n bytes   ciphertext, GCM tag included
//! ```
//!
//! The KDF parameters are not stored. They are pinned to `Argon2::default()`
//! for version 1, and a future change of parameters gets a new magic rather
//! than a parameter block -- a version byte an attacker can edit downward is a
//! footgun, and this format has exactly one producer.

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use anyhow::{anyhow, bail, Result};
use rand::RngCore;

const MAGIC: &[u8; 8] = b"STASHBK1";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const KEY_LEN: usize = 32;
pub const HEADER_LEN: usize = MAGIC.len() + SALT_LEN + NONCE_LEN;

/// True if these bytes look like one of our archives. Used to tell an encrypted
/// backup from a plain JSON export without asking the user which it is.
pub fn is_encrypted(bytes: &[u8]) -> bool {
    bytes.len() >= MAGIC.len() && &bytes[..MAGIC.len()] == MAGIC
}

fn derive_key(passphrase: &str, salt: &[u8]) -> Result<[u8; KEY_LEN]> {
    let mut key = [0u8; KEY_LEN];
    argon2::Argon2::default()
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        // The argon2 error type is deliberately vague about why; there is no
        // detail here worth surfacing to a user anyway.
        .map_err(|e| anyhow!("could not derive a key from the passphrase: {e}"))?;
    Ok(key)
}

pub fn seal(plaintext: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    if passphrase.is_empty() {
        bail!("a passphrase is required to encrypt a backup");
    }

    let mut salt = [0u8; SALT_LEN];
    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce_bytes);

    let key = derive_key(passphrase, &salt)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));
    let body = cipher
        .encrypt(Nonce::from_slice(&nonce_bytes), plaintext)
        .map_err(|_| anyhow!("encryption failed"))?;

    let mut out = Vec::with_capacity(HEADER_LEN + body.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&body);
    Ok(out)
}

pub fn open(archive: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    if !is_encrypted(archive) {
        bail!("this file is not a Stash encrypted backup");
    }
    if archive.len() <= HEADER_LEN {
        bail!("this backup is truncated");
    }

    let salt = &archive[MAGIC.len()..MAGIC.len() + SALT_LEN];
    let nonce_bytes = &archive[MAGIC.len() + SALT_LEN..HEADER_LEN];
    let body = &archive[HEADER_LEN..];

    let key = derive_key(passphrase, salt)?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key));

    // GCM cannot distinguish a wrong passphrase from a corrupted file, and
    // guessing would be worse than saying so.
    cipher
        .decrypt(Nonce::from_slice(nonce_bytes), body)
        .map_err(|_| anyhow!("wrong passphrase, or the backup is damaged"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let plain = br#"{"format":"stash-export","items":[]}"#;
        let sealed = seal(plain, "correct horse battery staple").unwrap();
        assert!(is_encrypted(&sealed));
        assert_eq!(open(&sealed, "correct horse battery staple").unwrap(), plain);
    }

    #[test]
    fn the_plaintext_is_not_in_the_archive() {
        let plain = b"Hunter2-VerifyMe and a note about the bank";
        let sealed = seal(plain, "pw").unwrap();
        // The whole point. Searching the bytes for the secret must fail.
        assert!(
            !sealed.windows(plain.len()).any(|w| w == plain),
            "plaintext survived into the archive"
        );
        assert!(!String::from_utf8_lossy(&sealed).contains("Hunter2"));
    }

    #[test]
    fn wrong_passphrase_is_rejected() {
        let sealed = seal(b"secret", "right").unwrap();
        let err = open(&sealed, "wrong").unwrap_err().to_string();
        assert!(err.contains("passphrase"), "unhelpful error: {err}");
    }

    #[test]
    fn tampering_is_detected() {
        let mut sealed = seal(b"secret payload", "pw").unwrap();
        let last = sealed.len() - 1;
        sealed[last] ^= 0x01;
        assert!(open(&sealed, "pw").is_err(), "GCM tag did not catch a flipped bit");
    }

    /// Two archives of the same bytes under the same passphrase must differ,
    /// or the salt and nonce are not actually fresh per archive.
    #[test]
    fn each_archive_is_uniquely_salted() {
        let a = seal(b"same", "pw").unwrap();
        let b = seal(b"same", "pw").unwrap();
        assert_ne!(a, b);
        assert_ne!(a[8..24], b[8..24], "salt repeated");
        assert_ne!(a[24..36], b[24..36], "nonce repeated");
        // Both still open.
        assert_eq!(open(&a, "pw").unwrap(), b"same");
        assert_eq!(open(&b, "pw").unwrap(), b"same");
    }

    #[test]
    fn rejects_foreign_and_truncated_files() {
        assert!(open(b"{\"format\":\"stash-export\"}", "pw").is_err());
        assert!(!is_encrypted(b"{"));
        let sealed = seal(b"x", "pw").unwrap();
        assert!(open(&sealed[..HEADER_LEN], "pw").is_err());
    }

    #[test]
    fn empty_passphrase_is_refused() {
        assert!(seal(b"x", "").is_err());
    }
}
