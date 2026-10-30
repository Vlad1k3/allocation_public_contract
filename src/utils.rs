use solana_program::{
    account_info::AccountInfo, entrypoint::ProgramResult, program::invoke_signed,
    program_error::ProgramError, program_pack::Pack, pubkey::Pubkey,
};
use spl_token::state::Account as TokenAccount;

use crate::error::SaleError;

pub fn require_signer(acc: &AccountInfo) -> ProgramResult {
    if !acc.is_signer {
        return Err(ProgramError::MissingRequiredSignature);
    }
    Ok(())
}

pub fn require_key(acc: &AccountInfo, expected: &Pubkey, err: SaleError) -> ProgramResult {
    if acc.key != expected {
        return Err(err.into());
    }
    Ok(())
}

/// Verifies `acc` is the PDA for `seeds` and returns its bump.
pub fn require_pda(
    acc: &AccountInfo,
    seeds: &[&[u8]],
    program_id: &Pubkey,
) -> Result<u8, ProgramError> {
    let (expected, bump) = Pubkey::find_program_address(seeds, program_id);
    if acc.key != &expected {
        return Err(SaleError::Unauthorized.into());
    }
    Ok(bump)
}

pub fn token_account(acc: &AccountInfo) -> Result<TokenAccount, ProgramError> {
    if acc.owner != &spl_token::id() {
        return Err(SaleError::InvalidAccountOwner.into());
    }
    TokenAccount::unpack(&acc.try_borrow_data()?)
}

pub fn vault_transfer<'a>(
    token_program: &AccountInfo<'a>,
    from: &AccountInfo<'a>,
    to: &AccountInfo<'a>,
    authority: &AccountInfo<'a>,
    amount: u64,
    signer_seeds: &[&[u8]],
) -> ProgramResult {
    let ix = spl_token::instruction::transfer(
        token_program.key,
        from.key,
        to.key,
        authority.key,
        &[],
        amount,
    )?;
    invoke_signed(
        &ix,
        &[
            from.clone(),
            to.clone(),
            authority.clone(),
            token_program.clone(),
        ],
        &[signer_seeds],
    )
}
