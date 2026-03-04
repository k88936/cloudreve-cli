mod commands;
mod config;

use clap::{Parser, Subcommand};
use commands::{AuthCommands, UploadCommand};

#[derive(Parser)]
#[command(name = "cloudreve")]
#[command(author, version, about = "Cloudreve cloud storage CLI", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
    
    #[arg(short = 'p', long, global = true, help = "Profile name to use")]
    profile: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    #[command(subcommand)]
    Auth(AuthCommands),
    
    #[command(alias = "up")]
    Upload(UploadCommand),
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    
    match cli.command {
        Commands::Auth(auth_cmd) => auth_cmd.execute(cli.profile).await?,
        Commands::Upload(mut upload_cmd) => {
            if cli.profile.is_some() {
                upload_cmd.profile = cli.profile;
            }
            upload_cmd.execute().await?;
        }
    }
    
    Ok(())
}
