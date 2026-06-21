//! Velque v3 account layout. Fixed offsets, no serializers.
//!
//! Price is always quote units per one whole base token (10^decimals base).
//!
//! ## Window book, PDA ["book", market, auction_id le], tag 7
//!
//! A header of HEADER bytes, then CAP entries of ENTRY bytes each.
//!
//! | off | len | header                                           |
//! |-----|-----|--------------------------------------------------|
//! | 0   | 1   | tag                                              |
//! | 1   | 1   | bump                                             |
//! | 2   | 1   | state: 0 open, 1 cleared                         |
//! | 4   | 2   | count                                            |
//! | 8   | 32  | market                                           |
//! | 40  | 8   | auction_id                                       |
//! | 48  | 8   | window_end                                       |
//! | 56  | 8   | clear_price (0 if the sides did not cross)       |
//! | 64  | 8   | volume                                           |
//! | 72  | 8   | imbalance                                        |
//! | 80  | 8   | reference used to break ties                     |
//! | 88  | 8   | cleared_at (unix)                                |
//! | 96  | 32  | payer: who paid the book's rent                  |
//!
//! | off | len | window entry                                     |
//! |-----|-----|--------------------------------------------------|
//! | 0   | 32  | owner                                            |
//! | 32  | 8   | price                                            |
//! | 40  | 8   | qty in this window                               |
//! | 48  | 8   | filled                                           |
//! | 56  | 8   | escrow (quote for a buy, base for a sell)        |
//! | 64  | 1   | side: 0 buy, 1 sell                              |
//! | 65  | 1   | status: 1 live, 2 cancelled, 3 claimed           |
//! | 66  | 1   | tif: 0 one window, 1 until cancelled             |
//! | 67  | 1   | internal: 1 = the remainder carries over         |
//! | 72  | 8   | internal: escrow that moves with the remainder   |
//!

use crate::clearing::CAP;
use pinocchio::Address;

pub const HEADER: usize = 128;
pub const ENTRY: usize = 80;
pub const BOOK_LEN: usize = HEADER + CAP * ENTRY;
pub const BOOK_TAG: u8 = 7;
pub const TIF_ONE: u8 = 0;
pub const TIF_GTC: u8 = 1;

pub const LIVE: u8 = 1;
pub const CANCELLED: u8 = 2;
pub const CLAIMED: u8 = 3;

pub fn get_u64(d: &[u8], off: usize) -> u64 {
    let mut b = [0u8; 8];
    b.copy_from_slice(&d[off..off + 8]);
    u64::from_le_bytes(b)
}
pub fn get_i64(d: &[u8], off: usize) -> i64 {
    get_u64(d, off) as i64
}
pub fn put_u64(d: &mut [u8], off: usize, v: u64) {
    d[off..off + 8].copy_from_slice(&v.to_le_bytes());
}
pub fn put_i64(d: &mut [u8], off: usize, v: i64) {
    put_u64(d, off, v as u64)
}
pub fn get_addr(d: &[u8], off: usize) -> Address {
    let mut b = [0u8; 32];
    b.copy_from_slice(&d[off..off + 32]);
    Address::new_from_array(b)
}
pub fn put_addr(d: &mut [u8], off: usize, a: &Address) {
    d[off..off + 32].copy_from_slice(a.as_ref());
}

pub mod b {
    pub const BUMP: usize = 1;
    pub const STATE: usize = 2;
    pub const COUNT: usize = 4;
    pub const MARKET: usize = 8;
    pub const AUCTION_ID: usize = 40;
    pub const WINDOW_END: usize = 48;
    pub const CLEAR_PRICE: usize = 56;
    pub const VOLUME: usize = 64;
    pub const IMBALANCE: usize = 72;
    pub const REFERENCE: usize = 80;
    pub const CLEARED_AT: usize = 88;
    pub const PAYER: usize = 96;
}

pub mod e {
    pub const OWNER: usize = 0;
    pub const PRICE: usize = 32;
    pub const QTY: usize = 40;
    pub const FILLED: usize = 48;
    pub const ESCROW: usize = 56;
    pub const SIDE: usize = 64;
    pub const STATUS: usize = 65;
    pub const TIF: usize = 66;
    pub const ROLL: usize = 67;
    pub const ROLL_ESCROW: usize = 72;
}

pub fn entry_off(i: usize) -> usize {
    HEADER + i * ENTRY
}

pub fn count(book: &[u8]) -> usize {
    u16::from_le_bytes([book[b::COUNT], book[b::COUNT + 1]]) as usize
}
pub fn set_count(book: &mut [u8], n: usize) {
    book[b::COUNT..b::COUNT + 2].copy_from_slice(&(n as u16).to_le_bytes());
}
