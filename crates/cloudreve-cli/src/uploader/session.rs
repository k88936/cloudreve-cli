use crate::uploader::providers::PolicyType;
use crate::uploader::ChunkProgress;
use chrono::Utc;
use cloudreve_api::models::explorer::{EncryptMetadata, UploadCredential};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadSession {
    pub id: String,
    pub task_id: String,
    pub drive_id: String,
    pub local_path: String,
    pub remote_uri: String,
    pub file_size: u64,
    pub chunk_size: u64,
    #[serde(with = "policy_type_serde")]
    policy_type: PolicyType,
    credential: UploadCredential,
    relay: bool,
    pub chunk_progress: Vec<ChunkProgress>,
    pub encrypt_metadata: Option<EncryptMetadata>,
    pub expires_at: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

mod policy_type_serde {
    use super::PolicyType;
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(policy_type: &PolicyType, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(policy_type.as_str())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<PolicyType, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(PolicyType::from_str(&s))
    }
}

impl UploadSession {
    pub fn new(
        task_id: String,
        drive_id: String,
        local_path: String,
        remote_uri: String,
        file_size: u64,
        credential: UploadCredential,
    ) -> Self {
        let chunk_size = credential.chunk_size as u64;
        let num_chunks = Self::calculate_num_chunks(file_size, chunk_size);
        let policy_type = credential
            .storage_policy
            .as_ref()
            .map(|p| PolicyType::from_api(&p.policy_type))
            .unwrap_or(PolicyType::Local);

        let now = Utc::now().timestamp();
        let chunk_progress: Vec<ChunkProgress> = (0..num_chunks)
            .map(|i: usize| ChunkProgress::new(i))
            .collect();
        let relay = credential
            .storage_policy
            .as_ref()
            .and_then(|p| p.relay)
            .unwrap_or(false);

        Self {
            id: credential.session_id.clone(),
            task_id,
            drive_id,
            local_path,
            remote_uri,
            file_size,
            chunk_size,
            policy_type,
            encrypt_metadata: credential.encrypt_metadata.clone(),
            chunk_progress,
            expires_at: credential.expires,
            created_at: now,
            updated_at: now,
            credential,
            relay,
        }
    }

    fn calculate_num_chunks(file_size: u64, chunk_size: u64) -> usize {
        if file_size == 0 || chunk_size == 0 {
            return 1;
        }
        ((file_size + chunk_size - 1) / chunk_size) as usize
    }

    pub fn session_id(&self) -> &str {
        &self.credential.session_id
    }

    pub fn credential(&self) -> &UploadCredential {
        &self.credential
    }

    pub fn policy_type(&self) -> PolicyType {
        self.policy_type
    }

    pub fn is_relay(&self) -> bool {
        self.relay
    }

    pub fn is_expired(&self) -> bool {
        Utc::now().timestamp() >= self.expires_at
    }

    pub fn num_chunks(&self) -> usize {
        self.chunk_progress.len()
    }

    pub fn pending_chunks(&self) -> Vec<usize> {
        self.chunk_progress
            .iter()
            .filter(|c| !c.is_complete())
            .map(|c| c.index)
            .collect()
    }

    pub fn all_chunks_complete(&self) -> bool {
        self.chunk_progress.iter().all(|c| c.is_complete())
    }

    pub fn chunk_size_for(&self, chunk_index: usize) -> u64 {
        if self.chunk_size == 0 {
            return self.file_size;
        }
        let start = chunk_index as u64 * self.chunk_size;
        let remaining = self.file_size.saturating_sub(start);
        remaining.min(self.chunk_size)
    }

    pub fn chunk_range(&self, chunk_index: usize) -> (u64, u64) {
        if self.chunk_size == 0 {
            return (0, self.file_size);
        }
        let start = chunk_index as u64 * self.chunk_size;
        let size = self.chunk_size_for(chunk_index);
        (start, start + size)
    }

    pub fn update_chunk_progress(&mut self, chunk_index: usize, loaded: u64, etag: Option<String>) {
        if chunk_index < self.chunk_progress.len() {
            self.chunk_progress[chunk_index].loaded = loaded;
            if let Some(tag) = etag {
                self.chunk_progress[chunk_index].etag = Some(tag);
            }
            self.updated_at = Utc::now().timestamp();
        }
    }

    pub fn complete_chunk(&mut self, chunk_index: usize, etag: Option<String>) {
        if chunk_index < self.chunk_progress.len() {
            let size = self.chunk_size_for(chunk_index);
            self.chunk_progress[chunk_index].loaded = size;
            if let Some(tag) = etag {
                self.chunk_progress[chunk_index].etag = Some(tag);
            }
            self.updated_at = Utc::now().timestamp();
        }
    }

    pub fn total_uploaded(&self) -> u64 {
        self.chunk_progress.iter().map(|c| c.loaded).sum()
    }

    pub fn progress(&self) -> f64 {
        if self.file_size == 0 {
            return 1.0;
        }
        self.total_uploaded() as f64 / self.file_size as f64
    }

    pub fn upload_url_for_chunk(&self, chunk_index: usize) -> Option<&str> {
        self.credential
            .upload_urls
            .as_ref()
            .and_then(|urls| urls.get(chunk_index))
            .map(|s| s.as_str())
    }

    pub fn upload_url(&self) -> Option<&str> {
        self.credential
            .upload_urls
            .as_ref()
            .and_then(|urls| urls.first().map(|s| s.as_str()))
    }

    pub fn complete_url(&self) -> &str {
        self.credential.complete_url.as_deref().unwrap_or_default()
    }

    pub fn callback_secret(&self) -> &str {
        &self.credential.callback_secret
    }

    pub fn credential_string(&self) -> &str {
        &self.credential.credential
    }

    pub fn upload_policy(&self) -> Option<&str> {
        self.credential.upload_policy.as_deref()
    }

    pub fn mime_type(&self) -> Option<&str> {
        self.credential.mime_type.as_deref()
    }

    pub fn is_encrypted(&self) -> bool {
        self.encrypt_metadata.is_some()
    }

    pub fn supports_streaming_encryption(&self) -> bool {
        self.credential
            .storage_policy
            .as_ref()
            .and_then(|p| p.streaming_encryption)
            .unwrap_or(false)
    }

    pub fn chunk_concurrency(&self) -> usize {
        self.credential
            .storage_policy
            .as_ref()
            .and_then(|p| p.chunk_concurrency)
            .map(|c| c.max(1) as usize)
            .unwrap_or(1)
    }
}
