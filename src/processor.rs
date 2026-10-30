use solana_program::{
    account_info::{next_account_info, AccountInfo},
    clock::Clock,
    entrypoint::ProgramResult,
    msg,
    native_token::LAMPORTS_PER_SOL,
    program::{invoke, invoke_signed},
    program_error::ProgramError,
    pubkey::Pubkey,
    rent::Rent,
    system_instruction, system_program,
    sysvar::Sysvar,
};

use crate::{
    constants::*,
    error::SaleError,
    instruction::SaleInstruction,
    pricing,
    state::SaleState,
    streamflow::StreamInfo,
    utils::{require_key, require_pda, require_signer, token_account, vault_transfer},
};

pub fn process(program_id: &Pubkey, accounts: &[AccountInfo], data: &[u8]) -> ProgramResult {
    use SaleInstruction::*;

    match SaleInstruction::unpack(data)? {
        Initialize { token_price } => initialize(program_id, accounts, token_price),
        Buy {
            lamports,
            min_tokens_out,
        } => buy(program_id, accounts, lamports, min_tokens_out),
        WithdrawSol => withdraw_sol(program_id, accounts),
        GetRate => get_rate(program_id, accounts),
        TransferOwnership => transfer_ownership(program_id, accounts),
        WithdrawTokens { amount } => {
            withdraw_from_vault(program_id, accounts, amount, TOKEN_VAULT_SEED)
        }
        SetTokenPrice { new_price } => set_token_price(program_id, accounts, new_price),
        CancelPendingPrice => cancel_pending_price(program_id, accounts),
        GuardianRecoverOwner => guardian_recover_owner(program_id, accounts),
        TransferTreasury => transfer_treasury(program_id, accounts),
        ClaimLockBonus => claim_lock_bonus(program_id, accounts),
        WithdrawBonusTokens { amount } => {
            withdraw_from_vault(program_id, accounts, amount, BONUS_VAULT_SEED)
        }
    }
}

/// Accounts:
/// 0. `[w]`  sale account (allocated by the client, owned by this program, zeroed)
/// 1. `[s]`  owner
/// 2. `[]`   treasury
/// 3. `[]`   guardian
/// 4. `[]`   token mint
/// 5. `[]`   token vault authority, PDA `["token_vault", sale]`
/// 6. `[]`   SOL vault, PDA `["sol_vault", sale]`
fn initialize(program_id: &Pubkey, accounts: &[AccountInfo], token_price: u64) -> ProgramResult {
    let it = &mut accounts.iter();
    let sale = next_account_info(it)?;
    let owner = next_account_info(it)?;
    let treasury = next_account_info(it)?;
    let guardian = next_account_info(it)?;
    let mint = next_account_info(it)?;
    let token_vault_auth = next_account_info(it)?;
    let sol_vault = next_account_info(it)?;

    require_signer(owner)?;
    if sale.owner != program_id {
        return Err(SaleError::InvalidAccountOwner.into());
    }
    if sale.data_len() < SaleState::LEN {
        return Err(ProgramError::AccountDataTooSmall);
    }
    if sale.try_borrow_data()?.iter().any(|b| *b != 0) {
        msg!("sale account is already initialized");
        return Err(ProgramError::AccountAlreadyInitialized);
    }

    let token_vault_bump = require_pda(
        token_vault_auth,
        &[TOKEN_VAULT_SEED, sale.key.as_ref()],
        program_id,
    )?;
    let sol_vault_bump = require_pda(sol_vault, &[SOL_VAULT_SEED, sale.key.as_ref()], program_id)?;

    pricing::check_price_bounds(token_price)?;

    if owner.key == treasury.key || owner.key == guardian.key || treasury.key == guardian.key {
        msg!("owner, treasury and guardian must be different keys");
        return Err(SaleError::Unauthorized.into());
    }

    SaleState {
        owner: *owner.key,
        treasury: *treasury.key,
        guardian: *guardian.key,
        token_mint: *mint.key,
        token_price_usd: token_price,
        pending_price_usd: 0,
        price_effective_at: 0,
        token_vault_bump,
        sol_vault_bump,
        total_collected: 0,
    }
    .save(sale)?;

    msg!("sale initialized, mint {} price {}", mint.key, token_price);
    Ok(())
}

