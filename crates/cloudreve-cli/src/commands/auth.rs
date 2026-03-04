use anyhow::{Context, Result};
use clap::Subcommand;
use cloudreve_api::{api::UserApi, Client, ClientConfig};
use crate::config::{Config, Profile, env_config};

#[derive(Debug, Subcommand)]
pub enum AuthCommands {
    #[command(about = "Login to Cloudreve server")]
    Login {
        #[arg(short, long, env = "CLOUDREVE_SERVER", help = "Server URL (e.g., https://cloud.example.com)")]
        server: String,
        #[arg(short, long, env = "CLOUDREVE_EMAIL", help = "Email address")]
        email: String,
        #[arg(short = 'P', long, env = "CLOUDREVE_PASSWORD", help = "Password")]
        password: String,
        #[arg(short = 'n', long, help = "Profile name to save (default: 'default')")]
        name: Option<String>,
    },
    #[command(about = "Logout and clear stored credentials")]
    Logout,
    #[command(about = "Show current authentication status")]
    Status,
    #[command(about = "List all saved profiles")]
    List,
}

impl AuthCommands {
    pub async fn execute(self, global_profile: Option<String>) -> Result<()> {
        match self {
            AuthCommands::Login { server, email, password, name } => {
                login(&server, &email, &password, name).await
            }
            AuthCommands::Logout => logout(global_profile),
            AuthCommands::Status => status(global_profile).await,
            AuthCommands::List => list_profiles(),
        }
    }
}

async fn login(server: &str, email: &str, password: &str, name: Option<String>) -> Result<()> {
    let profile_name = name.unwrap_or_else(|| "default".to_string());
    
    println!("Connecting to {}...", server);
    
    let config = ClientConfig::new(server)
        .with_user_agent(format!("cloudreve-cli/{}", env!("CARGO_PKG_VERSION")));
    let client = Client::new(config);
    
    let response = client
        .login(email, password)
        .await
        .context("Login failed")?;
    
    client.set_tokens(
        response.token.access_token.clone(),
        response.token.refresh_token.clone(),
    ).await;
    
    let profile = Profile {
        name: profile_name.clone(),
        server: server.trim_end_matches('/').to_string(),
        access_token: response.token.access_token,
        refresh_token: response.token.refresh_token,
        user_email: response.user.email.clone(),
        user_nickname: Some(response.user.nickname.clone()),
    };
    
    let mut cfg = Config::load().unwrap_or_default();
    cfg.add_or_update_profile(profile);
    cfg.active_profile = Some(profile_name.clone());
    cfg.save().context("Failed to save config")?;
    
    let email_display = response.user.email.as_deref().unwrap_or("no email");
    println!("✓ Logged in as {} ({})", response.user.nickname, email_display);
    println!("  Profile: {}", profile_name);
    
    Ok(())
}

fn logout(global_profile: Option<String>) -> Result<()> {
    let mut cfg = Config::load().unwrap_or_default();
    
    let profile_name = global_profile.or(cfg.active_profile.clone());
    
    match profile_name {
        Some(ref pname) => {
            if cfg.remove_profile(pname) {
                cfg.save().context("Failed to save config")?;
                println!("✓ Logged out from profile '{}'", pname);
            } else {
                println!("Profile '{}' not found", pname);
            }
        }
        None => {
            println!("No profile specified and no active profile set");
        }
    }
    
    Ok(())
}

async fn status(global_profile: Option<String>) -> Result<()> {
    if let Some(env_profile) = env_config() {
        println!("Using credentials from environment variables:");
        println!("  Server: {}", env_profile.server);
        if let Some(ref email) = env_profile.user_email {
            println!("  Email: {}", email);
        }
        println!();
    }
    
    let cfg = Config::load().unwrap_or_default();
    
    let profile = match global_profile {
        Some(ref n) => cfg.profiles.iter().find(|p| &p.name == n),
        None => cfg.get_active_profile(),
    };
    
    match profile {
        Some(p) => {
            println!("Profile: {}", p.name);
            println!("  Server: {}", p.server);
            if let Some(ref email) = p.user_email {
                println!("  Email: {}", email);
            }
            if let Some(ref nick) = p.user_nickname {
                println!("  Nickname: {}", nick);
            }
            
            let client_config = ClientConfig::new(&p.server)
                .with_user_agent(format!("cloudreve-cli/{}", env!("CARGO_PKG_VERSION")));
            let client = Client::new(client_config);
            client.set_tokens(p.access_token.clone(), p.refresh_token.clone()).await;
            
            match client.get_user_me().await {
                Ok(user) => {
                    println!("  Status: ✓ Authenticated");
                    println!("  User ID: {}", user.id);
                }
                Err(e) => {
                    println!("  Status: ✗ Authentication failed ({})", e);
                    println!("  Try running 'cloudreve login' again");
                }
            }
        }
        None => {
            println!("No active profile found.");
            println!("Run 'cloudreve login' to authenticate.");
        }
    }
    
    Ok(())
}

fn list_profiles() -> Result<()> {
    let cfg = Config::load().unwrap_or_default();
    
    if cfg.profiles.is_empty() {
        println!("No profiles configured.");
        println!("Run 'cloudreve login' to create a profile.");
        return Ok(());
    }
    
    println!("Profiles:");
    for profile in &cfg.profiles {
        let active = cfg.active_profile.as_ref() == Some(&profile.name);
        let marker = if active { "*" } else { " " };
        println!("  {} {} ({})", marker, profile.name, profile.server);
        if let Some(ref email) = profile.user_email {
            println!("      Email: {}", email);
        }
    }
    
    Ok(())
}
