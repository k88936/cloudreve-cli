use thiserror::Error;

pub type UploadResult<T> = Result<T, UploadError>;

#[derive(Debug, Error)]
pub enum UploadError {
    #[error("Failed to create upload session: {0}")]
    SessionCreationFailed(String),

    #[error("Failed to upload chunk {0}: {1}")]
    ChunkUploadFailed(usize, String),

    #[error("Failed to complete upload: {0}")]
    CompletionFailed(String),

    #[error("Failed to read file: {0}")]
    FileReadError(String),

    #[error("Failed to delete session: {0}")]
    SessionDeletionFailed(String),

    #[error("Encryption error: {0}")]
    EncryptionError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Request error: {0}")]
    RequestError(String),
}

impl From<reqwest::Error> for UploadError {
    fn from(e: reqwest::Error) -> Self {
        UploadError::RequestError(e.to_string())
    }
}
