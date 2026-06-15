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
