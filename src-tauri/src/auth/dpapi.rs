use std::fmt;
use zeroize::Zeroizing;

const ENTROPY_LABEL: &[u8] = b"com.vcoblivion.codex-switcher-safe:vault:v1";

/// Sanitized, typed errors for DPAPI operations.
/// Note: Error messages MUST NOT leak plaintext, ciphertext, tokens, pointer values, or raw buffer details.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DpapiError {
    ProtectFailed(u32),
    UnprotectFailed(u32),
    InvalidInput,
    MemoryAllocationFailed,
    UnsupportedPlatform,
}

impl std::error::Error for DpapiError {}

impl fmt::Display for DpapiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DpapiError::ProtectFailed(code) => {
                write!(f, "DPAPI encryption failed with HRESULT 0x{:08X}", code)
            }
            DpapiError::UnprotectFailed(code) => {
                write!(f, "DPAPI decryption failed with HRESULT 0x{:08X}", code)
            }
            DpapiError::InvalidInput => write!(f, "DPAPI received empty or invalid input"),
            DpapiError::MemoryAllocationFailed => write!(f, "DPAPI failed memory allocation"),
            DpapiError::UnsupportedPlatform => {
                write!(f, "DPAPI operation is not supported on this platform")
            }
        }
    }
}

/// Checked conversion of a buffer length to u32 for CRYPT_INTEGER_BLOB.cbData.
/// All length conversions must go through this helper; no direct `len() as u32` is permitted.
fn checked_blob_len(len: usize) -> Result<u32, DpapiError> {
    u32::try_from(len).map_err(|_| DpapiError::InvalidInput)
}

/// RAII Guard owning a Windows-allocated HLOCAL pointer.
/// Calls `LocalFree` exactly once on Drop.
/// When `zero_on_drop` is true (plaintext buffers), the buffer is zeroed before `LocalFree`.
/// Cannot be Clone or Copy.
#[cfg(windows)]
struct LocalFreeGuard {
    ptr: *mut u8,
    len: usize,
    zero_on_drop: bool,
}

#[cfg(windows)]
impl LocalFreeGuard {
    fn new(ptr: *mut u8, len: u32, zero_on_drop: bool) -> Result<Self, DpapiError> {
        if ptr.is_null() {
            return Err(DpapiError::MemoryAllocationFailed);
        }
        let len_usize = usize::try_from(len).map_err(|_| DpapiError::InvalidInput)?;
        Ok(Self {
            ptr,
            len: len_usize,
            zero_on_drop,
        })
    }

    /// Copy the buffer contents to a Vec without logging pointer addresses or buffer contents.
    fn copy_to_vec(&self) -> Vec<u8> {
        // Safety: ptr is non-null and len is validated in new().
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }.to_vec()
    }
}

#[cfg(windows)]
impl Drop for LocalFreeGuard {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            if self.zero_on_drop && self.len > 0 {
                // Zero the plaintext buffer before freeing.
                unsafe {
                    std::ptr::write_bytes(self.ptr, 0u8, self.len);
                }
            }
            use windows::Win32::Foundation::{LocalFree, HLOCAL};
            unsafe {
                let _ = LocalFree(Some(HLOCAL(self.ptr as _)));
            }
        }
    }
}

/// Protects plaintext bytes using Windows DPAPI with CurrentUser scope and explicit entropy.
pub(crate) fn protect(plaintext: &[u8]) -> Result<Vec<u8>, DpapiError> {
    protect_with_entropy(plaintext, Some(ENTROPY_LABEL))
}

/// Unprotects DPAPI ciphertext bytes and returns zeroized plaintext.
pub(crate) fn unprotect(ciphertext: &[u8]) -> Result<Zeroizing<Vec<u8>>, DpapiError> {
    unprotect_with_entropy(ciphertext, Some(ENTROPY_LABEL))
}

