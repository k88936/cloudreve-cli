use crate::config;
use crate::uploader::{ProgressCallback, ProgressUpdate, Uploader, UploaderConfig, UploadParams};
use anyhow::{Context, Result, bail};
use cloudreve_api::{Client, ClientConfig};
use cloudreve_api::api::user::UserApi;
use indicatif::{ProgressBar, ProgressStyle, MultiProgress};
use std::path::PathBuf;
use std::sync::Arc;
use walkdir::WalkDir;

pub async fn run(
    remote_path: &str,
    files: &[PathBuf],
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

    let normalized_remote = normalize_remote_path(remote_path);

    let upload_items = collect_upload_items(files, recursive, &normalized_remote)?;

    if upload_items.is_empty() {
        bail!("No files to upload");
    }

    println!("Uploading {} file(s) to {}...", upload_items.len(), normalized_remote);

    let multi_progress = Arc::new(MultiProgress::new());

    let style = ProgressStyle::with_template(
        "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}"
    )
    .unwrap()
    .progress_chars("#>-");

    let mut tasks = Vec::new();

    for (local_path, remote_uri) in upload_items {
        let local_path_clone = local_path.clone();
        let remote_uri_clone = remote_uri.clone();
        let uploader_ref = &uploader;

        let pb = multi_progress.add(ProgressBar::new(0));
        pb.set_style(style.clone());
        pb.set_message(local_path.file_name().unwrap().to_string_lossy().to_string());

        let progress = ProgressBarCallback { pb: pb.clone() };

        tasks.push(async move {
            upload_file(uploader_ref, &local_path_clone, &remote_uri_clone, overwrite, progress).await
        });
    }

    let mut success_count = 0;
    let mut fail_count = 0;

    for (idx, task) in tasks.into_iter().enumerate() {
        let file_name = files.get(idx)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "?".to_string());
        match task.await {
            Ok(()) => {
                success_count += 1;
                println!("✓ Uploaded: {}", file_name);
            }
            Err(e) => {
                fail_count += 1;
                eprintln!("✗ Failed: {} - {}", file_name, e);
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

fn collect_upload_items(
    files: &[PathBuf],
    recursive: bool,
    base_remote_path: &str,
) -> Result<Vec<(PathBuf, String)>> {
    let mut items = Vec::new();

    for file in files {
        if file.is_dir() {
            if !recursive {
                bail!("{} is a directory, use -r for recursive upload", file.display());
            }

            for entry in WalkDir::new(file).into_iter().filter_map(|e| e.ok()) {
                let path = entry.path();
                if path.is_file() {
                    let relative = path.strip_prefix(file).context("Failed to compute relative path")?;
                    let remote_uri = format!("{}/{}", base_remote_path.trim_end_matches('/'), relative.to_string_lossy());
                    items.push((path.to_path_buf(), remote_uri));
                }
            }
        } else if file.is_file() {
            let file_name = file.file_name().context("File has no name")?;
            let remote_uri = format!("{}/{}", base_remote_path.trim_end_matches('/'), file_name.to_string_lossy());
            items.push((file.clone(), remote_uri));
        } else {
            bail!("File not found: {}", file.display());
        }
    }

    Ok(items)
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
