//! Velque checks.

use velque::clearing::{self, Orders, BUY, SELL};

/// 6 decimals for both tokens: 1 share = 1_000_000, 1 USDC = 1_000_000.
const U: u64 = 1_000_000;
const TICK: u64 = 10_000; // $0.01
const LOT: u64 = 1_000; // 0.001 share

fn usd(cents: u64) -> u64 {
    cents * TICK
}

// ------------------------------------------------------------ 1. clearing

fn orders(list: &[(u8, u64, u64)]) -> Orders {
    let mut o = Orders::empty();
    for (i, (side, price, qty)) in list.iter().enumerate() {
        o.side[i] = *side;
        o.price[i] = *price;
        o.qty[i] = *qty;
        o.live[i] = true;
    }
    o.n = list.len();
    o
}

fn pass(name: &str) {
    println!("PASS  {name}");
}

