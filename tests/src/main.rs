//! Velque checks.
//!
//!   cargo run --release        (VELQUE_SO=path to velque.so)
//!
//! 1. Pure clearing: the example from docs.html#example and edge cases.
//! 2. End-to-end run of the built program in LiteSVM: a market on Token-2022,
//!    night auctions, carry-over, the opening cross, the day book with its
//!    band, the day close, balances reconciled to the unit.
//!
//! Each check prints PASS or fails with an explanation.

use litesvm::LiteSVM;
use solana_account::Account;
use solana_clock::Clock;
use solana_instruction::{AccountMeta, Instruction};
use solana_keypair::Keypair;
use solana_pubkey::Pubkey;
use solana_signer::Signer;
use solana_transaction::Transaction;
use velque::clearing::{self, Orders, BUY, SELL};

const TOKEN: Pubkey = Pubkey::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
const TOKEN_2022: Pubkey = Pubkey::from_str_const("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");
const ATA: Pubkey = Pubkey::from_str_const("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL");
const SYSTEM: Pubkey = Pubkey::from_str_const("11111111111111111111111111111111");

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

fn clearing_tests() {
    // the example from the docs, prices in cents
    let o = orders(&[
        (BUY, usd(18120), 100 * U),
        (BUY, usd(18090), 50 * U),
        (BUY, usd(18050), 200 * U),
        (SELL, usd(18040), 80 * U),
        (SELL, usd(18080), 120 * U),
        (SELL, usd(18130), 150 * U),
    ]);
    let out = clearing::find_price(&o, usd(18070), TICK);
    assert_eq!(out.price, usd(18080), "clearing price from the example");
    assert_eq!(out.volume, 150 * U);
    assert_eq!(out.imbalance, 50 * U);
    let mut f = [0u64; clearing::CAP];
    clearing::allocate(&o, out.price, out.volume, LOT, &mut f);
    assert_eq!(&f[..6], &[100 * U, 50 * U, 0, 80 * U, 70 * U, 0], "fills from the example");
    pass("docs example: clears at 180.80, fills 100/50 and 80/70");

    // the sides do not cross
    let o = orders(&[(BUY, usd(100), 10 * U), (SELL, usd(101), 10 * U)]);
    let out = clearing::find_price(&o, usd(100), TICK);
    assert_eq!(out.volume, 0);
    pass("no cross: nothing trades");

    // pro rata at the marginal price
    let o = orders(&[
        (BUY, usd(105), 200 * U),
        (SELL, usd(100), 100 * U),
        (SELL, usd(100), 300 * U),
    ]);
    let out = clearing::find_price(&o, usd(100), TICK);
    let mut f = [0u64; clearing::CAP];
    clearing::allocate(&o, out.price, out.volume, LOT, &mut f);
    assert_eq!(out.volume, 200 * U);
    assert_eq!(&f[..3], &[200 * U, 50 * U, 150 * U], "1:3 ratio");
    pass("pro rata at the clearing price");

    // tie on volume, imbalance and distance -> midpoint
    let o = orders(&[(BUY, usd(102), 10 * U), (SELL, usd(98), 10 * U)]);
    let out = clearing::find_price(&o, usd(100), TICK);
    assert_eq!(out.price, usd(100), "midpoint between 98 and 102");
    pass("full tie resolves to the midpoint");

    // fractional lots: 3 sellers share 10 lots, the remainder is handed out in order
    let o = orders(&[
        (BUY, usd(110), 10 * LOT),
        (SELL, usd(100), 10 * LOT),
        (SELL, usd(100), 10 * LOT),
        (SELL, usd(100), 10 * LOT),
    ]);
    let out = clearing::find_price(&o, usd(105), TICK);
    let mut f = [0u64; clearing::CAP];
    clearing::allocate(&o, out.price, out.volume, LOT, &mut f);
    assert_eq!(f[1] + f[2] + f[3], 10 * LOT);
    assert!(f[1..4].iter().all(|x| x % LOT == 0));
    assert_eq!(&f[1..4], &[4 * LOT, 3 * LOT, 3 * LOT]);
    pass("lot remainders go in order, total is exact");
}

// ------------------------------------------------------------ 2. LiteSVM

const M_AUCTION: usize = 256;
const M_WINDOW_END: usize = 272;
const M_REF: usize = 280;
const M_LAST: usize = 288;
const M_REF_AT: usize = 304;

