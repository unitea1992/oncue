use thiserror::Error;

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::{LocalFree, HLOCAL};
#[cfg(target_os = "windows")]
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("ログイン状態の暗号化に失敗しました")]
    EncryptionFailed,
    #[error("保存済みのログイン状態を復号できませんでした（別のWindowsユーザーやPCで保存された可能性があります）")]
    DecryptionFailed,
    #[error("保存済みのログイン状態の形式が正しくありません")]
    InvalidFormat,
    #[cfg(target_os = "windows")]
    #[error("Windowsの暗号化機能でエラーが発生しました: {0}")]
    WindowsError(#[from] windows::core::Error),
    #[cfg(not(target_os = "windows"))]
    #[error("この環境ではログイン状態を暗号化できません")]
    PlatformNotSupported,
}

pub struct SessionCrypto;

impl Default for SessionCrypto {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionCrypto {
    pub fn new() -> Self {
        Self
    }

    pub fn encrypt(&self, _plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        #[cfg(not(target_os = "windows"))]
        {
            let _ = _plaintext;
            Err(CryptoError::PlatformNotSupported)
        }

        #[cfg(target_os = "windows")]
        {
            let plaintext = _plaintext;
            if plaintext.is_empty() {
                return Ok(Vec::new());
            }

            let data_blob_in = CRYPT_INTEGER_BLOB {
                cbData: plaintext.len() as u32,
                pbData: plaintext.as_ptr() as *mut u8,
            };

            let mut data_blob_out = CRYPT_INTEGER_BLOB::default();

            // SAFETY: CryptProtectDataはWindows DPAPIの標準APIです。
            // - data_blob_inは有効な入力データを指しています
            // - data_blob_outはCryptProtectDataによって書き込まれます
            // - pbDataはLocalFreeで解放する必要があります（Windows API契約）
            // - CRYPTPROTECT_UI_FORBIDDENフラグによりUIプロンプトは表示されません
            unsafe {
                let result = CryptProtectData(
                    &data_blob_in,
                    None,
                    None,
                    None,
                    None,
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut data_blob_out,
                );
                if result.is_err() {
                    return Err(CryptoError::EncryptionFailed);
                }

                if data_blob_out.pbData.is_null() {
                    return Err(CryptoError::EncryptionFailed);
                }

                // SAFETY: data_blob_out.pbDataはCryptProtectDataによって割り当てられ、
                // cbDataバイトの有効なデータを指しています
                let encrypted =
                    std::slice::from_raw_parts(data_blob_out.pbData, data_blob_out.cbData as usize)
                        .to_vec();

                // SAFETY: LocalFreeはCryptProtectDataによって割り当てられたメモリを解放します
                let _ = LocalFree(HLOCAL(data_blob_out.pbData as _));

                Ok(encrypted)
            }
        }
    }

    pub fn decrypt(&self, _ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        #[cfg(not(target_os = "windows"))]
        {
            let _ = _ciphertext;
            Err(CryptoError::PlatformNotSupported)
        }

        #[cfg(target_os = "windows")]
        {
            let ciphertext = _ciphertext;
            if ciphertext.is_empty() {
                return Ok(Vec::new());
            }

            let data_blob_in = CRYPT_INTEGER_BLOB {
                cbData: ciphertext.len() as u32,
                pbData: ciphertext.as_ptr() as *mut u8,
            };

            let mut data_blob_out = CRYPT_INTEGER_BLOB::default();

            // SAFETY: CryptUnprotectDataはWindows DPAPIの標準APIです。
            // - data_blob_inは有効な暗号化データを指しています
            // - data_blob_outはCryptUnprotectDataによって書き込まれます
            // - pbDataはLocalFreeで解放する必要があります（Windows API契約）
            unsafe {
                let result = CryptUnprotectData(
                    &data_blob_in,
                    None,
                    None,
                    None,
                    None,
                    0,
                    &mut data_blob_out,
                );
                if result.is_err() {
                    return Err(CryptoError::DecryptionFailed);
                }

                if data_blob_out.pbData.is_null() {
                    return Err(CryptoError::DecryptionFailed);
                }

                // SAFETY: data_blob_out.pbDataはCryptUnprotectDataによって割り当てられ、
                // cbDataバイトの有効なデータを指しています
                let decrypted =
                    std::slice::from_raw_parts(data_blob_out.pbData, data_blob_out.cbData as usize)
                        .to_vec();

                // SAFETY: LocalFreeはCryptUnprotectDataによって割り当てられたメモリを解放します
                let _ = LocalFree(HLOCAL(data_blob_out.pbData as _));

                Ok(decrypted)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "windows")]
    fn test_empty_data() {
        let crypto = SessionCrypto::new();
        let encrypted = crypto.encrypt(b"").unwrap();
        let decrypted = crypto.decrypt(&encrypted).unwrap();
        assert!(decrypted.is_empty());
    }

    // test_empty_data_non_windows は test_platform_not_supported と重複するため削除。

    #[test]
    #[cfg(target_os = "windows")]
    fn test_round_trip() {
        let crypto = SessionCrypto::new();
        let plaintext = b"Hello, VRChat!";

        let encrypted = crypto.encrypt(plaintext).unwrap();
        assert!(!encrypted.is_empty());
        assert_ne!(encrypted, plaintext.to_vec());

        let decrypted = crypto.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, plaintext.to_vec());
    }

    // test_unicode_data は通常 round-trip と別経路を通らないため削除。APIはbyte slice。

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn test_platform_not_supported() {
        let crypto = SessionCrypto::new();
        let result = crypto.encrypt(b"test");
        assert!(matches!(result, Err(CryptoError::PlatformNotSupported)));
    }
}
