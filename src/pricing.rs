use chainlink_solana as chainlink;
use solana_program::{account_info::AccountInfo, msg, program_error::ProgramError};

use crate::{constants::*, error::SaleError};

// tokens = lamports / 10^9 * (sol_usd / 10^8)    -> USD
//        / (token_price / 10^6)                  -> whole tokens
//        * 10^9                                  -> base units
// the 10^9 cancel out and 10^6 / 10^8 leaves a /100.
// Worst case numerator is 10^14 * 10^14 = 10^28, nowhere near u128::MAX.
pub fn calculate_tokens(sol_usd: i128, lamports: u64, token_price: u64) -> Result<u64, SaleError> {
    if sol_usd <= 0 {
        return Err(SaleError::InvalidPriceFeed);
    }
    if token_price == 0 {
        return Err(SaleError::InvalidTokenPrice);
    }

    let num = (lamports as u128)
        .checked_mul(sol_usd as u128)
        .ok_or(SaleError::Overflow)?;
    let den = 100u128 * token_price as u128;

    u64::try_from(num / den).map_err(|_| SaleError::Overflow)
}

pub fn check_price_bounds(price: u64) -> Result<(), SaleError> {
    if !(MIN_TOKEN_PRICE..=MAX_TOKEN_PRICE).contains(&price) {
        msg!(
            "token price {} outside {}..={}",
            price,
            MIN_TOKEN_PRICE,
            MAX_TOKEN_PRICE
        );
        return Err(SaleError::InvalidTokenPrice);
    }
    Ok(())
}

// done in u64 on purpose: i64 subtraction here can wrap if the clock or feed is weird
pub fn check_freshness(now: i64, feed_ts: u64) -> Result<(), SaleError> {
    if now < 0 {
        return Err(SaleError::InvalidPriceFeed);
    }
    let now = now as u64;
    if feed_ts > now {
        msg!(
            "price feed timestamp is in the future: {} > {}",
            feed_ts,
            now
        );
        return Err(SaleError::InvalidPriceFeed);
    }
    if now - feed_ts > MAX_PRICE_AGE_SECONDS {
        msg!("price feed is stale: {}s old", now - feed_ts);
        return Err(SaleError::StalePriceFeed);
    }
    Ok(())
}

/// Reads SOL/USD from Chainlink. Only the known program and feeds are accepted,
/// otherwise anyone could pass an account with a price of their choice.
pub fn read_sol_price<'a>(
    chainlink_program: &AccountInfo<'a>,
    feed: &AccountInfo<'a>,
    now: i64,
) -> Result<i128, ProgramError> {
    if chainlink_program.key != &CHAINLINK_PROGRAM_ID {
        return Err(SaleError::InvalidPriceFeed.into());
    }
    if feed.key != &SOL_USD_FEED_MAINNET && feed.key != &SOL_USD_FEED_DEVNET {
        return Err(SaleError::InvalidPriceFeed.into());
    }

    let round = chainlink::latest_round_data(chainlink_program.clone(), feed.clone())?;
    check_freshness(now, round.timestamp as u64)?;

    if round.answer <= 0 {
        return Err(SaleError::InvalidPriceFeed.into());
    }
    Ok(round.answer)
}

