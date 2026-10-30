# Solana Token Allocation Program

A native Solana program (no Anchor) for selling an SPL token for SOL at a fixed
USD price. The SOL/USD rate comes from a Chainlink feed. It also pays an optional
bonus to holders who lock their tokens in an irrevocable Streamflow stream.

The design focuses on the things that usually go wrong in token sales:
oracle handling, admin key compromise, price manipulation by the operator, and
replay or griefing of one-time rewards.

## Features

- **Oracle-priced sale.** Buyers send SOL and get tokens at `token_price_usd`,
  converted with a live Chainlink SOL/USD price. Stale or future-dated rounds are
  rejected, and only whitelisted feed accounts are accepted.
- **Slippage protection.** Every purchase carries `min_tokens_out`.
- **Three separated roles.** These are owner, treasury and guardian (see below).
  No single key can both change the price and take the money.
- **Timelocked price changes.** A new price waits 24h before it takes effect. The
  owner or guardian can cancel it while it is pending.
- **PDA vaults.** Sale tokens, bonus tokens and collected SOL sit in
  program-derived accounts. Only the program can sign for them.
- **Lock bonus.** Lock at least 1,000 tokens for at least a year in a
  non-cancelable, non-transferable Streamflow cliff stream, and you can claim a
  one-time 5% bonus from a separate bonus vault.
- **Overflow-safe math.** All amounts use `u128` intermediates with checked
  conversions. Purchase size is capped, so the formula can never get close to
  overflowing.

## Roles

| Role     | Intended key            | Can do                                                                 | Cannot do                         |
|----------|-------------------------|------------------------------------------------------------------------|-----------------------------------|
| Owner    | hot wallet              | queue / cancel price changes, transfer ownership                       | withdraw SOL or tokens            |
| Treasury | cold wallet / multisig  | withdraw SOL, withdraw unsold and bonus tokens, rotate itself          | change price                      |
| Guardian | offline key             | replace a compromised owner (drops their pending price), cancel price  | withdraw anything, change price   |

The three keys must be different. The program enforces this at initialization
and on every role rotation.

## Instructions

Instruction data starts with a one-byte tag. `u64` arguments follow in
little-endian order.

| Tag | Instruction              | Args                                 | Signer          |
|----:|--------------------------|--------------------------------------|-----------------|
| 0   | `Initialize`             | `token_price: u64` (micro-USD)       | owner           |
| 1   | `Buy`                    | `lamports: u64`, `min_tokens_out: u64` | buyer         |
| 2   | `WithdrawSol`            | –                                    | treasury        |
| 3   | `GetRate`                | – (read-only, logs the rate)         | –               |
| 4   | `TransferOwnership`      | –                                    | owner           |
| 5   | `WithdrawTokens`         | `amount: u64`                        | treasury        |
| 6   | `SetTokenPrice`          | `new_price: u64` (micro-USD)         | owner           |
| 7   | `CancelPendingPrice`     | –                                    | owner/guardian  |
| 8   | `GuardianRecoverOwner`   | –                                    | guardian        |
| 9   | `TransferTreasury`       | –                                    | treasury        |
| 10  | `ClaimLockBonus`         | –                                    | stream recipient|
| 11  | `WithdrawBonusTokens`    | `amount: u64`                        | treasury        |

Each handler in [`src/processor.rs`](src/processor.rs) documents its account list.

### Buy accounts

| # | Account                | Flags |
|---|------------------------|-------|
| 0 | sale state             | w     |
| 1 | token vault authority  |       |
| 2 | token vault            | w     |
| 3 | buyer token account    | w     |
| 4 | SOL vault              | w     |
| 5 | buyer                  | w, s  |
| 6 | SPL Token program      |       |
| 7 | System program         |       |
| 8 | Chainlink SOL/USD feed |       |
| 9 | Chainlink program      |       |

## Accounts and PDAs

| Account                | Derivation                          | Purpose                                |
|------------------------|-------------------------------------|----------------------------------------|
| Sale state             | regular account owned by the program | `SaleState`, 162 bytes                |
| Token vault authority  | `["token_vault", sale]`             | owns the token account with sale supply |
| SOL vault              | `["sol_vault", sale]`               | system account that collects SOL       |
| Bonus vault authority  | `["bonus_vault", sale]`             | owns the token account with bonus supply |
| Claim marker           | `["bonus_claimed", stream]`         | 1-byte flag, one per stream            |