/// Accounts:
/// 0. `[w]`  sale account
/// 1. `[]`   token vault authority (PDA)
/// 2. `[w]`  token vault
/// 3. `[w]`  buyer token account
/// 4. `[w]`  SOL vault (PDA)
/// 5. `[ws]` buyer
/// 6. `[]`   SPL token program
/// 7. `[]`   system program
/// 8. `[]`   Chainlink SOL/USD feed
/// 9. `[]`   Chainlink program
fn buy(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    lamports: u64,
    min_tokens_out: u64,
) -> ProgramResult {
    if !(MIN_PURCHASE_LAMPORTS..=MAX_PURCHASE_LAMPORTS).contains(&lamports) {
        msg!("purchase amount {} out of range", lamports);
        return Err(SaleError::InvalidAmount.into());
    }

    let it = &mut accounts.iter();
    let sale = next_account_info(it)?;
    let token_vault_auth = next_account_info(it)?;
    let token_vault = next_account_info(it)?;
    let buyer_tokens = next_account_info(it)?;
    let sol_vault = next_account_info(it)?;
    let buyer = next_account_info(it)?;
    let token_program = next_account_info(it)?;
    let system_program_acc = next_account_info(it)?;
    let price_feed = next_account_info(it)?;
    let chainlink_program = next_account_info(it)?;

    require_signer(buyer)?;
    let mut state = SaleState::load(sale, program_id)?;

    require_pda(
        token_vault_auth,
        &[TOKEN_VAULT_SEED, sale.key.as_ref()],
        program_id,
    )?;
    require_pda(sol_vault, &[SOL_VAULT_SEED, sale.key.as_ref()], program_id)?;
    require_key(
        system_program_acc,
        &system_program::id(),
        SaleError::InvalidProgramAccount,
    )?;
    require_key(
        token_program,
        &spl_token::id(),
        SaleError::InvalidProgramAccount,
    )?;

    let vault = token_account(token_vault)?;
    let dest = token_account(buyer_tokens)?;
    if vault.mint != state.token_mint || dest.mint != state.token_mint {
        msg!(
            "token account mint does not match sale mint {}",
            state.token_mint
        );
        return Err(SaleError::InvalidTokenMint.into());
    }
    if dest.owner != *buyer.key {
        return Err(SaleError::InvalidAccountOwner.into());
    }

    let now = Clock::get()?.unix_timestamp;
    let sol_usd = pricing::read_sol_price(chainlink_program, price_feed, now)?;

    if state.apply_pending_price(now) {
        msg!("new token price is active: {}", state.token_price_usd);
    }

    let tokens = pricing::calculate_tokens(sol_usd, lamports, state.token_price_usd)?;
    if tokens == 0 {
        return Err(SaleError::InvalidAmount.into());
    }
    if tokens < min_tokens_out {
        msg!("slippage: {} < {}", tokens, min_tokens_out);
        return Err(SaleError::SlippageExceeded.into());
    }

    invoke(
        &system_instruction::transfer(buyer.key, sol_vault.key, lamports),
        &[buyer.clone(), sol_vault.clone(), system_program_acc.clone()],
    )?;
    vault_transfer(
        token_program,
        token_vault,
        buyer_tokens,
        token_vault_auth,
        tokens,
        &[
            TOKEN_VAULT_SEED,
            sale.key.as_ref(),
            &[state.token_vault_bump],
        ],
    )?;

    state.total_collected = state
        .total_collected
        .checked_add(lamports)
        .ok_or(SaleError::Overflow)?;
    state.save(sale)
}

/// Sends everything above rent-exempt minimum from the SOL vault to `recipient`.
///
/// Accounts: sale `[w]`, treasury `[s]`, SOL vault `[w]`, recipient `[w]`, system program
fn withdraw_sol(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let it = &mut accounts.iter();
    let sale = next_account_info(it)?;
    let treasury = next_account_info(it)?;
    let sol_vault = next_account_info(it)?;
    let recipient = next_account_info(it)?;
    let system_program_acc = next_account_info(it)?;

    require_signer(treasury)?;
    let mut state = SaleState::load(sale, program_id)?;
    if state.treasury != *treasury.key {
        return Err(SaleError::Unauthorized.into());
    }

    require_pda(sol_vault, &[SOL_VAULT_SEED, sale.key.as_ref()], program_id)?;
    require_key(
        system_program_acc,
        &system_program::id(),
        SaleError::InvalidProgramAccount,
    )?;
    if sol_vault.owner != &system_program::id() {
        return Err(SaleError::InvalidAccountOwner.into());
    }
    // a wallet, not some PDA nobody can sign for
    if recipient.owner != &system_program::id() {
        return Err(SaleError::InvalidAccountOwner.into());
    }

    let amount = sol_vault
        .lamports()
        .checked_sub(Rent::get()?.minimum_balance(0))
        .filter(|a| *a > 0)
        .ok_or(SaleError::InvalidAmount)?;

    invoke_signed(
        &system_instruction::transfer(sol_vault.key, recipient.key, amount),
        &[
            sol_vault.clone(),
            recipient.clone(),
            system_program_acc.clone(),
        ],
        &[&[SOL_VAULT_SEED, sale.key.as_ref(), &[state.sol_vault_bump]]],
    )?;

    state.total_collected = 0;
    state.save(sale)
}

