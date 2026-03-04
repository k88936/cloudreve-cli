#[allow(dead_code)]
mod chunk;
#[allow(dead_code)]
mod encrypt;
mod error;
#[allow(dead_code)]
mod progress;
mod providers;
mod session;

use anyhow::{Context, Result};
pub use chunk::{ChunkProgress, ChunkUploader};
pub use error::{UploadError, UploadResult};
pub use progress::{ProgressCallback, ProgressUpdate};
pub use session::UploadSession;

use cloudreve_api::{Client as CrClient, api::ExplorerApi};
use reqwest::Client as HttpClient;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

#[derive(Debug, Clone)]
pub struct UploaderConfig {
    pub max_retries: u32,
    pub retry_base_delay: Duration,
    pub retry_max_delay: Duration,
    pub request_timeout: Duration,
}

impl Default for UploaderConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            retry_base_delay: Duration::from_secs(1),
            retry_max_delay: Duration::from_secs(30),
            request_timeout: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone)]
pub struct UploadParams {
    pub local_path: PathBuf,
    pub remote_uri: String,
    pub file_size: u64,
    pub mime_type: Option<String>,
    pub last_modified: Option<i64>,
    pub overwrite: bool,
    pub previous_version: String,
}

pub struct Uploader {
    cr_client: Arc<CrClient>,
    http_client: HttpClient,
    config: UploaderConfig,
    cancel_token: CancellationToken,
}

impl Uploader {
    pub fn new(cr_client: Arc<CrClient>, config: UploaderConfig) -> Self {
        let http_client = HttpClient::builder()
            .connect_timeout(config.request_timeout)
            .build()
            .expect("Failed to create HTTP client");

        Self {
            cr_client,
            http_client,
            config,
            cancel_token: CancellationToken::new(),
        }
    }

    pub fn with_cancel_token(mut self, token: CancellationToken) -> Self {
        self.cancel_token = token;
        self
    }

    pub async fn upload<P: ProgressCallback + 'static>(
        &self,
        params: UploadParams,
        progress: P,
    ) -> Result<()> {
        info!(
            target: "uploader",
            local_path = %params.local_path.display(),
            remote_uri = %params.remote_uri,
            file_size = params.file_size,
            "Starting upload"
        );

        let session = self.create_session(&params).await?;

        let policy_type = if session.is_relay() {
            providers::PolicyType::Local
        } else {
            session.policy_type()
        };

        let chunk_uploader = ChunkUploader::new(
            self.http_client.clone(),
            self.cr_client.clone(),
            policy_type,
            self.config.clone(),
        );

        let mut session = session;
        let progress = Arc::new(progress);

        let result = chunk_uploader
            .upload_all(&params.local_path, &mut session, progress, &self.cancel_token)
            .await;

        match result {
            Ok(()) => {
                self.complete_upload(&session).await?;
                info!(
                    target: "uploader",
                    local_path = %params.local_path.display(),
                    "Upload completed successfully"
                );
                Ok(())
            }
            Err(e) => {
                error!(
                    target: "uploader",
                    local_path = %params.local_path.display(),
                    error = %e,
                    "Upload failed"
                );
                if let Err(e) = self.delete_remote_session(&session).await {
                    warn!(
                        target: "uploader",
                        local_path = %params.local_path.display(),
                        error = %e,
                        "Failed to delete remote upload session"
                    );
                }
                Err(e)
            }
        }
    }

    pub fn cancel(&self) {
        self.cancel_token.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.is_cancelled()
    }

    async fn create_session(&self, params: &UploadParams) -> Result<UploadSession> {
        use cloudreve_api::models::explorer::UploadSessionRequest;

        let request = UploadSessionRequest {
            uri: params.remote_uri.clone(),
            size: params.file_size as i64,
            policy_id: "".to_string(),
            last_modified: params.last_modified,
            previous: if params.previous_version.is_empty() {
                None
            } else {
                Some(params.previous_version.clone())
            },
            entity_type: if params.overwrite {
                Some("version".to_string())
            } else {
                None
            },
            mime_type: params.mime_type.clone(),
            metadata: None,
            encryption_supported: Some(vec![
                cloudreve_api::models::explorer::EncryptionCipher::Aes256Ctr,
            ]),
        };

        let credential = self
            .cr_client
            .create_upload_session(&request)
            .await
            .context("failed to create upload session")?;

        debug!(
            target: "uploader",
            session_id = %credential.session_id,
            chunk_size = credential.chunk_size,
            "Upload session created"
        );

        let session = UploadSession::new(
            uuid::Uuid::new_v4().to_string(),
            "default".to_string(),
            params.local_path.to_string_lossy().to_string(),
            params.remote_uri.clone(),
            params.file_size,
            credential,
        );

        Ok(session)
    }

    async fn complete_upload(&self, session: &UploadSession) -> Result<()> {
        debug!(
            target: "uploader",
            session_id = %session.session_id(),
            "Completing upload"
        );

        providers::complete_upload(&self.http_client, &self.cr_client, session).await
    }

    pub async fn delete_remote_session(&self, session: &UploadSession) -> Result<()> {
        use cloudreve_api::models::explorer::DeleteUploadSessionService;

        let request = DeleteUploadSessionService {
            id: session.session_id().to_string(),
            uri: session.remote_uri.clone(),
        };

        self.cr_client
            .delete_upload_session(&request)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to delete upload session: {}", e))?;

        Ok(())
    }
}
