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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub price: u64,
    pub volume: u64,
    pub imbalance: u64,
}

/// Indices of live orders by ascending price; at equal prices in arrival
/// order (the sort is stable).
fn sorted(o: &Orders, idx: &mut [u8; CAP]) -> usize {
    let mut k = 0;
    for i in 0..o.n {
        if !o.live[i] {
            continue;
        }
        let mut j = k;
        while j > 0 && o.price[idx[j - 1] as usize] > o.price[i] {
            idx[j] = idx[j - 1];
            j -= 1;
        }
        idx[j] = i as u8;
        k += 1;
    }
    k
}

/// Clearing price. volume == 0 means the sides did not cross.
pub fn find_price(o: &Orders, reference: u64, tick: u64) -> Outcome {
    let mut idx = [0u8; CAP];
    let k = sorted(o, &mut idx);

    let mut buy_total = 0u64;
    for &i in &idx[..k] {
        if o.side[i as usize] == BUY {
            buy_total = buy_total.saturating_add(o.qty[i as usize]);
        }
    }

    let mut best: Option<(u64, u64, u64)> = None; // (exec, imb, dist)
    let (mut lo, mut hi) = (0u64, 0u64);
    let mut sell_le = 0u64; // sells priced <= p
    let mut buy_below = 0u64; // buys priced < p
    let mut g = 0;
    while g < k {
        let p = o.price[idx[g] as usize];
        let mut end = g;
        let (mut gb, mut gs) = (0u64, 0u64);
        while end < k && o.price[idx[end] as usize] == p {
            let i = idx[end] as usize;
            if o.side[i] == BUY { gb += o.qty[i] } else { gs += o.qty[i] }
            end += 1;
        }
        sell_le = sell_le.saturating_add(gs);
        let buy_ge = buy_total - buy_below;
        let exec = buy_ge.min(sell_le);
        if exec > 0 {
            let imb = buy_ge.max(sell_le) - exec;
            let dist = p.abs_diff(reference);
            match best {
                None => {
                    best = Some((exec, imb, dist));
                    lo = p;
                    hi = p;
                }
                Some((be, bi, bd)) => {
                    let better = exec > be
                        || (exec == be && imb < bi)
                        || (exec == be && imb == bi && dist < bd);
                    if better {
                        best = Some((exec, imb, dist));
                        lo = p;
                        hi = p;
                    } else if exec == be && imb == bi && dist == bd {
                        hi = p; // ascending scan: lo is already lower
                    }
                }
            }
        }
        buy_below = buy_below.saturating_add(gb);
        g = end;
    }

    match best {
        None => Outcome { price: 0, volume: 0, imbalance: 0 },
        Some((exec, imb, _)) => {
            let price = if lo == hi { lo } else { ((lo + hi) / 2) / tick * tick };
            Outcome { price, volume: exec, imbalance: imb }
        }
    }
}

