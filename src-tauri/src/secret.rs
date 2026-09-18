//! The one place a stored secret is encoded or decoded.
//!
//! Every read and every write of a password passes through `seal` and `open`,
//! which is what let encryption land as a change to this file alone.
//!
//! On Windows a secret is sealed with DPAPI (`CryptProtectData`) under the
//! logged-in user's key and stored hex-encoded, with `item_secrets.enc` set to
//! `"dpapi.v1"`. `stash.db` copied to another machine, or read from another
//! Windows account, is useless. A program running as *you* can still call
//! `CryptUnprotectData` on it -- DPAPI does not defend against that, and the
//! settings panel says so.
//!
//! Rows written by an older build carry `enc = "none"` and still decode through
//! the `ENC_NONE` arm, so no schema migration was needed. They are re-sealed in
//! place at startup by `db::queries::upgrade_unsealed_secrets`, and any that
//! survive that pass upgrade the next time they are saved.
//!
//! Other platforms still store plaintext: there is no macOS Keychain or
//! secret-service arm yet, and pretending otherwise in the type would be worse
//! than the honest `ENC_NONE`.

use anyhow::Result;

/// Stored verbatim, no encryption. Written by non-Windows builds, and by every
/// build before DPAPI landed.
pub const ENC_NONE: &str = "none";

/// DPAPI-sealed under the current user, hex-encoded.
#[cfg(windows)]
pub const ENC_DPAPI_V1: &str = "dpapi.v1";

/// Plaintext in, `(stored, enc)` out. The single write-side seam.
///
/// Fallible on purpose. If DPAPI ever refuses, the caller must hear about it:
/// quietly falling back to plaintext would leave a password in the clear while
/// the UI claims it is encrypted, which is the one outcome worth failing a save
/// to avoid.
pub fn seal(plain: &str) -> Result<(String, &'static str)> {
    #[cfg(windows)]
    {
        let blob = win::protect(plain.as_bytes())?;
        Ok((hex::encode(blob), ENC_DPAPI_V1))
    }

    #[cfg(not(windows))]
    {
        Ok((plain.to_string(), ENC_NONE))
    }
}

/// `(stored, enc)` in, plaintext out. The single read-side seam.
///
/// An unrecognised `enc` is an error rather than a fallback to returning the
/// raw column: if a future build wrote ciphertext and this one cannot decrypt
/// it, handing the caller the ciphertext to paste into a login box would be
/// worse than saying so.
pub fn open(stored: &str, enc: &str) -> Result<String> {
    match enc {
        ENC_NONE => Ok(stored.to_string()),

        #[cfg(windows)]
        ENC_DPAPI_V1 => {
            let raw = hex::decode(stored)
                .map_err(|e| anyhow::anyhow!("stored secret is not valid hex: {e}"))?;
            let plain = win::unprotect(&raw)?;
            String::from_utf8(plain)
                .map_err(|_| anyhow::anyhow!("decrypted secret is not valid UTF-8"))
        }

        other => anyhow::bail!("this build cannot read secrets stored as '{other}'"),
    }
}

/// True for a row that is still plaintext and worth upgrading.
pub fn is_unsealed(enc: &str) -> bool {
    enc == ENC_NONE
}

/// True when this build can seal secrets at all. Non-Windows builds cannot, and
/// the settings panel needs to say which one the user is looking at.
pub const fn encryption_available() -> bool {
    cfg!(windows)
}

