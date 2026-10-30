use solana_program::{native_token::LAMPORTS_PER_SOL, pubkey, pubkey::Pubkey};

// Chainlink SOL/USD answers have 8 decimals
pub const PRICE_DECIMALS: u32 = 8;
// sale token is expected to use 9 decimals, same as SOL
pub const TOKEN_UNIT: u64 = 1_000_000_000;

pub const MIN_PURCHASE_LAMPORTS: u64 = LAMPORTS_PER_SOL / 100;
pub const MAX_PURCHASE_LAMPORTS: u64 = 100_000 * LAMPORTS_PER_SOL;

pub const MAX_PRICE_AGE_SECONDS: u64 = 300;

// token price is stored in micro-USD ($1 = 1_000_000)
pub const MIN_TOKEN_PRICE: u64 = 1_000; // $0.001
pub const MAX_TOKEN_PRICE: u64 = 1_000_000_000; // $1000

#[cfg(feature = "short-timelock")]
pub const PRICE_TIMELOCK_SECONDS: i64 = 120;
#[cfg(not(feature = "short-timelock"))]
pub const PRICE_TIMELOCK_SECONDS: i64 = 86_400;

// lock bonus
pub const BONUS_BPS: u64 = 500;
pub const MIN_LOCK_DURATION_SECS: u64 = 365 * 86_400;
pub const MIN_LOCK_AMOUNT: u64 = 1_000 * TOKEN_UNIT;
pub const MAX_LOCK_START_DRIFT_SECS: u64 = 7 * 86_400;

// PDA seeds
pub const TOKEN_VAULT_SEED: &[u8] = b"token_vault";
pub const SOL_VAULT_SEED: &[u8] = b"sol_vault";
pub const BONUS_VAULT_SEED: &[u8] = b"bonus_vault";
pub const BONUS_CLAIMED_SEED: &[u8] = b"bonus_claimed";

pub const CHAINLINK_PROGRAM_ID: Pubkey = pubkey!("HEvSKofvBgfaexv23kMabbYqxasxU3mQ4ibBMEmJWHny");
pub const SOL_USD_FEED_MAINNET: Pubkey = pubkey!("CH31Xns5z3M1cTAbKW34jcxPPciazARpijcHj9rxtemt");
pub const SOL_USD_FEED_DEVNET: Pubkey = pubkey!("99B2bTijsU6f1GCT73HmdR7HCFFjGMBcPZY6jZ96ynrR");

pub const STREAMFLOW_MAINNET: Pubkey = pubkey!("strmRqUCoQUgGUan5YhzUZa6KqdzwX5L6FpUxfmKg5m");
pub const STREAMFLOW_DEVNET: Pubkey = pubkey!("HqDGZjaVRXJ9MGRQEw7qDc2rAr6iH1n1kAQdCZaCMfMZ");
