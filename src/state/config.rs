use std::{collections::HashSet, fmt, path::PathBuf, str::FromStr, sync::Arc};

use anchor_client::Cluster;
use anyhow::anyhow;
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;

use crate::app_error::RawResult;

pub const DEFAULT_PROGRAM_ID: &str = "FCrymxYUTEnXDJXdDn2E71KPxB9sBjXZCh1ezBwmUhvp";
pub const DEFAULT_ZK_DEPOSIT_DATA_DIR: &str = "./data/zk-deposit-params";

const DEPOSIT_KEYS_VAR: &str = "DEPOSIT_WALLET_PRIVATE_KEYS";
const WITHDRAW_KEYS_VAR: &str = "WITHDRAW_WALLET_PRIVATE_KEYS";

/// Application settings read from the environment once at startup.
///
/// Startup code: errors are `RawResult` (anyhow) and stop the process. Code on the request
/// path returns `RequestResult` (`AppError`, mapped to an HTTP status).
///
/// Wallet keys are parsed here, once. `Debug` never prints them.
pub struct Config {
    pub version: String,
    pub solana_rpc_url: String,
    pub solana_ws_url: String,
    pub program_id: Pubkey,
    /// Depositors: sign the deposit and pay the funds and fees.
    pub deposit_wallet_keys: Vec<Arc<Keypair>>,
    /// Fee payers and signers for upload_proof / withdraw / close_proof. The first is the default.
    pub withdraw_wallet_keys: Vec<Arc<Keypair>>,
    /// `ZK_DEPOSIT_DATA`: one JSON file per deposit with its zk parameters (secret `s`, chunks, destinations).
    pub zk_deposit_data_dir: PathBuf,
}

impl Config {
    pub fn from_env() -> RawResult<Self> {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    /// Same as `from_env`, but reads the variables through `get` (tests build a config without
    /// touching the process environment).
    pub fn from_vars(get: impl Fn(&str) -> Option<String>) -> RawResult<Self> {
        let version = Self::required(&get, "VERSION")?;
        let solana_rpc_url = Self::required(&get, "SOLANA_RPC_URL")?;
        let solana_ws_url = match Self::optional(&get, "SOLANA_WS_URL") {
            Some(url) => url,
            None => derive_ws_url(&solana_rpc_url)?,
        };

        let program_id_raw =
            Self::optional(&get, "PROGRAM_ID").unwrap_or_else(|| DEFAULT_PROGRAM_ID.into());
        let program_id = Pubkey::from_str(program_id_raw.trim())
            .map_err(|_| anyhow!("PROGRAM_ID is not a valid public key: {program_id_raw}"))?;
        // Instructions go to the IDL address. A different PROGRAM_ID would only mislead.
        if program_id != crate::solana::program_helpers::ID {
            return Err(anyhow!(
                "PROGRAM_ID {program_id} differs from the IDL address {}; run scripts/sync-idl.sh",
                crate::solana::program_helpers::ID
            ));
        }

        let deposit_wallet_keys =
            parse_wallet_keys(DEPOSIT_KEYS_VAR, &Self::required(&get, DEPOSIT_KEYS_VAR)?)?;
        let withdraw_wallet_keys =
            parse_wallet_keys(WITHDRAW_KEYS_VAR, &Self::required(&get, WITHDRAW_KEYS_VAR)?)?;
        check_disjoint(&deposit_wallet_keys, &withdraw_wallet_keys)?;

        let zk_deposit_data_dir = Self::optional(&get, "ZK_DEPOSIT_DATA")
            .unwrap_or_else(|| DEFAULT_ZK_DEPOSIT_DATA_DIR.into())
            .into();

        Ok(Self {
            version,
            solana_rpc_url,
            solana_ws_url,
            program_id,
            deposit_wallet_keys,
            withdraw_wallet_keys,
            zk_deposit_data_dir,
        })
    }

    pub fn cluster(&self) -> Cluster {
        Cluster::Custom(self.solana_rpc_url.clone(), self.solana_ws_url.clone())
    }

    fn required(get: &impl Fn(&str) -> Option<String>, name: &str) -> RawResult<String> {
        get(name).ok_or_else(|| anyhow!("missing {name} in .env"))
    }

    fn optional(get: &impl Fn(&str) -> Option<String>, name: &str) -> Option<String> {
        get(name).filter(|value| !value.trim().is_empty())
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("version", &self.version)
            .field("solana_rpc_url", &self.solana_rpc_url)
            .field("solana_ws_url", &self.solana_ws_url)
            .field("program_id", &self.program_id.to_string())
            .field(
                "deposit_wallet_keys",
                &Redacted(self.deposit_wallet_keys.len()),
            )
            .field(
                "withdraw_wallet_keys",
                &Redacted(self.withdraw_wallet_keys.len()),
            )
            .field("zk_deposit_data_dir", &self.zk_deposit_data_dir)
            .finish()
    }
}

struct Redacted(usize);

impl fmt::Debug for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<{} redacted keys>", self.0)
    }
}

/// `http(s)://host:8899` -> `ws(s)://host:8900`, the same rule as the Solana CLI.
fn derive_ws_url(rpc_url: &str) -> RawResult<String> {
    match Cluster::from_str(rpc_url) {
        Ok(Cluster::Custom(_, ws_url)) => Ok(ws_url),
        _ => Err(anyhow!("SOLANA_RPC_URL must be an http(s) url: {rpc_url}")),
    }
}

