use anyhow::{Result, bail};
use chacha20poly1305::{
    ChaCha20Poly1305, KeyInit, Nonce,
    aead::rand_core::RngCore,
    aead::{Aead, OsRng},
};
use zeroize::Zeroize;

const KEY_SIZE: usize = 32;
const NONCE_SIZE: usize = 12;

#[derive(Clone)]
pub struct Crypto {
    key: Vec<u8>,
}

impl Drop for Crypto {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

impl Crypto {
    pub fn new(key: &[u8; KEY_SIZE]) -> Self {
        Self { key: key.to_vec() }
    }

    pub fn generate_key() -> [u8; KEY_SIZE] {
        let mut key = [0u8; KEY_SIZE];
        OsRng.fill_bytes(&mut key);
        key
    }

    pub fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        let cipher = ChaCha20Poly1305::new_from_slice(&self.key)
            .map_err(|e| anyhow::anyhow!("Failed to create cipher: {e}"))?;

        let mut nonce_bytes = [0u8; NONCE_SIZE];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|e| anyhow::anyhow!("Encryption failed: {e}"))?;

        let mut result = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
        result.extend_from_slice(&nonce_bytes);
        result.extend_from_slice(&ciphertext);

        Ok(result)
    }

    pub fn decrypt(&self, encrypted_data: &[u8]) -> Result<Vec<u8>> {
        if encrypted_data.len() < NONCE_SIZE {
            bail!("Encrypted data too short");
        }

        let (nonce_bytes, ciphertext) = encrypted_data.split_at(NONCE_SIZE);
        let nonce = Nonce::from_slice(nonce_bytes);

        let cipher = ChaCha20Poly1305::new_from_slice(&self.key)
            .map_err(|e| anyhow::anyhow!("Failed to create cipher: {e}"))?;

        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| anyhow::anyhow!("Decryption failed: {e}"))?;

        Ok(plaintext)
    }
}
