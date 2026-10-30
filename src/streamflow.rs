//! Minimal reader for Streamflow `Contract` accounts (v4 layout, no Anchor discriminator).
//! Offsets were checked against a real devnet stream dump.

use solana_program::pubkey::Pubkey;

use crate::{constants::*, error::SaleError};

const MAGIC: usize = 0;
const VERSION: usize = 8;
const CANCELED_AT: usize = 25;
const END_TIME: usize = 33;
const RECIPIENT: usize = 113;
const MINT: usize = 177;
const START_TIME: usize = 409;
const NET_AMOUNT_DEPOSITED: usize = 417;
const CLIFF: usize = 441;
const CLIFF_AMOUNT: usize = 449;
const CANCELABLE_BY_SENDER: usize = 457;
const CANCELABLE_BY_RECIPIENT: usize = 458;
const TRANSFERABLE_BY_SENDER: usize = 460;
const TRANSFERABLE_BY_RECIPIENT: usize = 461;
const MIN_LEN: usize = 463;

const EXPECTED_MAGIC: u64 = 4;
const EXPECTED_VERSION: u8 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamInfo {
    pub recipient: Pubkey,
    pub mint: Pubkey,
    pub canceled_at: u64,
    pub start_time: u64,
    pub end_time: u64,
    pub net_deposited: u64,
    pub cliff: u64,
    pub cliff_amount: u64,
    pub cancelable: bool,
    pub transferable: bool,
}

fn u64_at(d: &[u8], off: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[off..off + 8]);
    u64::from_le_bytes(b)
}

fn key_at(d: &[u8], off: usize) -> Pubkey {
    let mut b = [0u8; 32];
    b.copy_from_slice(&d[off..off + 32]);
    Pubkey::new_from_array(b)
}

impl StreamInfo {
    pub fn parse(d: &[u8]) -> Result<Self, SaleError> {
        if d.len() < MIN_LEN {
            return Err(SaleError::InvalidStream);
        }
        // if Streamflow ever changes the layout we'd rather fail than read garbage
        if u64_at(d, MAGIC) != EXPECTED_MAGIC || d[VERSION] != EXPECTED_VERSION {
            return Err(SaleError::StreamflowVersionMismatch);
        }

        Ok(Self {
            recipient: key_at(d, RECIPIENT),
            mint: key_at(d, MINT),
            canceled_at: u64_at(d, CANCELED_AT),
            start_time: u64_at(d, START_TIME),
            end_time: u64_at(d, END_TIME),
            net_deposited: u64_at(d, NET_AMOUNT_DEPOSITED),
            cliff: u64_at(d, CLIFF),
            cliff_amount: u64_at(d, CLIFF_AMOUNT),
            cancelable: d[CANCELABLE_BY_SENDER] != 0 || d[CANCELABLE_BY_RECIPIENT] != 0,
            transferable: d[TRANSFERABLE_BY_SENDER] != 0 || d[TRANSFERABLE_BY_RECIPIENT] != 0,
        })
    }