#[cfg(windows)]
mod win {
    use anyhow::{anyhow, Result};
    use std::ffi::c_void;
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
    };
    use windows::core::PCWSTR;

    /// Shown in the Windows credential-recovery UI if it is ever surfaced.
    const DESCRIPTION: &str = "Stash credential";

    /// Owns a blob DPAPI allocated with `LocalAlloc`. Zeroes the bytes before
    /// freeing them, so a decrypted password does not linger in freed heap, and
    /// frees on every exit path including the error ones.
    struct OutBlob(CRYPT_INTEGER_BLOB);

    impl OutBlob {
        fn to_vec(&self) -> Vec<u8> {
            if self.0.pbData.is_null() {
                return Vec::new();
            }
            unsafe { std::slice::from_raw_parts(self.0.pbData, self.0.cbData as usize) }.to_vec()
        }
    }

    impl Drop for OutBlob {
        fn drop(&mut self) {
            if self.0.pbData.is_null() {
                return;
            }
            unsafe {
                std::ptr::write_bytes(self.0.pbData, 0, self.0.cbData as usize);
                let _ = LocalFree(Some(HLOCAL(self.0.pbData as *mut c_void)));
            }
        }
    }

    /// `pbData` is `*mut` in the API even for the input blob, which is read-only
    /// in practice -- hence the cast off a shared slice.
    fn in_blob(bytes: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: bytes.len() as u32,
            pbData: bytes.as_ptr() as *mut u8,
        }
    }

    pub fn protect(plain: &[u8]) -> Result<Vec<u8>> {
        let input = in_blob(plain);
        let mut out = OutBlob(CRYPT_INTEGER_BLOB::default());

        let desc: Vec<u16> = DESCRIPTION.encode_utf16().chain(std::iter::once(0)).collect();

        unsafe {
            CryptProtectData(
                &input,
                PCWSTR(desc.as_ptr()),
                None, // no extra entropy: a constant baked into the binary buys nothing
                None,
                None,
                0,
                &mut out.0,
            )
        }
        .map_err(|e| anyhow!("CryptProtectData failed: {e}"))?;

        Ok(out.to_vec())
    }

    pub fn unprotect(sealed: &[u8]) -> Result<Vec<u8>> {
        let input = in_blob(sealed);
        let mut out = OutBlob(CRYPT_INTEGER_BLOB::default());

        unsafe {
            CryptUnprotectData(&input, None, None, None, None, 0, &mut out.0)
        }
        .map_err(|e| anyhow!("CryptUnprotectData failed: {e}"))?;

        Ok(out.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rows written before encryption landed must keep decoding, or every
    /// existing credential becomes unreadable the moment this build starts.
    #[test]
    fn still_reads_legacy_plaintext_rows() {
        assert_eq!(open("Hunter2-VerifyMe", ENC_NONE).unwrap(), "Hunter2-VerifyMe");
        assert!(is_unsealed(ENC_NONE));
    }

    #[test]
    fn refuses_unknown_encoding() {
        assert!(open("whatever", "aes-gcm.v9").is_err());
    }

    #[cfg(windows)]
    #[test]
    fn seals_with_dpapi_and_round_trips() {
        let (stored, enc) = seal("Hunter2-VerifyMe").unwrap();
        assert_eq!(enc, ENC_DPAPI_V1);
        // The whole point: the plaintext must not be sitting in the column.
        assert!(!stored.contains("Hunter2"));
        assert!(hex::decode(&stored).is_ok());
        assert!(!is_unsealed(enc));
        assert_eq!(open(&stored, enc).unwrap(), "Hunter2-VerifyMe");
    }

    #[cfg(windows)]
    #[test]
    fn round_trips_unicode_and_empty() {
        for plain in ["", "pä$$ wörd\t🔑", "   ", &"x".repeat(4096)] {
            let (stored, enc) = seal(plain).unwrap();
            assert_eq!(open(&stored, enc).unwrap(), plain);
        }
    }

    #[cfg(windows)]
    #[test]
    fn refuses_corrupt_ciphertext() {
        let (stored, enc) = seal("Hunter2-VerifyMe").unwrap();
        let mut bytes = hex::decode(&stored).unwrap();
        let last = bytes.len() - 1;
        bytes[last] ^= 0xff;
        assert!(open(&hex::encode(bytes), enc).is_err());
        assert!(open("not-hex", enc).is_err());
    }
}
