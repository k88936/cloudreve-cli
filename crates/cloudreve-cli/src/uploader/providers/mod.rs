mod local;
mod onedrive;
mod qiniu;
mod s3;
mod upyun;

use crate::uploader::chunk::ChunkInfo;
use crate::uploader::session::UploadSession;
use anyhow::Result;
use bytes::Bytes;
use cloudreve_api::Client as CrClient;
use cloudreve_api::models::explorer::PolicyType as ApiPolicyType;
use futures::Stream;
use reqwest::Client as HttpClient;
use std::io;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyType {
    Local,
    Remote,
    Oss,
    Qiniu,
    OneDrive,
    Cos,
    Upyun,
    S3,
    Ks3,
    Obs,
}

impl PolicyType {
    pub fn from_api(api_type: &ApiPolicyType) -> Self {
        match api_type {
            ApiPolicyType::Local => PolicyType::Local,
            ApiPolicyType::Remote => PolicyType::Remote,
            ApiPolicyType::Oss => PolicyType::Oss,
            ApiPolicyType::Qiniu => PolicyType::Qiniu,
            ApiPolicyType::Onedrive => PolicyType::OneDrive,
            ApiPolicyType::Cos => PolicyType::Cos,
            ApiPolicyType::Upyun => PolicyType::Upyun,
            ApiPolicyType::S3 => PolicyType::S3,
            ApiPolicyType::Ks3 => PolicyType::Ks3,
            ApiPolicyType::Obs => PolicyType::Obs,
            ApiPolicyType::LoadBalance => PolicyType::Local,
        }
    }

    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "local" => PolicyType::Local,
            "remote" => PolicyType::Remote,
            "oss" => PolicyType::Oss,
            "qiniu" => PolicyType::Qiniu,
            "onedrive" => PolicyType::OneDrive,
            "cos" => PolicyType::Cos,
            "upyun" => PolicyType::Upyun,
            "s3" => PolicyType::S3,
            "ks3" => PolicyType::Ks3,
            "obs" => PolicyType::Obs,
            _ => PolicyType::Local,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            PolicyType::Local => "local",
            PolicyType::Remote => "remote",
            PolicyType::Oss => "oss",
            PolicyType::Qiniu => "qiniu",
            PolicyType::OneDrive => "onedrive",
            PolicyType::Cos => "cos",
            PolicyType::Upyun => "upyun",
            PolicyType::S3 => "s3",
            PolicyType::Ks3 => "ks3",
            PolicyType::Obs => "obs",
        }
    }

    pub fn is_s3_like(&self) -> bool {
        matches!(
            self,
            PolicyType::Oss | PolicyType::Cos | PolicyType::S3 | PolicyType::Ks3 | PolicyType::Obs
        )
    }

    pub fn requires_callback(&self) -> bool {
        matches!(
            self,
            PolicyType::S3 | PolicyType::Ks3 | PolicyType::Cos | PolicyType::OneDrive
        )
    }

    pub fn uses_per_chunk_urls(&self) -> bool {
        self.is_s3_like()
    }
}

pub async fn upload_chunk_with_progress<S>(
    http_client: &HttpClient,
    cr_client: &Arc<CrClient>,
    policy_type: PolicyType,
    chunk: &ChunkInfo,
    stream: S,
    session: &UploadSession,
) -> Result<Option<String>>
where
    S: Stream<Item = Result<Bytes, io::Error>> + Send + Sync + Unpin + 'static,
{
    match policy_type {
        PolicyType::Local | PolicyType::Remote => {
            local::upload_chunk_generic(http_client, cr_client, chunk, stream, session).await
        }
        PolicyType::Oss => s3::upload_chunk_oss_generic(http_client, chunk, stream, session).await,
        PolicyType::Cos => s3::upload_chunk_cos_generic(http_client, chunk, stream, session).await,
        PolicyType::S3 | PolicyType::Ks3 => {
            s3::upload_chunk_s3_generic(http_client, chunk, stream, session).await
        }
        PolicyType::Obs => s3::upload_chunk_obs_generic(http_client, chunk, stream, session).await,
        PolicyType::OneDrive => {
            onedrive::upload_chunk_generic(http_client, chunk, stream, session).await
        }
        PolicyType::Qiniu => qiniu::upload_chunk_generic(http_client, chunk, stream, session).await,
        PolicyType::Upyun => upyun::upload_chunk_generic(http_client, chunk, stream, session).await,
    }
}

pub async fn complete_upload(
    http_client: &HttpClient,
    cr_client: &Arc<CrClient>,
    session: &UploadSession,
) -> Result<()> {
    let policy_type = if session.is_relay() {
        PolicyType::Local
    } else {
        session.policy_type()
    };

    match policy_type {
        PolicyType::Local | PolicyType::Remote => {
            Ok(())
        }
        PolicyType::Oss => s3::complete_upload_oss(http_client, session).await,
        PolicyType::Cos => {
            s3::complete_upload_s3like(http_client, session).await?;
            s3::callback_s3like(cr_client, session, "cos").await
        }
        PolicyType::S3 => {
            s3::complete_upload_s3like(http_client, session).await?;
            s3::callback_s3like(cr_client, session, "s3").await
        }
        PolicyType::Ks3 => {
            s3::complete_upload_s3like(http_client, session).await?;
            s3::callback_s3like(cr_client, session, "ks3").await
        }
        PolicyType::Obs => s3::complete_upload_obs(http_client, session).await,
        PolicyType::OneDrive => onedrive::complete_upload(cr_client, session).await,
        PolicyType::Qiniu => qiniu::complete_upload(http_client, session).await,
        PolicyType::Upyun => {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok(())
        }
    }
}
