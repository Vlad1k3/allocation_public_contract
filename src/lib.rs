#![allow(unexpected_cfgs)]

pub mod constants;
pub mod error;
pub mod instruction;
pub mod pricing;
pub mod processor;
pub mod state;
pub mod streamflow;
mod utils;

use solana_program::{account_info::AccountInfo, entrypoint::ProgramResult, pubkey::Pubkey};

#[cfg(not(feature = "no-entrypoint"))]
solana_program::entrypoint!(process_instruction);

pub fn process_instruction(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    data: &[u8],
) -> ProgramResult {
    processor::process(program_id, accounts, data)
}
