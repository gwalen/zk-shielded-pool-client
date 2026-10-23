use std::{collections::HashMap, fmt, str::FromStr, sync::Arc};

use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;

use crate::app_error::{AppError, RequestResult};

/// Which wallet set a `UserWallets` holds. Used in error messages and `Debug`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalletKind {
    Deposit,
    Withdraw,
}

impl fmt::Display for WalletKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Deposit => "deposit",
            Self::Withdraw => "withdraw",
        })
    }
}

/// One configured wallet set (deposit or withdraw). Lookups by public key; the first wallet
/// in config order is the default.
pub struct UserWallets {
    kind: WalletKind,
    keypairs: Vec<Arc<Keypair>>,
    index: HashMap<Pubkey, Arc<Keypair>>,
}

impl UserWallets {
    /// Runs at startup. Infallible: `Config` already rejects an empty key list
    /// (`parse_wallet_keys`), and `default()` relies on that.
    pub fn new(kind: WalletKind, keypairs: &[Arc<Keypair>]) -> Self {
        let index = keypairs
            .iter()
            .map(|k| (k.pubkey(), Arc::clone(k)))
            .collect();
        Self {
            kind,
            keypairs: keypairs.to_vec(),
            index,
        }
    }

    pub fn kind(&self) -> WalletKind {
        self.kind
    }

    pub fn get(&self, pubkey: &Pubkey) -> Option<Arc<Keypair>> {
        self.index.get(pubkey).cloned()
    }

    /// Runs per request. 400 for a bad base58 address or a wallet that is not in this set.
    /// The error never lists the configured wallets.
    pub fn parse_and_get(&self, address: &str) -> RequestResult<Arc<Keypair>> {
        let pubkey = Pubkey::from_str(address.trim())
            .map_err(|_| AppError::BadRequest(format!("invalid {} wallet address", self.kind)))?;
        self.get(&pubkey).ok_or_else(|| {
            AppError::BadRequest(format!("{pubkey} is not a configured {} wallet", self.kind))
        })
    }

    /// The first wallet in config order.
    pub fn default(&self) -> Arc<Keypair> {
        Arc::clone(&self.keypairs[0])
    }

    pub fn pubkeys(&self) -> Vec<Pubkey> {
        self.keypairs.iter().map(|k| k.pubkey()).collect()
    }
}

impl fmt::Debug for UserWallets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pubkeys: Vec<String> = self.pubkeys().iter().map(ToString::to_string).collect();
        f.debug_struct("UserWallets")
            .field("kind", &self.kind)
            .field("pubkeys", &pubkeys)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{http::StatusCode, response::IntoResponse};

    fn wallets(kind: WalletKind, n: usize) -> UserWallets {
        let keys: Vec<_> = (0..n).map(|_| Arc::new(Keypair::new())).collect();
        UserWallets::new(kind, &keys)
    }

    #[test]
    fn lookup_and_default() {
        let set = wallets(WalletKind::Deposit, 2);
        let [first, second] = set.pubkeys()[..] else {
            panic!()
        };
        assert_eq!(set.default().pubkey(), first);
        assert_eq!(
            set.parse_and_get(&second.to_string()).unwrap().pubkey(),
            second
        );
    }

    /// A withdraw wallet sent as `deposit_wallet_address` is a 400, and the other way round.
    #[test]
    fn a_wallet_from_the_other_set_is_a_bad_request() {
        let deposit = wallets(WalletKind::Deposit, 1);
        let withdraw = wallets(WalletKind::Withdraw, 1);

        for (set, other) in [(&deposit, &withdraw), (&withdraw, &deposit)] {
            let err = set
                .parse_and_get(&other.default().pubkey().to_string())
                .unwrap_err();
            let message = err.to_string();
            assert!(
                message.contains(&format!("not a configured {} wallet", set.kind())),
                "{message}"
            );
            assert!(
                !message.contains(&set.default().pubkey().to_string()),
                "lists configured wallets"
            );
            assert_eq!(err.into_response().status(), StatusCode::BAD_REQUEST);
        }

        let err = deposit.parse_and_get("not-a-key").unwrap_err();
        assert_eq!(err.into_response().status(), StatusCode::BAD_REQUEST);
    }
}