/// Read-only, logs the current rate. Handy for simulateTransaction.
///
/// Accounts: sale, Chainlink program, Chainlink feed
fn get_rate(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let it = &mut accounts.iter();
    let sale = next_account_info(it)?;
    let chainlink_program = next_account_info(it)?;
    let price_feed = next_account_info(it)?;

    let state = SaleState::load(sale, program_id)?;
    let now = Clock::get()?.unix_timestamp;
    let sol_usd = pricing::read_sol_price(chainlink_program, price_feed, now)?;

    let price = state.effective_price(now);
    let per_sol = pricing::calculate_tokens(sol_usd, LAMPORTS_PER_SOL, price)?;

    msg!("SOL/USD (1e{}): {}", PRICE_DECIMALS, sol_usd);
    msg!("token price (micro-USD): {}", price);
    msg!("tokens per SOL: {} ({} raw)", per_sol / TOKEN_UNIT, per_sol);
    if state.pending_price_usd != 0 && price != state.pending_price_usd {
        msg!(
            "pending price {} at {}",
            state.pending_price_usd,
            state.price_effective_at
        );
    }
    Ok(())
}

/// Accounts: sale `[w]`, current owner `[s]`, new owner
// TODO: two-step transfer (propose/accept) would be safer against typos
fn transfer_ownership(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let it = &mut accounts.iter();
    let sale = next_account_info(it)?;
    let owner = next_account_info(it)?;
    let new_owner = next_account_info(it)?;

    require_signer(owner)?;
    let mut state = SaleState::load(sale, program_id)?;
    if state.owner != *owner.key {
        return Err(SaleError::Unauthorized.into());
    }
    if *new_owner.key == state.treasury || *new_owner.key == state.guardian {
        msg!("new owner can't be treasury or guardian");
        return Err(SaleError::Unauthorized.into());
    }

    msg!("owner {} -> {}", state.owner, new_owner.key);
    state.owner = *new_owner.key;
    state.save(sale)
}

/// Used for both the sale vault (unsold tokens) and the bonus vault.
///
/// Accounts:
/// 0. `[]`   sale account
/// 1. `[s]`  treasury
/// 2. `[]`   vault authority (PDA for `seed`)
/// 3. `[w]`  vault token account
/// 4. `[w]`  recipient token account
/// 5. `[]`   SPL token program
/// 6. `[]`   token mint
fn withdraw_from_vault(
    program_id: &Pubkey,
    accounts: &[AccountInfo],
    amount: u64,
    seed: &[u8],
) -> ProgramResult {
    let it = &mut accounts.iter();
    let sale = next_account_info(it)?;
    let treasury = next_account_info(it)?;
    let vault_auth = next_account_info(it)?;
    let vault_acc = next_account_info(it)?;
    let recipient = next_account_info(it)?;
    let token_program = next_account_info(it)?;
    let mint = next_account_info(it)?;

    require_signer(treasury)?;
    if amount == 0 {
        return Err(SaleError::InvalidAmount.into());
    }

    let state = SaleState::load(sale, program_id)?;
    if state.treasury != *treasury.key {
        return Err(SaleError::Unauthorized.into());
    }
    require_key(mint, &state.token_mint, SaleError::InvalidTokenMint)?;
    require_key(
        token_program,
        &spl_token::id(),
        SaleError::InvalidProgramAccount,
    )?;
    let bump = require_pda(vault_auth, &[seed, sale.key.as_ref()], program_id)?;

    let vault = token_account(vault_acc)?;
    let dest = token_account(recipient)?;
    if vault.mint != state.token_mint || dest.mint != state.token_mint {
        return Err(SaleError::InvalidTokenMint.into());
    }
    if vault.owner != *vault_auth.key {
        return Err(SaleError::InvalidAccountOwner.into());
    }
    if amount > vault.amount {
        msg!("vault has {} tokens, requested {}", vault.amount, amount);
        return Err(SaleError::InvalidAmount.into());
    }

    vault_transfer(
        token_program,
        vault_acc,
        recipient,
        vault_auth,
        amount,
        &[seed, sale.key.as_ref(), &[bump]],
    )?;
    msg!("withdrew {} tokens to {}", amount, recipient.key);
    Ok(())
}