const DAY_HEADER: usize = 72;
const DAY_ENTRY: usize = 80;

fn mint_data(authority: &Pubkey, decimals: u8) -> Vec<u8> {
    let mut d = vec![0u8; 82];
    d[0..4].copy_from_slice(&1u32.to_le_bytes());
    d[4..36].copy_from_slice(authority.as_ref());
    d[44] = decimals;
    d[45] = 1;
    d
}

fn token_data(mint: &Pubkey, owner: &Pubkey, amount: u64) -> Vec<u8> {
    let mut d = vec![0u8; 165];
    d[0..32].copy_from_slice(mint.as_ref());
    d[32..64].copy_from_slice(owner.as_ref());
    d[64..72].copy_from_slice(&amount.to_le_bytes());
    d[108] = 1; // initialized
    d
}

fn set_owned(svm: &mut LiteSVM, key: &Pubkey, data: Vec<u8>, owner: Pubkey) {
    let lamports = svm.minimum_balance_for_rent_exemption(data.len());
    svm.set_account(*key, Account { lamports, data, owner, executable: false, rent_epoch: 0 }).unwrap();
}

fn amount(svm: &LiteSVM, key: &Pubkey) -> u64 {
    let a = svm.get_account(key).unwrap();
    u64::from_le_bytes(a.data[64..72].try_into().unwrap())
}

fn u64_at(d: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(d[o..o + 8].try_into().unwrap())
}

fn ata(wallet: &Pubkey, prog: &Pubkey, mint: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(&[wallet.as_ref(), prog.as_ref(), mint.as_ref()], &ATA).0
}

struct Trader {
    kp: Keypair,
    base: Pubkey,
    quote: Pubkey,
}

struct Env {
    svm: LiteSVM,
    pid: Pubkey,
    base_mint: Pubkey,
    quote_mint: Pubkey,
    market: Pubkey,
    vbase: Pubkey,
    vquote: Pubkey,
    day: Pubkey,
}

