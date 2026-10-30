use solana_program::program_error::ProgramError;

/// First byte of instruction data is the tag, u64 args follow little-endian.
/// Account lists are documented on each handler in `processor.rs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaleInstruction {
    Initialize { token_price: u64 },
    Buy { lamports: u64, min_tokens_out: u64 },
    WithdrawSol,
    GetRate,
    TransferOwnership,
    WithdrawTokens { amount: u64 },
    SetTokenPrice { new_price: u64 },
    CancelPendingPrice,
    GuardianRecoverOwner,
    TransferTreasury,
    ClaimLockBonus,
    WithdrawBonusTokens { amount: u64 },
}

impl SaleInstruction {
    pub fn unpack(data: &[u8]) -> Result<Self, ProgramError> {
        let (&tag, rest) = data
            .split_first()
            .ok_or(ProgramError::InvalidInstructionData)?;

        Ok(match tag {
            0 => Self::Initialize {
                token_price: read_u64(rest, 0)?,
            },
            1 => Self::Buy {
                lamports: read_u64(rest, 0)?,
                min_tokens_out: read_u64(rest, 8)?,
            },
            2 => Self::WithdrawSol,
            3 => Self::GetRate,
            4 => Self::TransferOwnership,
            5 => Self::WithdrawTokens {
                amount: read_u64(rest, 0)?,
            },
            6 => Self::SetTokenPrice {
                new_price: read_u64(rest, 0)?,
            },
            7 => Self::CancelPendingPrice,
            8 => Self::GuardianRecoverOwner,
            9 => Self::TransferTreasury,
            10 => Self::ClaimLockBonus,
            11 => Self::WithdrawBonusTokens {
                amount: read_u64(rest, 0)?,
            },
            _ => return Err(ProgramError::InvalidInstructionData),
        })
    }
}

fn read_u64(data: &[u8], offset: usize) -> Result<u64, ProgramError> {
    data.get(offset..offset + 8)
        .and_then(|b| b.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or(ProgramError::InvalidInstructionData)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ix(tag: u8, args: &[u64]) -> Vec<u8> {
        let mut v = vec![tag];
        for a in args {
            v.extend_from_slice(&a.to_le_bytes());
        }
        v
    }

    #[test]
    fn unpacks_buy() {
        let got = SaleInstruction::unpack(&ix(1, &[5_000_000_000, 123])).unwrap();
        assert_eq!(
            got,
            SaleInstruction::Buy {
                lamports: 5_000_000_000,
                min_tokens_out: 123
            }
        );
    }

    #[test]
    fn unpacks_no_arg_instructions() {
        assert_eq!(
            SaleInstruction::unpack(&[2]).unwrap(),
            SaleInstruction::WithdrawSol
        );
        assert_eq!(
            SaleInstruction::unpack(&[10]).unwrap(),
            SaleInstruction::ClaimLockBonus
        );
    }

    #[test]
    fn unpacks_single_arg_instructions() {
        assert_eq!(
            SaleInstruction::unpack(&ix(0, &[100_000])).unwrap(),
            SaleInstruction::Initialize {
                token_price: 100_000
            }
        );
        assert_eq!(
            SaleInstruction::unpack(&ix(11, &[7])).unwrap(),
            SaleInstruction::WithdrawBonusTokens { amount: 7 }
        );
    }

    #[test]
    fn rejects_bad_data() {
        assert!(SaleInstruction::unpack(&[]).is_err());
        assert!(SaleInstruction::unpack(&[12]).is_err());
        // buy without min_tokens_out
        assert!(SaleInstruction::unpack(&ix(1, &[1_000_000_000])).is_err());
        assert!(SaleInstruction::unpack(&[6, 1, 2, 3]).is_err());
    }
}