/// Price changes go through a timelock so buyers can see them coming.
/// The pending value is picked up by the first `buy` after `price_effective_at`.
///
/// Accounts: sale `[w]`, owner `[s]`
fn set_token_price(program_id: &Pubkey, accounts: &[AccountInfo], new_price: u64) -> ProgramResult {
    let it = &mut accounts.iter();
    let sale = next_account_info(it)?;
    let owner = next_account_info(it)?;

    require_signer(owner)?;
    let mut state = SaleState::load(sale, program_id)?;
    if state.owner != *owner.key {
        return Err(SaleError::Unauthorized.into());
    }
    pricing::check_price_bounds(new_price)?;

    let now = Clock::get()?.unix_timestamp;
    // a matured pending price must not be silently overwritten if nobody bought in between
    state.apply_pending_price(now);

    let effective_at = now
        .checked_add(PRICE_TIMELOCK_SECONDS)
        .ok_or(SaleError::Overflow)?;
    state.pending_price_usd = new_price;
    state.price_effective_at = effective_at;

    msg!(
        "price change queued: {} -> {} at {}",
        state.token_price_usd,
        new_price,
        effective_at
    );
    state.save(sale)
}

/// Owner or guardian can drop a queued price.
///
/// Accounts: sale `[w]`, owner or guardian `[s]`
fn cancel_pending_price(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let it = &mut accounts.iter();
    let sale = next_account_info(it)?;
    let signer = next_account_info(it)?;

    require_signer(signer)?;
    let mut state = SaleState::load(sale, program_id)?;
    if *signer.key != state.owner && *signer.key != state.guardian {
        return Err(SaleError::Unauthorized.into());
    }
    if state.pending_price_usd == 0 {
        return Err(SaleError::NoPendingPrice.into());
    }

    msg!(
        "pending price {} cancelled by {}",
        state.pending_price_usd,
        signer.key
    );
    state.clear_pending_price();
    state.save(sale)
}

/// Emergency path for a lost or leaked owner key. Guardian still can't touch funds.
///
/// Accounts: sale `[w]`, guardian `[s]`, new owner
fn guardian_recover_owner(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let it = &mut accounts.iter();
    let sale = next_account_info(it)?;
    let guardian = next_account_info(it)?;
    let new_owner = next_account_info(it)?;

    require_signer(guardian)?;
    let mut state = SaleState::load(sale, program_id)?;
    if state.guardian != *guardian.key {
        return Err(SaleError::Unauthorized.into());
    }
    if *new_owner.key == state.treasury || *new_owner.key == state.guardian {
        msg!("new owner can't be treasury or guardian");
        return Err(SaleError::Unauthorized.into());
    }

    msg!(
        "guardian replaced owner {} -> {}, dropped pending price {}",
        state.owner,
        new_owner.key,
        state.pending_price_usd
    );
    state.owner = *new_owner.key;
    // whatever the old owner queued is not trusted anymore
    state.clear_pending_price();
    state.save(sale)
}

/// Only the treasury rotates itself, owner has no say here.
///
/// Accounts: sale `[w]`, current treasury `[s]`, new treasury
fn transfer_treasury(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let it = &mut accounts.iter();
    let sale = next_account_info(it)?;
    let treasury = next_account_info(it)?;
    let new_treasury = next_account_info(it)?;

    require_signer(treasury)?;
    let mut state = SaleState::load(sale, program_id)?;
    if state.treasury != *treasury.key {
        return Err(SaleError::Unauthorized.into());
    }
    if *new_treasury.key == state.owner || *new_treasury.key == state.guardian {
        msg!("new treasury can't be owner or guardian");
        return Err(SaleError::Unauthorized.into());
    }

    msg!("treasury {} -> {}", state.treasury, new_treasury.key);
    state.treasury = *new_treasury.key;
    state.save(sale)
}