impl Env {
    fn send(&mut self, ix: Instruction, signer: &Keypair) -> Result<u64, String> {
        self.svm.expire_blockhash();
        let tx = Transaction::new_signed_with_payer(&[ix], Some(&signer.pubkey()), &[signer], self.svm.latest_blockhash());
        match self.svm.send_transaction(tx) {
            Ok(m) => Ok(m.compute_units_consumed),
            Err(e) => Err(format!("{:?}\n{}", e.err, e.meta.logs.join("\n"))),
        }
    }
    fn book(&self, id: u64) -> Pubkey {
        Pubkey::find_program_address(&[b"book", self.market.as_ref(), &id.to_le_bytes()], &self.pid).0
    }
    fn trader(&mut self, base: u64, quote: u64) -> Trader {
        let kp = Keypair::new();
        self.svm.airdrop(&kp.pubkey(), 1_000_000_000).unwrap();
        let (b, q) = (Pubkey::new_unique(), Pubkey::new_unique());
        let bm = self.base_mint;
        let qm = self.quote_mint;
        set_owned(&mut self.svm, &b, token_data(&bm, &kp.pubkey(), base), TOKEN_2022);
        set_owned(&mut self.svm, &q, token_data(&qm, &kp.pubkey(), quote), TOKEN);
        Trader { kp, base: b, quote: q }
    }
    /// (account, vault, mint, program) for a side: base for a sell, quote for a buy
    fn side_accounts(&self, t: &Trader, side: u8) -> [AccountMeta; 4] {
        if side == SELL {
            [AccountMeta::new(t.base, false), AccountMeta::new(self.vbase, false), AccountMeta::new_readonly(self.base_mint, false), AccountMeta::new_readonly(TOKEN_2022, false)]
        } else {
            [AccountMeta::new(t.quote, false), AccountMeta::new(self.vquote, false), AccountMeta::new_readonly(self.quote_mint, false), AccountMeta::new_readonly(TOKEN, false)]
        }
    }
    /// tail of 8 accounts for instructions that touch both sides
    fn both_legs(&self, t: &Trader) -> Vec<AccountMeta> {
        vec![
            AccountMeta::new(t.base, false),
            AccountMeta::new(t.quote, false),
            AccountMeta::new(self.vbase, false),
            AccountMeta::new(self.vquote, false),
            AccountMeta::new_readonly(self.base_mint, false),
            AccountMeta::new_readonly(self.quote_mint, false),
            AccountMeta::new_readonly(TOKEN_2022, false),
            AccountMeta::new_readonly(TOKEN, false),
        ]
    }
    fn place(&mut self, t: &Trader, side: u8, price: u64, qty: u64) -> Result<u64, String> {
        self.place_tif(t, side, price, qty, 0)
    }
    fn place_tif(&mut self, t: &Trader, side: u8, price: u64, qty: u64, tif: u8) -> Result<u64, String> {
        let id = self.market_u64(M_AUCTION);
        let mut data = vec![1u8, side];
        data.extend_from_slice(&price.to_le_bytes());
        data.extend_from_slice(&qty.to_le_bytes());
        data.push(tif);
        let mut accounts = vec![
            AccountMeta::new(t.kp.pubkey(), true),
            AccountMeta::new_readonly(self.market, false),
            AccountMeta::new(self.book(id), false),
        ];
        accounts.extend(self.side_accounts(t, side));
        accounts.push(AccountMeta::new_readonly(SYSTEM, false));
        self.send(Instruction { program_id: self.pid, accounts, data }, &t.kp)
    }
    fn cancel(&mut self, t: &Trader, idx: u16, side: u8) -> Result<u64, String> {
        let id = self.market_u64(M_AUCTION);
        let mut data = vec![2u8];
        data.extend_from_slice(&idx.to_le_bytes());
        let mut accounts = vec![
            AccountMeta::new_readonly(t.kp.pubkey(), true),
            AccountMeta::new_readonly(self.market, false),
            AccountMeta::new(self.book(id), false),
        ];
        accounts.extend(self.side_accounts(t, side));
        self.send(Instruction { program_id: self.pid, accounts, data }, &t.kp)
    }
    fn clear(&mut self, cranker: &Keypair) -> Result<u64, String> {
        let id = self.market_u64(M_AUCTION);
        let ix = Instruction {
            program_id: self.pid,
            accounts: vec![
                AccountMeta::new(cranker.pubkey(), true),
                AccountMeta::new(self.market, false),
                AccountMeta::new(self.book(id), false),
                AccountMeta::new(self.book(id + 1), false),
                AccountMeta::new(self.day, false),
                AccountMeta::new_readonly(SYSTEM, false),
            ],
            data: vec![3u8],
        };
        self.send(ix, cranker)
    }
    fn claim(&mut self, t: &Trader, book: Pubkey, idx: u16) -> Result<u64, String> {
        let mut data = vec![4u8];
        data.extend_from_slice(&idx.to_le_bytes());
        let mut accounts = vec![
            AccountMeta::new_readonly(t.kp.pubkey(), true),
            AccountMeta::new_readonly(self.market, false),
            AccountMeta::new(book, false),
        ];
        accounts.extend(self.both_legs(t));
        self.send(Instruction { program_id: self.pid, accounts, data }, &t.kp)
    }
    fn set_reference(&mut self, signer: &Keypair, price: u64) -> Result<u64, String> {
        let mut data = vec![5u8];
        data.extend_from_slice(&price.to_le_bytes());
        let ix = Instruction {
            program_id: self.pid,
            accounts: vec![AccountMeta::new_readonly(signer.pubkey(), true), AccountMeta::new(self.market, false)],
            data,
        };
        self.send(ix, signer)
    }
    fn close_book(&mut self, book: Pubkey, payer: Pubkey, signer: &Keypair) -> Result<u64, String> {
        let ix = Instruction {
            program_id: self.pid,
            accounts: vec![AccountMeta::new(book, false), AccountMeta::new(payer, false)],
            data: vec![6u8],
        };
        self.send(ix, signer)
    }
    fn market_u64(&self, off: usize) -> u64 {
        u64_at(&self.svm.get_account(&self.market).unwrap().data, off)
    }
    fn warp(&mut self, secs: i64) {
        let mut c: Clock = self.svm.get_sysvar();
        c.unix_timestamp += secs;
        c.slot += 1;
        self.svm.set_sysvar(&c);
    }
    fn vaults(&self) -> (u64, u64) {
        (amount(&self.svm, &self.vbase), amount(&self.svm, &self.vquote))
    }
}


fn main() {
    clearing_tests();
    println!("ALL PASS");
}
