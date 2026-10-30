use solana_program::program_error::ProgramError;

// Codes are part of the client API, append only.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaleError {
    InvalidAmount = 0,
    Overflow = 1,
    Unauthorized = 2,
    InvalidPriceFeed = 3,
    StalePriceFeed = 4,
    InvalidAccountOwner = 5,
    InvalidProgramAccount = 6,
    SlippageExceeded = 7,
    InvalidTokenPrice = 8,
    NoPendingPrice = 9,
    InvalidTokenMint = 10,
    InvalidStream = 11,
    LockTooShort = 12,
    LockTooSmall = 13,
    LockCancelable = 14,
    LockCanceled = 15,
    BonusAlreadyClaimed = 16,
    LockTransferable = 17,
    LockStaleStart = 18,
    LockShapeInvalid = 19,
    StreamflowVersionMismatch = 20,
}

impl From<SaleError> for ProgramError {
    fn from(e: SaleError) -> Self {
        ProgramError::Custom(e as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_did_not_shift() {
        assert_eq!(SaleError::InvalidAmount as u32, 0);
        assert_eq!(SaleError::SlippageExceeded as u32, 7);
        assert_eq!(SaleError::InvalidTokenMint as u32, 10);
        assert_eq!(SaleError::BonusAlreadyClaimed as u32, 16);
        assert_eq!(SaleError::StreamflowVersionMismatch as u32, 20);
    }

    #[test]
    fn maps_to_custom_program_error() {
        let e: ProgramError = SaleError::StalePriceFeed.into();
        assert_eq!(e, ProgramError::Custom(4));
    }
}