    /// Checks the stream is a real, irrevocable lock that qualifies for the bonus.
    pub fn check_lock(&self, recipient: &Pubkey, mint: &Pubkey, now: i64) -> Result<(), SaleError> {
        if self.recipient != *recipient {
            return Err(SaleError::InvalidStream);
        }
        if self.mint != *mint {
            return Err(SaleError::InvalidTokenMint);
        }
        if self.canceled_at != 0 {
            return Err(SaleError::LockCanceled);
        }
        if self.cancelable {
            return Err(SaleError::LockCancelable);
        }
        // otherwise: claim bonus, transfer the stream to a fresh wallet, claim again
        if self.transferable {
            return Err(SaleError::LockTransferable);
        }
        if self.net_deposited < MIN_LOCK_AMOUNT {
            return Err(SaleError::LockTooSmall);
        }
        if self.end_time <= self.start_time {
            return Err(SaleError::InvalidStream);
        }
        if self.end_time - self.start_time < MIN_LOCK_DURATION_SECS {
            return Err(SaleError::LockTooShort);
        }

        // linear vesting unlocks from day one, so the whole deposit must sit behind the cliff
        let min_cliff = self
            .start_time
            .checked_add(MIN_LOCK_DURATION_SECS)
            .ok_or(SaleError::Overflow)?;
        if self.cliff_amount != self.net_deposited || self.cliff < min_cliff {
            return Err(SaleError::LockShapeInvalid);
        }

        // back-dated streams are already (partly) vested, far-future ones lock nothing yet
        if now < 0 {
            return Err(SaleError::InvalidStream);
        }
        if (now as u64).abs_diff(self.start_time) > MAX_LOCK_START_DRIFT_SECS {
            return Err(SaleError::LockStaleStart);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_700_000_000;
    const YEAR: u64 = MIN_LOCK_DURATION_SECS;

    struct Fixture {
        recipient: Pubkey,
        mint: Pubkey,
        data: Vec<u8>,
    }

    impl Fixture {
        // valid 1y cliff lock of MIN_LOCK_AMOUNT starting now
        fn new() -> Self {
            let mut f = Self {
                recipient: Pubkey::new_unique(),
                mint: Pubkey::new_unique(),
                data: vec![0u8; MIN_LEN + 37],
            };
            f.put_u64(MAGIC, EXPECTED_MAGIC);
            f.data[VERSION] = EXPECTED_VERSION;
            f.data[RECIPIENT..RECIPIENT + 32].copy_from_slice(f.recipient.as_ref());
            f.data[MINT..MINT + 32].copy_from_slice(f.mint.as_ref());
            f.put_u64(START_TIME, NOW);
            f.put_u64(END_TIME, NOW + YEAR);
            f.put_u64(CLIFF, NOW + YEAR);
            f.put_u64(NET_AMOUNT_DEPOSITED, MIN_LOCK_AMOUNT);
            f.put_u64(CLIFF_AMOUNT, MIN_LOCK_AMOUNT);
            f
        }

        fn put_u64(&mut self, off: usize, v: u64) {
            self.data[off..off + 8].copy_from_slice(&v.to_le_bytes());
        }

        fn check(&self) -> Result<(), SaleError> {
            StreamInfo::parse(&self.data)?.check_lock(&self.recipient, &self.mint, NOW as i64)
        }
    }

    #[test]
    fn parses_fields() {
        let f = Fixture::new();
        let s = StreamInfo::parse(&f.data).unwrap();
        assert_eq!(s.recipient, f.recipient);
        assert_eq!(s.mint, f.mint);
        assert_eq!(s.end_time - s.start_time, YEAR);
        assert!(!s.cancelable && !s.transferable);
    }

    #[test]
    fn valid_lock_passes() {
        assert_eq!(Fixture::new().check(), Ok(()));
    }

    #[test]
    fn rejects_unknown_layout() {
        let mut f = Fixture::new();
        f.data[VERSION] = 5;
        assert_eq!(f.check(), Err(SaleError::StreamflowVersionMismatch));

        let mut f = Fixture::new();
        f.put_u64(MAGIC, 1);
        assert_eq!(f.check(), Err(SaleError::StreamflowVersionMismatch));

        assert_eq!(
            StreamInfo::parse(&[0u8; 100]),
            Err(SaleError::InvalidStream)
        );
    }

    #[test]
    fn rejects_someone_elses_stream() {
        let f = Fixture::new();
        let s = StreamInfo::parse(&f.data).unwrap();
        assert_eq!(
            s.check_lock(&Pubkey::new_unique(), &f.mint, NOW as i64),
            Err(SaleError::InvalidStream)
        );
        assert_eq!(
            s.check_lock(&f.recipient, &Pubkey::new_unique(), NOW as i64),
            Err(SaleError::InvalidTokenMint)
        );
    }

    #[test]
    fn rejects_revocable_streams() {
        let mut f = Fixture::new();
        f.put_u64(CANCELED_AT, NOW);
        assert_eq!(f.check(), Err(SaleError::LockCanceled));

        for off in [CANCELABLE_BY_SENDER, CANCELABLE_BY_RECIPIENT] {
            let mut f = Fixture::new();
            f.data[off] = 1;
            assert_eq!(f.check(), Err(SaleError::LockCancelable));
        }
        for off in [TRANSFERABLE_BY_SENDER, TRANSFERABLE_BY_RECIPIENT] {
            let mut f = Fixture::new();
            f.data[off] = 1;
            assert_eq!(f.check(), Err(SaleError::LockTransferable));
        }
    }

    #[test]
    fn rejects_small_or_short_locks() {
        let mut f = Fixture::new();
        f.put_u64(NET_AMOUNT_DEPOSITED, MIN_LOCK_AMOUNT - 1);
        f.put_u64(CLIFF_AMOUNT, MIN_LOCK_AMOUNT - 1);
        assert_eq!(f.check(), Err(SaleError::LockTooSmall));

        let mut f = Fixture::new();
        f.put_u64(END_TIME, NOW + YEAR - 1);
        assert_eq!(f.check(), Err(SaleError::LockTooShort));

        let mut f = Fixture::new();
        f.put_u64(END_TIME, NOW);
        assert_eq!(f.check(), Err(SaleError::InvalidStream));
    }

    #[test]
    fn rejects_linear_or_partial_cliff() {
        let mut f = Fixture::new();
        f.put_u64(CLIFF_AMOUNT, 0);
        assert_eq!(f.check(), Err(SaleError::LockShapeInvalid));

        let mut f = Fixture::new();
        f.put_u64(CLIFF_AMOUNT, MIN_LOCK_AMOUNT / 2);
        assert_eq!(f.check(), Err(SaleError::LockShapeInvalid));

        let mut f = Fixture::new();
        f.put_u64(CLIFF, NOW + YEAR - 1);
        assert_eq!(f.check(), Err(SaleError::LockShapeInvalid));
    }

    #[test]
    fn start_time_must_be_close_to_now() {
        for start in [
            NOW - MAX_LOCK_START_DRIFT_SECS,
            NOW + MAX_LOCK_START_DRIFT_SECS,
        ] {
            let mut f = Fixture::new();
            f.put_u64(START_TIME, start);
            f.put_u64(END_TIME, start + YEAR);
            f.put_u64(CLIFF, start + YEAR);
            assert_eq!(f.check(), Ok(()));
        }
        for start in [
            NOW - MAX_LOCK_START_DRIFT_SECS - 1,
            NOW + MAX_LOCK_START_DRIFT_SECS + 1,
        ] {
            let mut f = Fixture::new();
            f.put_u64(START_TIME, start);
            f.put_u64(END_TIME, start + YEAR);
            f.put_u64(CLIFF, start + YEAR);
            assert_eq!(f.check(), Err(SaleError::LockStaleStart));
        }
    }
}
