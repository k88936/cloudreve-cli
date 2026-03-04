use crate::config;
use crate::uploader::{ProgressCallback, ProgressUpdate, Uploader, UploaderConfig, UploadParams};
use anyhow::{Context, Result};
use cloudreve_api::{Client, ClientConfig};
use cloudreve_api::api::user::UserApi;
use indicatif::{ProgressBar, ProgressStyle, MultiProgress};
use std::path::PathBuf;
use std::sync::Arc;
use std::iter;
use walkdir::WalkDir;

pub async fn run(
    local: &PathBuf,
    remote: &str,
    recursive: bool,
    overwrite: bool,
    _concurrency: usize,
) -> Result<()> {
    let config = config::load()?;
    let server_config = config.default.resolve();

    println!("Connecting to {}...", server_config.server);

    let client_config = ClientConfig::new(&server_config.server);
    let client = Arc::new(Client::new(client_config));

    let login_response = client
        .login(&server_config.email, &server_config.password)
        .await
        .context("Login failed")?;

    client.set_tokens(
        login_response.token.access_token.clone(),
        login_response.token.refresh_token.clone(),
    ).await;

    println!("Logged in as: {}", login_response.user.nickname);

    let uploader_config = UploaderConfig::default();
    let uploader = Uploader::new(Arc::clone(&client), uploader_config);

    let normalized_remote = normalize_remote_path(remote);

    let upload_items = create_upload_stream(local, recursive, &normalized_remote);

    let multi_progress = Arc::new(MultiProgress::new());

    let style = ProgressStyle::with_template(
        "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}"
    )?
    .progress_chars("#>-");

    let mut success_count = 0;
    let mut fail_count = 0;

    for item_result in upload_items {
        let (local_path, remote_uri) = item_result?;

        let pb = multi_progress.add(ProgressBar::new(0));
        pb.set_style(style.clone());
        pb.set_message(local_path.file_name().unwrap().to_string_lossy().to_string());

        let progress = ProgressBarCallback { pb: pb.clone() };

        match upload_file(&uploader, &local_path, &remote_uri, overwrite, progress).await {
            Ok(()) => {
                success_count += 1;
                println!("✓ Uploaded: {}", local_path.display());
            }
            Err(e) => {
                fail_count += 1;
                eprintln!("✗ Failed: {} - {}", local_path.display(), e);
            }
        }
    }

    println!("\nUpload complete: {} succeeded, {} failed", success_count, fail_count);

    Ok(())
}

fn normalize_remote_path(remote_path: &str) -> String {
    if remote_path.starts_with("cloudreve://") {
        remote_path.to_string()
    } else if remote_path.starts_with('/') {
        format!("cloudreve://my{}", remote_path)
    } else {
        format!("cloudreve://my/{}", remote_path.trim_start_matches('/'))
    }
}

fn create_upload_stream<'a>(
    local: &'a PathBuf,
    recursive: bool,
    base_remote_path: &'a str,
) -> Box<dyn Iterator<Item = Result<(PathBuf, String)>> + 'a> {
    if local.is_dir() {
        if !recursive {
            Box::new(iter::once(Err(anyhow::anyhow!("{} is a directory, use -r for recursive upload", local.display()))))
        } else {
            Box::new(
                WalkDir::new(local)
                    .into_iter()
                    .filter_map(|e| e.ok())
                    .filter(|entry| entry.path().is_file())
                    .map(move |entry| {
                        let path = entry.path();
                        let relative = path.strip_prefix(local)
                            .context("Failed to compute relative path")?;
                        let remote_uri = format!("{}/{}", base_remote_path.trim_end_matches('/'), relative.to_string_lossy());
                        Ok((path.to_path_buf(), remote_uri))
                    }),
            )
        }
    } else if local.is_file() {
        let file_name = local.file_name()
            .context("File has no name")
            .map(|name| name.to_string_lossy().to_string());
        match file_name {
            Ok(name) => {
                let remote_uri = format!("{}/{}", base_remote_path.trim_end_matches('/'), name);
                Box::new(iter::once(Ok((local.clone(), remote_uri))))
            }
            Err(e) => Box::new(iter::once(Err(e)))
        }
    } else {
        Box::new(iter::once(Err(anyhow::anyhow!("File not found: {}", local.display()))))
    }
}

async fn upload_file(
    uploader: &Uploader,
    local_path: &PathBuf,
    remote_uri: &str,
    overwrite: bool,
    progress: impl ProgressCallback + 'static,
) -> Result<()> {
    let metadata = tokio::fs::metadata(local_path).await?;
    let file_size = metadata.len();
    let last_modified = metadata.modified().ok().and_then(|t| {
        use std::time::UNIX_EPOCH;
        t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
    });

    let params = UploadParams {
        local_path: local_path.clone(),
        remote_uri: remote_uri.to_string(),
        file_size,
        mime_type: guess_mime_type(local_path),
        last_modified,
        overwrite,
        previous_version: String::new(),
    };

    uploader.upload(params, progress).await
}

fn guess_mime_type(path: &PathBuf) -> Option<String> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => Some("image/jpeg".to_string()),
        "png" => Some("image/png".to_string()),
        "gif" => Some("image/gif".to_string()),
        "pdf" => Some("application/pdf".to_string()),
        "txt" => Some("text/plain".to_string()),
        "json" => Some("application/json".to_string()),
        "zip" => Some("application/zip".to_string()),
        "mp4" => Some("video/mp4".to_string()),
        "mp3" => Some("audio/mpeg".to_string()),
        _ => None,
    }
}

struct ProgressBarCallback {
    pb: ProgressBar,
}

impl ProgressCallback for ProgressBarCallback {
    fn on_progress(&self, update: ProgressUpdate) {
        if self.pb.length().unwrap_or(0) == 0 {
            self.pb.set_length(update.total_size);
        }
        self.pb.set_position(update.uploaded);
    }
}