#[cfg(windows)]
fn protect_with_entropy(plaintext: &[u8], entropy: Option<&[u8]>) -> Result<Vec<u8>, DpapiError> {
    use windows::core::PCWSTR;
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    if plaintext.is_empty() {
        return Err(DpapiError::InvalidInput);
    }

    let plaintext_len = checked_blob_len(plaintext.len())?;
    let mut data_in = CRYPT_INTEGER_BLOB {
        cbData: plaintext_len,
        pbData: plaintext.as_ptr() as *mut u8,
    };

    let mut entropy_blob = match entropy {
        Some(e) => {
            let e_len = checked_blob_len(e.len())?;
            Some(CRYPT_INTEGER_BLOB {
                cbData: e_len,
                pbData: e.as_ptr() as *mut u8,
            })
        }
        None => None,
    };

    let entropy_ptr = entropy_blob
        .as_mut()
        .map(|b| b as *mut CRYPT_INTEGER_BLOB as *const CRYPT_INTEGER_BLOB);

    let mut data_out = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };

    let flags = CRYPTPROTECT_UI_FORBIDDEN;

    let res = unsafe {
        CryptProtectData(
            &mut data_in,
            PCWSTR::null(),
            entropy_ptr,
            None,
            None,
            flags,
            &mut data_out,
        )
    };

    if let Err(err) = res {
        let code = err.code().0 as u32;
        return Err(DpapiError::ProtectFailed(code));
    }

    // Ciphertext buffer — zero_on_drop = false (not plaintext).
    let guard = LocalFreeGuard::new(data_out.pbData, data_out.cbData, false)?;
    Ok(guard.copy_to_vec())
}

#[cfg(not(windows))]
fn protect_with_entropy(_plaintext: &[u8], _entropy: Option<&[u8]>) -> Result<Vec<u8>, DpapiError> {
    Err(DpapiError::UnsupportedPlatform)
}

#[cfg(windows)]
fn unprotect_with_entropy(
    ciphertext: &[u8],
    entropy: Option<&[u8]>,
) -> Result<Zeroizing<Vec<u8>>, DpapiError> {
    use windows::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    if ciphertext.is_empty() {
        return Err(DpapiError::InvalidInput);
    }

    let ciphertext_len = checked_blob_len(ciphertext.len())?;
    let mut data_in = CRYPT_INTEGER_BLOB {
        cbData: ciphertext_len,
        pbData: ciphertext.as_ptr() as *mut u8,
    };

    let mut entropy_blob = match entropy {
        Some(e) => {
            let e_len = checked_blob_len(e.len())?;
            Some(CRYPT_INTEGER_BLOB {
                cbData: e_len,
                pbData: e.as_ptr() as *mut u8,
            })
        }
        None => None,
    };

    let entropy_ptr = entropy_blob
        .as_mut()
        .map(|b| b as *mut CRYPT_INTEGER_BLOB as *const CRYPT_INTEGER_BLOB);

    let mut data_out = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };

    let flags = CRYPTPROTECT_UI_FORBIDDEN;

    let res = unsafe {
        CryptUnprotectData(
            &mut data_in,
            None,
            entropy_ptr,
            None,
            None,
            flags,
            &mut data_out,
        )
    };

    if let Err(err) = res {
        let code = err.code().0 as u32;
        return Err(DpapiError::UnprotectFailed(code));
    }

    // Plaintext buffer — zero_on_drop = true (must be cleared before LocalFree).
    let guard = LocalFreeGuard::new(data_out.pbData, data_out.cbData, true)?;
    let raw_bytes = guard.copy_to_vec();
    Ok(Zeroizing::new(raw_bytes))
}

