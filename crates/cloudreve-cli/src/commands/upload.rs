use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use bytes::Bytes;
use clap::Args;
use cloudreve_api::api::{ExplorerApi, UserApi};
use cloudreve_api::models::explorer::{PolicyType, UploadSessionRequest};
use cloudreve_api::{Client, ClientConfig};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use reqwest::Url;
use walkdir::WalkDir;

use crate::config::{Config, env_config};

#[derive(Debug, Args)]
pub struct UploadCommand {
    #[arg(help = "Local file or directory path to upload")]
    pub source: PathBuf,
    
    #[arg(help = "Remote URI (e.g., cloudreve://my/Documents)")]
    pub destination: String,
    
    #[arg(short = 'r', long, help = "Upload directories recursively")]
    pub recursive: bool,
    
    #[arg(long, help = "Overwrite existing files (create new version)")]
    pub overwrite: bool,
    
    #[arg(short = 'c', long, default_value = "3", help = "Number of concurrent uploads")]
    pub concurrency: usize,
    
    #[arg(long, help = "Read file list from a file (one path per line)")]
    pub batch: Option<PathBuf>,
    
    #[arg(short = 'p', long, help = "Profile name to use")]
    pub profile: Option<String>,
    
    #[arg(long, help = "Continue on errors during batch upload")]
    pub continue_on_error: bool,
}

impl UploadCommand {
    pub async fn execute(self) -> Result<()> {
        let client = init_client(&self.profile)?;
        set_tokens(&client, &self.profile).await?;
        
        if let Some(ref batch_file) = self.batch {
            return self.upload_batch(&client, batch_file).await;
        }
        
        if self.source.is_dir() {
            if !self.recursive {
                anyhow::bail!("Source is a directory. Use --recursive to upload directories.");
            }
            self.upload_directory(&client).await
        } else {
            self.upload_single_file(&client).await
        }
    }
    
    async fn upload_single_file(&self, client: &Client) -> Result<()> {
        let source = &self.source;
        let dest_uri = self.build_remote_uri(source)?;
        
        let file_name = source.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file");
        
        let file_size = source.metadata()
            .context("Failed to read file metadata")?
            .len();
        
        let mp = MultiProgress::new();
        let pb = create_progress_bar(&mp, file_name, file_size);
        
        upload_file(
            client,
            source,
            &dest_uri,
            self.overwrite,
            Some(pb.clone()),
        ).await?;
        
        pb.finish_with_message(format!("✓ {}", file_name));
        
        Ok(())
    }
    
    async fn upload_directory(&self, client: &Client) -> Result<()> {
        let source = &self.source;
        let base_uri = self.destination.trim_end_matches('/').to_string();
        
        let entries: Vec<PathBuf> = WalkDir::new(source)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| !e.file_type().is_dir())
            .map(|e| e.path().to_path_buf())
            .collect();
        
        if entries.is_empty() {
            println!("No files to upload.");
            return Ok(());
        }
        
        println!("Found {} files to upload", entries.len());
        
        let mp = MultiProgress::new();
        let overall_pb = mp.add(ProgressBar::new(entries.len() as u64));
        overall_pb.set_style(
            ProgressStyle::with_template("{msg} [{bar:40}] {pos}/{len}")
                .expect("invalid style")
        );
        overall_pb.set_message("Overall progress");
        
        let mut success_count = 0;
        let mut error_count = 0;
        let mut errors: Vec<(String, String)> = Vec::new();
        
        for entry in entries {
            let relative = entry.strip_prefix(source)
                .context("Failed to get relative path")?;
            let remote_uri = format!("{}/{}", base_uri, relative.to_str().unwrap_or("").replace('\\', "/"));
            
            let file_name = entry.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file");
            
            let file_size = match entry.metadata() {
                Ok(m) => m.len(),
                Err(e) => {
                    if self.continue_on_error {
                        error_count += 1;
                        errors.push((entry.display().to_string(), e.to_string()));
                        overall_pb.inc(1);
                        continue;
                    } else {
                        return Err(e).context(format!("Failed to read metadata for {:?}", entry));
                    }
                }
            };
            
            let pb = create_progress_bar(&mp, file_name, file_size);
            
            match upload_file(client, &entry, &remote_uri, self.overwrite, Some(pb.clone())).await {
                Ok(_) => {
                    success_count += 1;
                    pb.finish_with_message(format!("✓ {}", file_name));
                }
                Err(e) => {
                    error_count += 1;
                    errors.push((entry.display().to_string(), e.to_string()));
                    pb.finish_with_message(format!("✗ {}", file_name));
                    if !self.continue_on_error {
                        return Err(e);
                    }
                }
            }
            
            overall_pb.inc(1);
        }
        