/// Pays BONUS_BPS of the locked amount to holders who lock tokens in an
/// irrevocable Streamflow cliff stream for at least a year. One claim per stream.
///
/// Accounts:
/// 0. `[]`   sale account
/// 1. `[w]`  claim marker, PDA `["bonus_claimed", stream]`
/// 2. `[]`   Streamflow stream
/// 3. `[]`   bonus vault authority, PDA `["bonus_vault", sale]`
/// 4. `[w]`  bonus vault token account
/// 5. `[w]`  recipient token account
/// 6. `[ws]` recipient (stream recipient, pays for the marker)
/// 7. `[]`   SPL token program
/// 8. `[]`   system program
fn claim_lock_bonus(program_id: &Pubkey, accounts: &[AccountInfo]) -> ProgramResult {
    let it = &mut accounts.iter();
    let sale = next_account_info(it)?;
    let marker = next_account_info(it)?;
    let stream = next_account_info(it)?;
    let bonus_auth = next_account_info(it)?;
    let bonus_vault = next_account_info(it)?;
    let recipient_tokens = next_account_info(it)?;
    let recipient = next_account_info(it)?;
    let token_program = next_account_info(it)?;
    let system_program_acc = next_account_info(it)?;

    require_signer(recipient)?;
    let state = SaleState::load(sale, program_id)?;
    require_key(
        system_program_acc,
        &system_program::id(),
        SaleError::InvalidProgramAccount,
    )?;
    require_key(
        token_program,
        &spl_token::id(),
        SaleError::InvalidProgramAccount,
    )?;

    if stream.owner != &STREAMFLOW_MAINNET && stream.owner != &STREAMFLOW_DEVNET {
        msg!("{} is not a Streamflow account", stream.key);
        return Err(SaleError::InvalidStream.into());
    }

    let bonus_bump = require_pda(
        bonus_auth,
        &[BONUS_VAULT_SEED, sale.key.as_ref()],
        program_id,
    )?;

    let dest = token_account(recipient_tokens)?;
    if dest.mint != state.token_mint {
        return Err(SaleError::InvalidTokenMint.into());
    }
    if dest.owner != *recipient.key {
        return Err(SaleError::InvalidAccountOwner.into());
    }
    let vault = token_account(bonus_vault)?;
    if vault.mint != state.token_mint {
        return Err(SaleError::InvalidTokenMint.into());
    }
    if vault.owner != *bonus_auth.key {
        return Err(SaleError::InvalidAccountOwner.into());
    }

    let lock = StreamInfo::parse(&stream.try_borrow_data()?)?;
    let now = Clock::get()?.unix_timestamp;
    lock.check_lock(recipient.key, &state.token_mint, now)?;

    let marker_bump = require_pda(
        marker,
        &[BONUS_CLAIMED_SEED, stream.key.as_ref()],
        program_id,
    )?;
    mark_claimed(
        marker,
        recipient,
        system_program_acc,
        program_id,
        &[BONUS_CLAIMED_SEED, stream.key.as_ref(), &[marker_bump]],
    )?;

    let bonus = pricing::lock_bonus(lock.net_deposited)?;
    if bonus == 0 {
        return Err(SaleError::InvalidAmount.into());
    }

    vault_transfer(
        token_program,
        bonus_vault,
        recipient_tokens,
        bonus_auth,
        bonus,
        &[BONUS_VAULT_SEED, sale.key.as_ref(), &[bonus_bump]],
    )?;

    msg!(
        "bonus {} paid for stream {} ({} locked)",
        bonus,
        stream.key,
        lock.net_deposited
    );
    Ok(())
}

// The marker address is known as soon as the stream exists, so anyone can send it
// a lamport first. create_account would then fail forever, hence top-up +
// allocate + assign. A prefunded marker is still system-owned with no data.
fn mark_claimed<'a>(
    marker: &AccountInfo<'a>,
    payer: &AccountInfo<'a>,
    system_program_acc: &AccountInfo<'a>,
    program_id: &Pubkey,
    seeds: &[&[u8]],
) -> ProgramResult {
    if marker.owner == program_id && marker.data_len() > 0 {
        msg!("bonus already claimed for this stream");
        return Err(SaleError::BonusAlreadyClaimed.into());
    }

    let rent = Rent::get()?.minimum_balance(1);
    let have = marker.lamports();
    if have < rent {
        invoke(
            &system_instruction::transfer(payer.key, marker.key, rent - have),
            &[payer.clone(), marker.clone(), system_program_acc.clone()],
        )?;
    }
    invoke_signed(
        &system_instruction::allocate(marker.key, 1),
        &[marker.clone(), system_program_acc.clone()],
        &[seeds],
    )?;
    invoke_signed(
        &system_instruction::assign(marker.key, program_id),
        &[marker.clone(), system_program_acc.clone()],
        &[seeds],
    )?;
    marker.try_borrow_mut_data()?[0] = 1;
    Ok(())
}