pub fn lock_bonus(locked: u64) -> Result<u64, SaleError> {
    let bonus = (locked as u128) * BONUS_BPS as u128 / 10_000;
    u64::try_from(bonus).map_err(|_| SaleError::Overflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_program::native_token::LAMPORTS_PER_SOL;

    const TEN_CENTS: u64 = 100_000;

    fn usd(v: u64) -> i128 {
        v as i128 * 10i128.pow(PRICE_DECIMALS)
    }

    #[test]
    fn one_sol_at_200_buys_2000_tokens() {
        let t = calculate_tokens(usd(200), LAMPORTS_PER_SOL, TEN_CENTS).unwrap();
        assert_eq!(t, 2_000 * TOKEN_UNIT);
    }

    #[test]
    fn scales_linearly() {
        let base = calculate_tokens(usd(200), LAMPORTS_PER_SOL, TEN_CENTS).unwrap();
        let half_sol = calculate_tokens(usd(200), LAMPORTS_PER_SOL / 2, TEN_CENTS).unwrap();
        let double_sol_price = calculate_tokens(usd(400), LAMPORTS_PER_SOL, TEN_CENTS).unwrap();
        let double_token_price =
            calculate_tokens(usd(200), LAMPORTS_PER_SOL, 2 * TEN_CENTS).unwrap();

        assert_eq!(half_sol * 2, base);
        assert_eq!(double_sol_price, base * 2);
        assert_eq!(double_token_price * 2, base);
    }

    #[test]
    fn rounds_down() {
        // $0.30 token: 200 / 0.3 = 666.666... tokens
        let t = calculate_tokens(usd(200), LAMPORTS_PER_SOL, 300_000).unwrap();
        assert_eq!(t, 666_666_666_666);
    }

    #[test]
    fn min_purchase_is_not_dust() {
        let t = calculate_tokens(usd(200), MIN_PURCHASE_LAMPORTS, TEN_CENTS).unwrap();
        assert_eq!(t, 20 * TOKEN_UNIT);
    }

    #[test]
    fn rejects_bad_oracle_or_price() {
        assert_eq!(
            calculate_tokens(0, LAMPORTS_PER_SOL, TEN_CENTS),
            Err(SaleError::InvalidPriceFeed)
        );
        assert_eq!(
            calculate_tokens(-5, LAMPORTS_PER_SOL, TEN_CENTS),
            Err(SaleError::InvalidPriceFeed)
        );
        assert_eq!(
            calculate_tokens(usd(200), LAMPORTS_PER_SOL, 0),
            Err(SaleError::InvalidTokenPrice)
        );
    }

    #[test]
    fn max_purchase_fits_at_realistic_prices() {
        // 100k SOL at $5000 -> 5 * 10^18 base units, still fits in u64
        assert!(calculate_tokens(usd(5_000), MAX_PURCHASE_LAMPORTS, TEN_CENTS).is_ok());
    }

    #[test]
    fn absurd_inputs_error_instead_of_wrapping() {
        let r = calculate_tokens(usd(1_000_000), MAX_PURCHASE_LAMPORTS, TEN_CENTS);
        assert_eq!(r, Err(SaleError::Overflow));
    }

    #[test]
    fn price_bounds() {
        assert!(check_price_bounds(MIN_TOKEN_PRICE).is_ok());
        assert!(check_price_bounds(MAX_TOKEN_PRICE).is_ok());
        assert!(check_price_bounds(0).is_err());
        assert!(check_price_bounds(MAX_TOKEN_PRICE + 1).is_err());

        let t = calculate_tokens(usd(200), LAMPORTS_PER_SOL, MAX_TOKEN_PRICE).unwrap();
        assert_eq!(t, 200_000_000); // 0.2 tokens
    }

    #[test]
    fn freshness() {
        let now = 1_700_000_000;
        assert!(check_freshness(now, now as u64).is_ok());
        assert!(check_freshness(now, now as u64 - MAX_PRICE_AGE_SECONDS).is_ok());
        assert_eq!(
            check_freshness(now, now as u64 - MAX_PRICE_AGE_SECONDS - 1),
            Err(SaleError::StalePriceFeed)
        );
        assert_eq!(
            check_freshness(now, now as u64 + 1),
            Err(SaleError::InvalidPriceFeed)
        );
        assert_eq!(check_freshness(-1, 0), Err(SaleError::InvalidPriceFeed));
    }

    #[test]
    fn bonus_is_five_percent() {
        assert_eq!(lock_bonus(MIN_LOCK_AMOUNT).unwrap(), 50 * TOKEN_UNIT);
        assert_eq!(lock_bonus(19).unwrap(), 0);
        assert!(lock_bonus(u64::MAX).is_ok());
    }
}