        overall_pb.finish();
        
        println!();
        println!("Upload complete: {} succeeded, {} failed", success_count, error_count);
        
        if !errors.is_empty() {
            println!("\nErrors:");
            for (path, err) in errors {
                println!("  {}: {}", path, err);
            }
        }
        
        if error_count > 0 && !self.continue_on_error {
            anyhow::bail!("Upload failed with {} errors", error_count);
        }
        
        Ok(())
    }
    
    async fn upload_batch(&self, client: &Client, batch_file: &Path) -> Result<()> {
        let content = std::fs::read_to_string(batch_file)
            .context("Failed to read batch file")?;
        
        let files: Vec<PathBuf> = content
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.trim().starts_with('#'))
            .map(|l| PathBuf::from(l.trim()))
            .collect();
        
        if files.is_empty() {
            println!("No files in batch list.");
            return Ok(());
        }
        
        println!("Batch upload: {} files", files.len());
        
        let mp = MultiProgress::new();
        let overall_pb = mp.add(ProgressBar::new(files.len() as u64));
        overall_pb.set_style(
            ProgressStyle::with_template("{msg} [{bar:40}] {pos}/{len}")
                .expect("invalid style")
        );
        overall_pb.set_message("Overall progress");
        
        let mut success_count = 0;
        let mut error_count = 0;
        let mut errors: Vec<(String, String)> = Vec::new();
        
        for file_path in files {
            if !file_path.exists() {
                error_count += 1;
                errors.push((file_path.display().to_string(), "File not found".to_string()));
                overall_pb.inc(1);
                continue;
            }
            
            let remote_uri = self.build_remote_uri(&file_path)?;
            
            let file_name = file_path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("file");
            
            let file_size = file_path.metadata()
                .context("Failed to read file metadata")?
                .len();
            
            let pb = create_progress_bar(&mp, file_name, file_size);
            
            match upload_file(client, &file_path, &remote_uri, self.overwrite, Some(pb.clone())).await {
                Ok(_) => {
                    success_count += 1;
                    pb.finish_with_message(format!("✓ {}", file_name));
                }
                Err(e) => {
                    error_count += 1;
                    errors.push((file_path.display().to_string(), e.to_string()));
                    pb.finish_with_message(format!("✗ {}", file_name));
                    if !self.continue_on_error {
                        return Err(e);
                    }
                }
            }
            
            overall_pb.inc(1);
        }
        
        overall_pb.finish();
        
        println!();
        println!("Batch upload complete: {} succeeded, {} failed", success_count, error_count);
        
        if !errors.is_empty() {
            println!("\nErrors:");
            for (path, err) in errors {
                println!("  {}: {}", path, err);
            }
        }
        
        Ok(())
    }
    
    fn build_remote_uri(&self, local_path: &Path) -> Result<String> {
        let file_name = local_path.file_name()
            .and_then(|n| n.to_str())
            .context("Invalid file name")?;
        
        let dest = self.destination.trim_end_matches('/');
        
        if dest.ends_with(file_name) {
            Ok(dest.to_string())
        } else {
            Ok(format!("{}/{}", dest, file_name))
        }
    }
}

fn create_progress_bar(mp: &MultiProgress, name: &str, size: u64) -> ProgressBar {
    let pb = mp.add(ProgressBar::new(size));
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} {msg} [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
            .expect("invalid style")
            .progress_chars("=>-")
    );
    pb.set_message(truncate_name(name, 30));
    pb
}

fn truncate_name(name: &str, max_len: usize) -> String {
    if name.len() <= max_len {
        name.to_string()
    } else {
        let start = name.len().saturating_sub(max_len - 3);
        format!("...{}", &name[start..])
    }
}