/// Comma-separated base58 64-byte secret keys. Errors name the position or the public key,
/// never the secret.
fn parse_wallet_keys(var: &str, raw: &str) -> RawResult<Vec<Arc<Keypair>>> {
    let mut keys = Vec::new();
    let mut seen = HashSet::new();
    for (position, entry) in raw
        .split(',')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .enumerate()
    {
        let n = position + 1;
        let bytes = bs58::decode(entry)
            .into_vec()
            .map_err(|_| anyhow!("{var}: key #{n} is not valid base58"))?;
        let keypair = Keypair::try_from(bytes.as_slice())
            .map_err(|_| anyhow!("{var}: key #{n} is not a valid 64-byte keypair"))?;
        if !seen.insert(keypair.pubkey()) {
            return Err(anyhow!(
                "{var}: wallet {} is listed twice",
                keypair.pubkey()
            ));
        }
        keys.push(Arc::new(keypair));
    }
    if keys.is_empty() {
        return Err(anyhow!("{var} must hold at least one key"));
    }
    Ok(keys)
}

/// The server must never sign a deposit and its withdrawal with the same wallet.
fn check_disjoint(deposit: &[Arc<Keypair>], withdraw: &[Arc<Keypair>]) -> RawResult<()> {
    let deposit: HashSet<Pubkey> = deposit.iter().map(|k| k.pubkey()).collect();
    match withdraw
        .iter()
        .map(|k| k.pubkey())
        .find(|pubkey| deposit.contains(pubkey))
    {
        Some(pubkey) => Err(anyhow!(
            "wallet {pubkey} is in both {DEPOSIT_KEYS_VAR} and {WITHDRAW_KEYS_VAR}"
        )),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn secret(keypair: &Keypair) -> String {
        bs58::encode(keypair.to_bytes()).into_string()
    }

    fn env(deposit: &str, withdraw: &str) -> HashMap<&'static str, String> {
        HashMap::from([
            ("VERSION", "test".to_string()),
            ("SOLANA_RPC_URL", "http://127.0.0.1:8899".to_string()),
            (DEPOSIT_KEYS_VAR, deposit.to_string()),
            (WITHDRAW_KEYS_VAR, withdraw.to_string()),
        ])
    }

    fn load(vars: HashMap<&'static str, String>) -> Result<Config, String> {
        Config::from_vars(|name| vars.get(name).cloned()).map_err(|e| e.to_string())
    }

    #[test]
    fn loads_with_defaults() {
        let (d1, d2, w1) = (Keypair::new(), Keypair::new(), Keypair::new());
        let config = load(env(
            &format!("{}, {}", secret(&d1), secret(&d2)),
            &secret(&w1),
        ))
        .unwrap();

        assert_eq!(config.program_id.to_string(), DEFAULT_PROGRAM_ID);
        assert_eq!(config.solana_ws_url, "ws://127.0.0.1:8900/");
        assert_eq!(
            config.zk_deposit_data_dir,
            PathBuf::from(DEFAULT_ZK_DEPOSIT_DATA_DIR)
        );
        assert_eq!(config.deposit_wallet_keys.len(), 2);
        assert_eq!(config.withdraw_wallet_keys[0].pubkey(), w1.pubkey());
    }

    #[test]
    fn a_key_in_both_lists_fails_startup() {
        let shared = Keypair::new();
        let err = load(env(
            &secret(&shared),
            &format!("{},{}", secret(&Keypair::new()), secret(&shared)),
        ))
        .unwrap_err();
        assert!(err.contains(&shared.pubkey().to_string()), "{err}");
        assert!(!err.contains(&secret(&shared)), "error leaks the secret");
    }

    #[test]
    fn duplicate_or_invalid_keys_fail_without_leaking() {
        let key = Keypair::new();
        let err = load(env(
            &format!("{},{}", secret(&key), secret(&key)),
            &secret(&Keypair::new()),
        ))
        .unwrap_err();
        assert!(err.contains("listed twice"), "{err}");
        assert!(!err.contains(&secret(&key)));

        let bad = "not-base58-0OIl";
        let err = load(env(bad, &secret(&Keypair::new()))).unwrap_err();
        assert!(err.contains("key #1") && !err.contains(bad), "{err}");

        let short = bs58::encode([7u8; 32]).into_string();
        let err = load(env(&short, &secret(&Keypair::new()))).unwrap_err();
        assert!(err.contains("64-byte") && !err.contains(&short), "{err}");

        assert!(load(env(" , ", &secret(&Keypair::new()))).is_err());
    }

    #[test]
    fn program_id_must_match_the_idl() {
        let mut vars = env(&secret(&Keypair::new()), &secret(&Keypair::new()));
        vars.insert("PROGRAM_ID", Pubkey::new_unique().to_string());
        assert!(
            load(vars)
                .unwrap_err()
                .contains("differs from the IDL address")
        );
    }

    #[test]
    fn debug_never_prints_keys() {
        let (d, w) = (Keypair::new(), Keypair::new());
        let config = load(env(&secret(&d), &secret(&w))).unwrap();
        let rendered = format!("{config:?}");
        assert!(rendered.contains("<1 redacted keys>"));
        assert!(!rendered.contains(&secret(&d)) && !rendered.contains(&secret(&w)));
    }
}
