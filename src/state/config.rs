/// Application settings read from the environment once at startup.
#[derive(Debug)]
pub struct Config {
    pub version: String,
    pub solana_rpc_url: String,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let version = std::env::var("VERSION").map_err(|_| "missing VERSION in .env")?;
        let solana_rpc_url = std::env::var("SOLANA_RPC_URL").map_err(|_| "missing SOLANA_RPC_URL in .env")?;

        Ok(
            Self {
                version, 
                solana_rpc_url
            }
        )
    }
}