async fn upload_file(
    client: &Client,
    local_path: &Path,
    remote_uri: &str,
    overwrite: bool,
    progress: Option<ProgressBar>,
) -> Result<()> {
    let file_size = local_path.metadata()
        .context("Failed to read file metadata")?
        .len();
    
    let mime_type = mime_guess::from_path(local_path)
        .first()
        .map(|m| m.to_string());
    
    let last_modified = local_path.metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs() as i64);
    
    let policy_id = match client.get_user_storage_policies().await {
        Ok(policies) => policies.first()
            .map(|p| p.id.clone())
            .unwrap_or_default(),
        Err(e) => {
            eprintln!("Warning: Could not fetch storage policies ({}), using default", e);
            String::new()
        }
    };
    
    let request = UploadSessionRequest {
        uri: remote_uri.to_string(),
        size: file_size as i64,
        policy_id,
        last_modified,
        previous: None,
        entity_type: if overwrite { Some("version".to_string()) } else { None },
        mime_type,
        metadata: None,
        encryption_supported: None,
    };
    
    let credential = client.create_upload_session(&request)
        .await
        .context("Failed to create upload session")?;
    
    let storage_policy = credential.storage_policy.as_ref();
    let is_relay = storage_policy.and_then(|p| p.relay).unwrap_or(false);
    let policy_type = storage_policy.map(|p| &p.policy_type).unwrap_or(&PolicyType::Local);
    
    let use_cloudreve_upload = is_relay || matches!(policy_type, PolicyType::Local);
    
    let chunk_size = credential.chunk_size as u64;
    let num_chunks = if chunk_size == 0 { 1 } else { ((file_size + chunk_size - 1) / chunk_size) as usize };
    
    let file = tokio::fs::File::open(local_path).await
        .context("Failed to open file")?;
    
    if use_cloudreve_upload {
        for chunk_idx in 0..num_chunks {
            let offset = chunk_idx as u64 * chunk_size;
            let chunk_len = chunk_size.min(file_size - offset);
            
            let chunk_data = read_chunk(&file, offset, chunk_len).await?;
            
            client.upload_chunk(&credential.session_id, chunk_idx, chunk_data).await
                .context("Chunk upload failed")?;
            
            if let Some(ref pb) = progress {
                pb.inc(chunk_len);
            }
        }
    } else if let Some(ref upload_urls) = credential.upload_urls {
        let is_s3 = matches!(policy_type, PolicyType::S3 | PolicyType::Cos | PolicyType::Oss | PolicyType::Obs);
        let is_onedrive = matches!(policy_type, PolicyType::Onedrive);
        let mut etags: Vec<Option<String>> = vec![None; num_chunks];
        
        for chunk_idx in 0..num_chunks {
            let offset = chunk_idx as u64 * chunk_size;
            let chunk_len = chunk_size.min(file_size - offset);
            
            let chunk_data = read_chunk(&file, offset, chunk_len).await?;
            
            let upload_url = upload_urls.get(chunk_idx)
                .or_else(|| upload_urls.first())
                .context("No upload URL provided for chunk")?;
            
            let response = if is_s3 {
                let mut url = Url::parse(upload_url)?;
                if num_chunks > 1 && upload_urls.len() == 1 {
                    use std::borrow::Cow;
                    let mut query: Vec<(String, String)> = url.query_pairs()
                        .map(|(k, v): (Cow<str>, Cow<str>)| (k.to_string(), v.to_string()))
                        .collect();
                    query.retain(|(k, _)| k != "partNumber");
                    query.push(("partNumber".to_string(), (chunk_idx + 1).to_string()));
                    {
                        let mut query_pairs = url.query_pairs_mut();
                        query_pairs.clear();
                        for (k, v) in query {
                            query_pairs.append_pair(&k, &v);
                        }
                    }
                }
                
                let resp = reqwest::Client::new()
                    .put(url.as_str())
                    .header("Content-Type", "application/octet-stream")
                    .header("Content-Length", chunk_len)
                    .body(chunk_data)
                    .send()
                    .await
                    .context("Failed to upload chunk to S3 storage")?;
                
                let etag = resp.headers()
                    .get("ETag")
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());
                etags[chunk_idx] = etag;
                
                resp
            } else {
                let mut req = reqwest::Client::new()
                    .post(upload_url)
                    .query(&[("chunk", chunk_idx)])
                    .header("Content-Type", "application/octet-stream")
                    .header("Content-Length", chunk_len)
                    .body(chunk_data);
                
                if !credential.credential.is_empty() {
                    req = req.header("Authorization", &credential.credential);
                }
                
                req.send().await.context("Failed to upload chunk to remote storage")?
            };
            
            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                anyhow::bail!("Remote upload failed ({}): {}", status, body);
            }
            
            if let Some(ref pb) = progress {
                pb.inc(chunk_len);
            }
        }
        
        if is_s3 && !credential.callback_secret.is_empty() {
            if let Some(ref complete_url) = credential.complete_url {
                if !complete_url.is_empty() {
                    let mut parts_xml = String::new();
                    for (idx, etag) in etags.iter().enumerate() {
                        if let Some(tag) = etag {
                            parts_xml.push_str(&format!(
                                "<Part><PartNumber>{}</PartNumber><ETag>{}</ETag></Part>",
                                idx + 1,
                                tag
                            ));
                        }
                    }
                    let complete_body = format!(
                        r#"<?xml version="1.0" encoding="UTF-8"?><CompleteMultipartUpload>{}</CompleteMultipartUpload>"#,
                        parts_xml
                    );
                    
                    let response = reqwest::Client::new()
                        .post(complete_url)
                        .header("Content-Type", "application/xml")
                        .body(complete_body)
                        .send()
                        .await
                        .context("Failed to complete S3 multipart upload")?;
                    
                    if !response.status().is_success() {
                        let status = response.status();
                        let body = response.text().await.unwrap_or_default();
                        eprintln!("Warning: S3 complete multipart upload returned {}: {}", status, body);
                    }
                }
            }
            
            let policy_type_str = match policy_type {
                PolicyType::S3 => "s3",
                PolicyType::Cos => "cos",
                PolicyType::Oss => "oss",
                PolicyType::Obs => "obs",
                _ => "s3",
            };
            
            client.complete_s3_upload(policy_type_str, &credential.session_id, &credential.callback_secret)
                .await
                .context("Failed to complete upload callback")?;
        } else if is_onedrive && !credential.callback_secret.is_empty() {
            client.complete_onedrive_upload(&credential.session_id, &credential.callback_secret)
                .await
                .context("Failed to complete OneDrive upload")?;
        }
    } else {
        anyhow::bail!("Storage policy type {:?} is not supported for CLI upload yet. Please use a local storage policy or one with relay enabled.", policy_type);
    }
    
    Ok(())
}

