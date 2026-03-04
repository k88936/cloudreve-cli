use crate::uploader::error::{UploadError, UploadResult};
use aes::cipher::{KeyIvInit, StreamCipher};
use aes::Aes256;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use cloudreve_api::models::explorer::EncryptMetadata;
use ctr::Ctr128BE;

type Aes256Ctr = Ctr128BE<Aes256>;

#[derive(Clone)]
pub struct EncryptionConfig {
    key: [u8; 32],
    iv: [u8; 16],
}

impl EncryptionConfig {
    pub fn from_metadata(metadata: &EncryptMetadata) -> UploadResult<Self> {
        let key_bytes = BASE64
            .decode(&metadata.key_plain_text)
            .map_err(|e| UploadError::EncryptionError(format!("Invalid key: {}", e)))?;

        let iv_bytes = BASE64
            .decode(&metadata.iv)
            .map_err(|e| UploadError::EncryptionError(format!("Invalid IV: {}", e)))?;

        if key_bytes.len() != 32 {
            return Err(UploadError::EncryptionError(format!(
                "Invalid key length: expected 32, got {}",
                key_bytes.len()
            )));
        }

        if iv_bytes.len() != 16 {
            return Err(UploadError::EncryptionError(format!(
                "Invalid IV length: expected 16, got {}",
                iv_bytes.len()
            )));
        }

        let mut key = [0u8; 32];
        let mut iv = [0u8; 16];
        key.copy_from_slice(&key_bytes);
        iv.copy_from_slice(&iv_bytes);

        Ok(Self { key, iv })
    }

    fn create_cipher_at_offset(&self, byte_offset: u64) -> Aes256Ctr {
        let block_offset = byte_offset / 16;
        let mut counter = self.iv;
        Self::increment_counter(&mut counter, block_offset);
        Aes256Ctr::new(&self.key.into(), &counter.into())
    }

    fn increment_counter(counter: &mut [u8; 16], blocks: u64) {
        let mut carry = blocks;
        for i in (0..16).rev() {
            if carry == 0 {
                break;
            }
            let sum = counter[i] as u64 + (carry & 0xFF);
            counter[i] = (sum & 0xFF) as u8;
            carry = (carry >> 8) + (sum >> 8);
        }
    }

    pub fn encrypt_at_offset(&self, data: &mut [u8], byte_offset: u64) {
        let mut cipher = self.create_cipher_at_offset(byte_offset);

        let offset_in_block = (byte_offset % 16) as usize;
        if offset_in_block != 0 {
            let first_block_remaining = (16 - offset_in_block).min(data.len());
            let mut temp_block = [0u8; 16];
            temp_block[offset_in_block..offset_in_block + first_block_remaining]
                .copy_from_slice(&data[..first_block_remaining]);
            cipher.apply_keystream(&mut temp_block);
            data[..first_block_remaining].copy_from_slice(
                &temp_block[offset_in_block..offset_in_block + first_block_remaining],
            );

            if data.len() > first_block_remaining {
                cipher.apply_keystream(&mut data[first_block_remaining..]);
            }
        } else {
            cipher.apply_keystream(data);
        }
    }
}