```rust
pub struct SaleState {
    pub owner: Pubkey,
    pub treasury: Pubkey,
    pub guardian: Pubkey,
    pub token_mint: Pubkey,
    pub token_price_usd: u64,     // micro-USD
    pub pending_price_usd: u64,   // 0 = nothing queued
    pub price_effective_at: i64,
    pub token_vault_bump: u8,
    pub sol_vault_bump: u8,
    pub total_collected: u64,     // lamports since last withdraw
}
```

## Pricing

```
tokens = lamports * sol_usd / (100 * token_price_usd)
```

`sol_usd` has 8 decimals (Chainlink) and `token_price_usd` is in micro-USD (6
decimals). Both SOL and the token have 9 decimals, so those cancel out. The
6 vs 8 decimal mismatch leaves the factor of 100. The result is rounded down.

Example: 1 SOL at $200 with a token price of $0.10 gives 2,000 tokens.

Constraints:

- purchase must be between 0.01 and 100,000 SOL
- token price must be between $0.001 and $1000
- the oracle round must be at most 300 s old and not from the future

### Price timelock

`SetTokenPrice` stores the new value as `pending_price_usd` with
`price_effective_at = now + 24h`. Nothing needs to be sent to apply it. The first
`Buy` after that moment promotes it. If a newer change gets queued before any
buy happens, the matured one is promoted first, so it is never lost.

## Lock bonus

`ClaimLockBonus` reads the Streamflow `Contract` account directly and requires:

- the account is owned by the Streamflow program (mainnet or devnet ID)
- the layout fingerprint matches (`magic = 4`, `version = 4`), otherwise the claim is refused
- the signer is the stream recipient, and the mint is the sale mint
- the stream is not canceled, not cancelable, and not transferable by either side
- `net_amount_deposited >= 1,000 tokens`
- `end_time - start_time >= 365 days`
- the whole deposit is behind a cliff at `start_time + 365 days` or later (linear vesting doesn't count)
- `start_time` is within ±7 days of the claim, which rules out back-dated or far-future streams

On success, a marker PDA is created for the stream and 5% of the locked amount
is paid from the bonus vault. The treasury tops up the bonus vault with a plain
SPL transfer and can pull leftovers back with `WithdrawBonusTokens`.

The marker address is public as soon as the stream exists, so an attacker could
pre-fund it with lamports and make `create_account` fail forever. The program
avoids this: it tops the account up to rent-exempt, then uses
`allocate` + `assign`. An account counts as "claimed" only once it is owned by
the program and holds data.

## Security notes

- Every account is validated: program-owned state, PDA derivations, SPL Token
  ownership, mints matching the sale mint, buyer owns the destination token
  account, and fixed IDs for the System, Token and Chainlink programs.
- `Initialize` refuses a non-zero account, so state can't be re-initialized.
- Oracle age is checked with unsigned arithmetic, so a skewed clock can't wrap it.
- SOL withdrawals keep the vault rent-exempt and only go to system-owned accounts.
- A guardian recovery also clears any price queued by the old owner.
- Error codes are stable (`SaleError`, `Custom(0..=20)`), so clients can map them.

Known trade-off: ownership and treasury transfers are single-step.

## Project layout

```
src/
├── lib.rs           entrypoint
├── constants.rs     limits, seeds, external program IDs
├── error.rs         SaleError
├── instruction.rs   instruction decoding
├── processor.rs     instruction handlers
├── pricing.rs       oracle read, token math, bonus math
├── state.rs         SaleState + pending price logic
├── streamflow.rs    Streamflow account parser and lock rules
└── utils.rs         signer / PDA / token account helpers
```

## Build and test

Requirements: Rust stable and the Solana/Agave CLI (`cargo build-sbf`).

```bash
# unit tests (math, decoding, state, Streamflow rules)
cargo test

# on-chain build -> target/deploy/allocation_program.so
cargo build-sbf

# devnet build with a 2 minute price timelock
cargo build-sbf --features short-timelock
```

The `no-entrypoint` feature lets other crates use this one as a library.

## Deploy (devnet)

```bash
solana config set --url devnet
solana program deploy target/deploy/allocation_program.so
```

Then:

1. Create the sale state account (`SaleState::LEN` bytes, owner = program ID).
2. Create a token account for the sale mint, owned by the `token_vault` PDA, and
   fund it with the sale supply.
3. Send `Initialize` with the starting price.
4. Optionally create and fund the bonus vault (the token account of the
   `bonus_vault` PDA).

## Tech

Rust · solana-program 1.18 · spl-token 4 · Borsh · Chainlink Solana · Streamflow

## License

MIT
