use crate::config;
use anyhow::Result;
use cloudreve_api::{Client, ClientConfig};
use cloudreve_api::api::user::UserApi;

pub async fn run(server: &str, email: &str, password: &str) -> Result<()> {
    println!("Logging in to {}...", server);
    
    let client_config = ClientConfig::new(server);
    let client = Client::new(client_config);
    
    let response = client.login(email, password).await?;
    
    println!("Login successful!");
    println!("User: {} ({})", response.user.nickname, response.user.id);
    
    config::save_credentials(server, email, password)?;
    
    println!("Credentials saved to ~/cloudreve-cli.toml");
    
    Ok(())
}
