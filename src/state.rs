use borsh::{BorshDeserialize, BorshSerialize};
use solana_program::{
    account_info::AccountInfo, entrypoint::ProgramResult, program_error::ProgramError,
    pubkey::Pubkey,
};

use crate::error::SaleError;

#[derive(BorshSerialize, BorshDeserialize, Debug, Clone, PartialEq, Eq)]
pub struct SaleState {
    pub owner: Pubkey,    // hot key: pricing, config
    pub treasury: Pubkey, // cold key: the only role that can move funds
    pub guardian: Pubkey, // offline key: can replace owner / veto a price change

    pub token_mint: Pubkey,
    pub token_price_usd: u64, // micro-USD
    pub pending_price_usd: u64,
    pub price_effective_at: i64,

    pub token_vault_bump: u8,
    pub sol_vault_bump: u8,

    pub total_collected: u64, // lamports since last withdraw
}

impl SaleState {
    pub const LEN: usize = 32 * 4 + 8 * 4 + 2;

    pub fn load(account: &AccountInfo, program_id: &Pubkey) -> Result<Self, ProgramError> {
        if account.owner != program_id {
            return Err(SaleError::InvalidAccountOwner.into());
        }
        Ok(Self::deserialize(&mut &account.data.borrow()[..])?)
    }

    pub fn save(&self, account: &AccountInfo) -> ProgramResult {
        self.serialize(&mut &mut account.data.borrow_mut()[..])?;
        Ok(())
    }

    /// Price that applies at `now`, without touching state.
    pub fn effective_price(&self, now: i64) -> u64 {
        if self.pending_price_usd != 0 && now >= self.price_effective_at {
            self.pending_price_usd
        } else {
            self.token_price_usd
        }
    }

    /// Promotes the pending price once its timelock is over. Returns true if it did.
    pub fn apply_pending_price(&mut self, now: i64) -> bool {
        if self.pending_price_usd == 0 || now < self.price_effective_at {
            return false;
        }
        self.token_price_usd = self.pending_price_usd;
        self.clear_pending_price();
        true
    }

    pub fn clear_pending_price(&mut self) {
        self.pending_price_usd = 0;
        self.price_effective_at = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> SaleState {
        SaleState {
            owner: Pubkey::new_unique(),
            treasury: Pubkey::new_unique(),
            guardian: Pubkey::new_unique(),
            token_mint: Pubkey::new_unique(),
            token_price_usd: 100_000,
            pending_price_usd: 0,
            price_effective_at: 0,
            token_vault_bump: 255,
            sol_vault_bump: 254,
            total_collected: 0,
        }
    }

    #[test]
    fn len_matches_borsh_size() {
        let mut buf = Vec::new();
        sample().serialize(&mut buf).unwrap();
        assert_eq!(buf.len(), SaleState::LEN);
        assert_eq!(SaleState::LEN, 162);
    }

    #[test]
    fn borsh_roundtrip() {
        let mut s = sample();
        s.pending_price_usd = 120_000;
        s.price_effective_at = 1_700_000_000;
        s.total_collected = 42 * 1_000_000_000;

        let mut buf = Vec::new();
        s.serialize(&mut buf).unwrap();
        assert_eq!(SaleState::deserialize(&mut buf.as_slice()).unwrap(), s);
    }

    #[test]
    fn zeroed_account_decodes() {
        let zeros = vec![0u8; SaleState::LEN];
        let s = SaleState::deserialize(&mut zeros.as_slice()).unwrap();
        assert_eq!(s.owner, Pubkey::default());
        assert_eq!(s.token_price_usd, 0);
    }

    #[test]
    fn pending_price_waits_for_timelock() {
        let mut s = sample();
        s.pending_price_usd = 200_000;
        s.price_effective_at = 1_000;

        assert_eq!(s.effective_price(999), 100_000);
        assert!(!s.apply_pending_price(999));
        assert_eq!(s.token_price_usd, 100_000);

        assert_eq!(s.effective_price(1_000), 200_000);
        assert!(s.apply_pending_price(1_000));
        assert_eq!(s.token_price_usd, 200_000);
        assert_eq!(s.pending_price_usd, 0);
        assert_eq!(s.price_effective_at, 0);
    }

    #[test]
    fn nothing_to_apply_without_pending() {
        let mut s = sample();
        assert!(!s.apply_pending_price(i64::MAX));
        assert_eq!(s.token_price_usd, 100_000);
    }
}