#[cfg(not(windows))]
fn unprotect_with_entropy(
    _ciphertext: &[u8],
    _entropy: Option<&[u8]>,
) -> Result<Zeroizing<Vec<u8>>, DpapiError> {
    Err(DpapiError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYNTHETIC_SECRET: &[u8] = b"synthetic-dpapi-secret-A";

    // --- checked_blob_len tests ---

    #[test]
    fn test_checked_blob_len_accepts_zero() {
        assert_eq!(checked_blob_len(0), Ok(0u32));
    }

    #[test]
    fn test_checked_blob_len_accepts_u32_max() {
        // On platforms where usize >= 32 bits this must succeed.
        assert_eq!(checked_blob_len(u32::MAX as usize), Ok(u32::MAX));
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn test_checked_blob_len_rejects_overflow_on_64bit() {
        let overflow = (u32::MAX as usize) + 1;
        assert_eq!(checked_blob_len(overflow), Err(DpapiError::InvalidInput));
    }

    // --- DPAPI error formatting ---

    #[test]
    fn test_dpapi_error_formatting_uses_hresult() {
        let protect_err = DpapiError::ProtectFailed(0x8009_0006u32);
        assert_eq!(
            protect_err.to_string(),
            "DPAPI encryption failed with HRESULT 0x80090006"
        );

        let unprotect_err = DpapiError::UnprotectFailed(0x8009_0006u32);
        assert_eq!(
            unprotect_err.to_string(),
            "DPAPI decryption failed with HRESULT 0x80090006"
        );
    }

    // --- Windows-only DPAPI functional tests ---

    #[test]
    #[cfg(windows)]
    fn test_dpapi_round_trip_succeeds() {
        let protected = protect(SYNTHETIC_SECRET).expect("Protect failed");
        assert_ne!(protected, SYNTHETIC_SECRET);

        let unprotected = unprotect(&protected).expect("Unprotect failed");
        assert_eq!(&*unprotected, SYNTHETIC_SECRET);
    }

    #[test]
    #[cfg(windows)]
    fn test_dpapi_same_plaintext_produces_decryptable_ciphertext() {
        let ciphertext1 = protect(SYNTHETIC_SECRET).expect("Protect 1 failed");
        let ciphertext2 = protect(SYNTHETIC_SECRET).expect("Protect 2 failed");

        let plain1 = unprotect(&ciphertext1).expect("Unprotect 1 failed");
        let plain2 = unprotect(&ciphertext2).expect("Unprotect 2 failed");

        assert_eq!(&*plain1, SYNTHETIC_SECRET);
        assert_eq!(&*plain2, SYNTHETIC_SECRET);
    }

    #[test]
    #[cfg(windows)]
    fn test_dpapi_corrupted_ciphertext_fails_closed() {
        let mut protected = protect(SYNTHETIC_SECRET).expect("Protect failed");
        if !protected.is_empty() {
            protected[0] ^= 0xFF;
        }

        let res = unprotect(&protected);
        assert!(
            res.is_err(),
            "Expected decryption to fail for corrupted ciphertext"
        );
    }

    #[test]
    #[cfg(windows)]
    fn test_dpapi_truncated_ciphertext_fails_closed() {
        let protected = protect(SYNTHETIC_SECRET).expect("Protect failed");
        let truncated = &protected[..protected.len() / 2];

        let res = unprotect(truncated);
        assert!(
            res.is_err(),
            "Expected decryption to fail for truncated ciphertext"
        );
    }

    #[test]
    #[cfg(windows)]
    fn test_dpapi_empty_ciphertext_fails_cleanly() {
        let res = unprotect(&[]);
        assert_eq!(res, Err(DpapiError::InvalidInput));
    }

    #[test]
    #[cfg(windows)]
    fn test_dpapi_error_text_does_not_contain_plaintext() {
        let secret_str = std::str::from_utf8(SYNTHETIC_SECRET).unwrap();
        let res = unprotect(&[0x00, 0x01, 0x02, 0x03]);
        assert!(res.is_err());
        let err = res.unwrap_err();
        let err_msg = err.to_string();

        assert!(!err_msg.contains(secret_str));
    }

    #[test]
    #[cfg(windows)]
    fn test_dpapi_incorrect_entropy_fails() {
        let protected = protect_with_entropy(SYNTHETIC_SECRET, Some(b"correct_entropy")).unwrap();
        let wrong_res = unprotect_with_entropy(&protected, Some(b"wrong_entropy"));

        assert!(
            wrong_res.is_err(),
            "Decryption with incorrect entropy should fail"
        );
    }
}