async fn read_chunk(file: &tokio::fs::File, offset: u64, size: u64) -> Result<Bytes> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    
    let mut file = file.try_clone().await?;
    file.seek(std::io::SeekFrom::Start(offset)).await?;
    
    let mut buffer = vec![0u8; size as usize];
    file.read_exact(&mut buffer).await?;
    
    Ok(Bytes::from(buffer))
}

fn init_client(profile_name: &Option<String>) -> Result<Client> {
    if let Some(env_profile) = env_config() {
        let config = ClientConfig::new(&env_profile.server)
            .with_user_agent(format!("cloudreve-cli/{}", env!("CARGO_PKG_VERSION")));
        let client = Client::new(config);
        return Ok(client);
    }
    
    let cfg = Config::load().context("Failed to load config")?;
    let profile = match profile_name {
        Some(ref n) => cfg.profiles.iter().find(|p| &p.name == n),
        None => cfg.get_active_profile(),
    };
    
    let p = profile.ok_or_else(|| anyhow::anyhow!(
        "No profile found. Run 'cloudreve login' first or set CLOUDREVE_* environment variables."
    ))?;
    
    let config = ClientConfig::new(&p.server)
        .with_user_agent(format!("cloudreve-cli/{}", env!("CARGO_PKG_VERSION")));
    let client = Client::new(config);
    Ok(client)
}

async fn set_tokens(client: &Client, profile_name: &Option<String>) -> Result<()> {
    if let Some(env_profile) = env_config() {
        client.set_tokens(env_profile.access_token, env_profile.refresh_token).await;
        return Ok(());
    }
    
    let cfg = Config::load().context("Failed to load config")?;
    let profile = match profile_name {
        Some(ref n) => cfg.profiles.iter().find(|p| &p.name == n),
        None => cfg.get_active_profile(),
    };
    
    let p = profile.ok_or_else(|| anyhow::anyhow!(
        "No profile found. Run 'cloudreve login' first."
    ))?;
    
    client.set_tokens(p.access_token.clone(), p.refresh_token.clone()).await;
    Ok(())
}
