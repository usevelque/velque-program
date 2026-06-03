//! Clearing of a single auction. Pure logic with no accounts, so it can be
//! checked by host-side tests and executed the exact same way in the program.
//!
//! Price rule (docs.html#auctions):
//!   1. the price at which the most shares change hands;
//!   2. on a tie, the smaller remainder (imbalance);
//!   3. on a tie, the one closer to the last reference price;
//!   4. on a tie, the midpoint of the remaining range, rounded down to the tick.
//!
//! Allocation: orders at a better price fill in full first; at the marginal
//! price the remainder is split pro rata by size, in lot multiples, and the
//! tail goes out lot by lot in arrival order.
//!
//! Orders are sorted by price once (insertion sort, there are at most CAP of
//! them), then everything is computed in one pass: demand at price p is the
//! buys priced >= p, supply is the sells priced <= p. This way a full book
//! clears within the default compute limit.

pub const CAP: usize = 64;
pub const BUY: u8 = 0;
pub const SELL: u8 = 1;

/// The window's orders as clearing sees them.
pub struct Orders {
    pub n: usize,
    pub price: [u64; CAP],
    pub qty: [u64; CAP],
    pub side: [u8; CAP],
    pub live: [bool; CAP],
}

impl Orders {
    pub const fn empty() -> Self {
        Self { n: 0, price: [0; CAP], qty: [0; CAP], side: [0; CAP], live: [false; CAP] }
    }
}

