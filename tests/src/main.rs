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
    fn place_day(&mut self, t: &Trader, side: u8, price: u64, qty: u64) -> Result<u64, String> {
        let mut data = vec![7u8, side];
        data.extend_from_slice(&price.to_le_bytes());
        data.extend_from_slice(&qty.to_le_bytes());
        let mut accounts = vec![
            AccountMeta::new(t.kp.pubkey(), true),
            AccountMeta::new(self.market, false),
            AccountMeta::new(self.day, false),
        ];
        accounts.extend(self.both_legs(t));
        accounts.push(AccountMeta::new_readonly(SYSTEM, false));
        self.send(Instruction { program_id: self.pid, accounts, data }, &t.kp)
    }
    fn day_exit(&mut self, t: &Trader, idx: u16, cancel: bool) -> Result<u64, String> {
        let mut data = vec![if cancel { 8u8 } else { 9u8 }];
        data.extend_from_slice(&idx.to_le_bytes());
        let mut accounts = vec![
            AccountMeta::new_readonly(t.kp.pubkey(), true),
            AccountMeta::new_readonly(self.market, false),
            AccountMeta::new(self.day, false),
        ];
        accounts.extend(self.both_legs(t));
        self.send(Instruction { program_id: self.pid, accounts, data }, &t.kp)
    }
    fn close_day(&mut self, cranker: &Keypair) -> Result<u64, String> {
        let id = self.market_u64(M_AUCTION);
        let ix = Instruction {
            program_id: self.pid,
            accounts: vec![
                AccountMeta::new(cranker.pubkey(), true),
                AccountMeta::new_readonly(self.market, false),
                AccountMeta::new(self.day, false),
                AccountMeta::new(self.book(id), false),
                AccountMeta::new_readonly(SYSTEM, false),
            ],
            data: vec![10u8],
        };
        self.send(ix, cranker)
    }
    fn market_u64(&self, off: usize) -> u64 {
        u64_at(&self.svm.get_account(&self.market).unwrap().data, off)
    }
    /// (owner, price, qty, escrow, owed, side, status) of a day book slot
    fn day_slot(&self, i: usize) -> (Pubkey, u64, u64, u64, u64, u8, u8) {
        let d = self.svm.get_account(&self.day).unwrap().data;
        let o = DAY_HEADER + i * DAY_ENTRY;
        (
            Pubkey::new_from_array(d[o..o + 32].try_into().unwrap()),
            u64_at(&d, o + 32),
            u64_at(&d, o + 40),
            u64_at(&d, o + 48),
            u64_at(&d, o + 56),
            d[o + 72],
            d[o + 73],
        )
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

fn expect_err(r: Result<u64, String>, code: u32, what: &str) {
    let e = r.expect_err(what);
    assert!(e.contains(&format!("Custom({code})")), "{what}: expected Custom({code}), got {e}");
}

fn svm_tests(so: &str) {
    let pid = Pubkey::new_unique();
    let mut svm = LiteSVM::new();
    svm.add_program_from_file(pid, so).unwrap();

    let admin = Keypair::new();
    svm.airdrop(&admin.pubkey(), 10_000_000_000).unwrap();
    // the stock is on Token-2022, like real xStocks; USDC is on plain SPL Token
    let (base_mint, quote_mint) = (Pubkey::new_unique(), Pubkey::new_unique());
    set_owned(&mut svm, &base_mint, mint_data(&admin.pubkey(), 6), TOKEN_2022);
    set_owned(&mut svm, &quote_mint, mint_data(&admin.pubkey(), 6), TOKEN);
    let market = Pubkey::find_program_address(&[b"market", base_mint.as_ref()], &pid).0;
    let vbase = ata(&market, &TOKEN_2022, &base_mint);
    let vquote = ata(&market, &TOKEN, &quote_mint);
    let day = Pubkey::find_program_address(&[b"day", market.as_ref()], &pid).0;
    let mut env = Env { svm, pid, base_mint, quote_mint, market, vbase, vquote, day };

    // market: 60 s window, $0.01 tick, 0.001 lot, reference 180.70,
    // reference fresh for 300 s, day book band 5%, minimum order $10
    let mut data = vec![0u8];
    for v in [60u64, TICK, LOT, usd(18070), 300, 500, 10 * U] {
        data.extend_from_slice(&v.to_le_bytes());
    }
    let ix = Instruction {
        program_id: pid,
        accounts: vec![
            AccountMeta::new(admin.pubkey(), true),
            AccountMeta::new(market, false),
            AccountMeta::new_readonly(base_mint, false),
            AccountMeta::new_readonly(quote_mint, false),
            AccountMeta::new(vbase, false),
            AccountMeta::new(vquote, false),
            AccountMeta::new_readonly(TOKEN_2022, false),
            AccountMeta::new_readonly(TOKEN, false),
            AccountMeta::new_readonly(ATA, false),
            AccountMeta::new_readonly(SYSTEM, false),
        ],
        data,
    };
    env.send(ix, &admin).expect("init_market");
    assert_eq!(env.market_u64(M_REF), usd(18070));
    assert_eq!(env.market_u64(M_REF_AT), 0, "starts in Dark");
    assert_eq!(env.svm.get_account(&vbase).unwrap().owner, TOKEN_2022, "stock vault is on Token-2022");
    assert_eq!(env.svm.get_account(&vquote).unwrap().owner, TOKEN);
    pass("init_market: Token-2022 stock, SPL USDC, both vaults are market ATAs");

    // ---------------------------------------------------------------- Dark

    let bids = [(18120u64, 100u64), (18090, 50), (18050, 200)];
    let asks = [(18040u64, 80u64), (18080, 120), (18130, 150)];
    let buyers: Vec<Trader> = bids.iter().map(|_| env.trader(0, 100_000 * U)).collect();
    let sellers: Vec<Trader> = asks.iter().map(|_| env.trader(1_000 * U, 0)).collect();
    let quitter = env.trader(0, 100_000 * U);

    let mut max_cu = 0u64;
    for (t, (p, q)) in buyers.iter().zip(bids) {
        max_cu = max_cu.max(env.place(t, BUY, usd(p), q * U).expect("buy"));
    }
    for (t, (p, q)) in sellers.iter().zip(asks) {
        max_cu = max_cu.max(env.place(t, SELL, usd(p), q * U).expect("sell"));
    }
    assert_eq!(amount(&env.svm, &buyers[0].quote), 100_000 * U - 18_120 * U);
    assert_eq!(amount(&env.svm, &env.vbase), 350 * U);
    pass("place: 6 orders from the docs example, funds in escrow");

    env.place(&quitter, BUY, usd(18100), 10 * U).expect("quitter buy");
    env.cancel(&quitter, 6, BUY).expect("cancel");
    assert_eq!(amount(&env.svm, &quitter.quote), 100_000 * U);
    assert!(env.cancel(&quitter, 6, BUY).is_err(), "double cancel");
    assert!(env.cancel(&buyers[1], 0, BUY).is_err(), "someone else's order");
    pass("cancel: full refund, no double cancel, only the owner");

    let cranker = Keypair::new();
    env.svm.airdrop(&cranker.pubkey(), 1_000_000_000).unwrap();
    expect_err(env.clear(&cranker), 5, "clear on an open window");
    pass("clear before the window ends is rejected");

    env.warp(61);
    assert!(env.place(&buyers[0], BUY, usd(18100), U).is_err(), "order into a closed window");
    let book0 = env.book(0);
    let cu = env.clear(&cranker).expect("clear");
    let bd = env.svm.get_account(&book0).unwrap().data;
    assert_eq!(bd[2], 1, "book cleared");
    assert_eq!(u64_at(&bd, 56), usd(18080), "clearing price");
    assert_eq!(u64_at(&bd, 64), 150 * U, "volume");
    assert_eq!(env.market_u64(M_AUCTION), 1, "next window is open");
    assert_eq!(env.market_u64(M_LAST), usd(18080));
    pass(&format!("clear: 180.80 for 150 shares, {cu} CU"));

    for (i, t) in buyers.iter().enumerate() {
        env.claim(t, book0, i as u16).expect("claim buy");
    }
    for (i, t) in sellers.iter().enumerate() {
        env.claim(t, book0, (i + 3) as u16).expect("claim sell");
    }
    assert!(env.claim(&buyers[0], book0, 0).is_err(), "double claim");
    let q0 = 100_000 * U;
    assert_eq!(amount(&env.svm, &buyers[0].base), 100 * U);
    assert_eq!(amount(&env.svm, &buyers[0].quote), q0 - 18_080 * U, "pays 180.80, not 181.20");
    assert_eq!(amount(&env.svm, &buyers[1].base), 50 * U);
    assert_eq!(amount(&env.svm, &buyers[1].quote), q0 - 9_040 * U);
    assert_eq!(amount(&env.svm, &buyers[2].base), 0);
    assert_eq!(amount(&env.svm, &buyers[2].quote), q0, "not filled, funds returned");
    assert_eq!(amount(&env.svm, &sellers[0].quote), 80 * 18_080 * U / 100);
    assert_eq!(amount(&env.svm, &sellers[0].base), 920 * U);
    assert_eq!(amount(&env.svm, &sellers[1].quote), 70 * 18_080 * U / 100);
    assert_eq!(amount(&env.svm, &sellers[1].base), 1_000 * U - 70 * U);
    assert_eq!(amount(&env.svm, &sellers[2].base), 1_000 * U);
    assert_eq!(env.vaults(), (0, 0), "both vaults are empty");
    pass("claim: every balance matches the docs, both vaults end at zero");

    // carry-over: a GTC buy of 100 @ 180.00 fills 40, the remaining 60 rolls into window 2
    let g = env.trader(0, 100_000 * U);
    let s1 = env.trader(1_000 * U, 0);
    let s2 = env.trader(1_000 * U, 0);
    env.place_tif(&g, BUY, usd(18000), 100 * U, 1).expect("gtc buy");
    env.place(&s1, SELL, usd(17950), 40 * U).expect("s1");
    env.warp(61);
    env.clear(&cranker).expect("clear 1");
    let b1 = env.book(1);
    let b2 = env.book(2);
    let d1 = env.svm.get_account(&b1).unwrap().data;
    assert_eq!(u64_at(&d1, 56), usd(18000), "window 1 at 180.00");
    assert_eq!(u64_at(&d1, 128 + 48), 40 * U, "filled 40");
    assert_eq!(u64_at(&d1, 128 + 56), 7_200 * U, "the old entry keeps escrow for exactly 40 shares");
    let d2 = env.svm.get_account(&b2).unwrap().data;
    assert_eq!(u16::from_le_bytes([d2[4], d2[5]]), 1, "window 2 holds one carried-over order");
    assert_eq!(u64_at(&d2, 128 + 40), 60 * U, "remainder 60");
    assert_eq!(u64_at(&d2, 128 + 56), 10_800 * U, "remainder escrow");
    assert_eq!(d2[128 + 66], 1, "a carried-over order stays GTC");
    pass("rollover: GTC remainder moves to the next window with exact escrow");

    env.place(&s2, SELL, usd(17900), 60 * U).expect("s2");
    env.warp(61);
    env.clear(&cranker).expect("clear 2");
    let d2 = env.svm.get_account(&b2).unwrap().data;
    assert_eq!(u64_at(&d2, 56), usd(18000));
    assert_eq!(u64_at(&d2, 128 + 48), 60 * U);
    assert!(env.svm.get_account(&env.book(3)).map(|a| a.data.is_empty()).unwrap_or(true), "nothing to carry over, book 3 is not created");
    env.claim(&g, b1, 0).expect("g claim 1");
    env.claim(&g, b2, 0).expect("g claim 2");
    env.claim(&s1, b1, 1).expect("s1 claim");
    env.claim(&s2, b2, 1).expect("s2 claim");
    assert_eq!(amount(&env.svm, &g.base), 100 * U);
    assert_eq!(amount(&env.svm, &g.quote), 100_000 * U - 18_000 * U, "exactly 18,000 for 100 shares");
    assert_eq!(amount(&env.svm, &s1.quote), 7_200 * U);
    assert_eq!(amount(&env.svm, &s2.quote), 10_800 * U);
    assert_eq!(env.vaults(), (0, 0));
    pass("rollover: fills across two windows settle to the unit, vaults at zero");

    let lone = env.trader(0, 100_000 * U);
    env.place_tif(&lone, BUY, usd(15000), 10 * U, 1).expect("lone gtc");
    env.warp(61);
    env.clear(&cranker).expect("clear 3");
    env.cancel(&lone, 0, BUY).expect("cancel rolled");
    assert_eq!(amount(&env.svm, &lone.quote), 100_000 * U, "a carried-over order cancels with no loss");
    pass("rollover: an unmatched GTC order rolls whole and cancels in full");

    let b3 = env.book(3);
    let payer3 = Pubkey::new_from_array(env.svm.get_account(&b3).unwrap().data[96..128].try_into().unwrap());
    assert_eq!(payer3, lone.kp.pubkey(), "book 3 was opened by lone's order");
    assert!(env.close_book(b3, cranker.pubkey(), &cranker).is_err(), "wrong rent recipient");
    env.close_book(b3, payer3, &cranker).expect("close book 3");
    assert!(env.svm.get_account(&b3).map(|a| a.lamports == 0).unwrap_or(true), "book 3 is closed");
    let p4 = Pubkey::new_from_array(env.svm.get_account(&env.book(4)).unwrap().data[96..128].try_into().unwrap());
    assert_eq!(p4, cranker.pubkey(), "the carry-over book was paid for by the cranker");
    pass("close_book: settled books close, rent goes back to whoever paid it");

    env.warp(61);
    env.clear(&cranker).expect("empty clear");
    assert_eq!(env.market_u64(M_AUCTION), 5);
    pass("an empty window rolls over");

    // ---------------------------------------------------------------- opening cross

    // night: a GTC buy of 30 @ 181.00 and a sell of 20 @ 180.50 wait in window 5
    let night_buy = env.trader(0, 100_000 * U);
    let night_sell = env.trader(1_000 * U, 0);
    env.place_tif(&night_buy, BUY, usd(18100), 30 * U, 1).expect("night buy");
    env.place(&night_sell, SELL, usd(18050), 20 * U).expect("night sell");
    expect_err(env.place_day(&night_buy, BUY, usd(18100), U), 13, "day order in Dark");
    pass("dark: the day book is closed, place_day is rejected");

    // oracle: a stranger cannot, the authority can; a fresh reference means Day
    let stranger = Keypair::new();
    env.svm.airdrop(&stranger.pubkey(), 1_000_000_000).unwrap();
    expect_err(env.set_reference(&stranger, usd(18080)), 11, "stranger as oracle");
    env.set_reference(&admin, usd(18080)).expect("set_reference");
    expect_err(env.place(&night_sell, SELL, usd(18050), U), 12, "auction order during Day");
    pass("set_reference: only the authority; a fresh reference switches the market to Day");

    // window 5 has not ended yet, but during Day clearing a book with orders is the cross
    assert!((env.market_u64(M_WINDOW_END) as i64) > env.svm.get_sysvar::<Clock>().unix_timestamp, "window still open");
    let b5 = env.book(5);
    let cu = env.clear(&cranker).expect("opening cross");
    let d5 = env.svm.get_account(&b5).unwrap().data;
    assert_eq!(u64_at(&d5, 64), 20 * U, "cross: 20 shares");
    let cross_price = u64_at(&d5, 56);
    assert!(cross_price >= usd(18050) && cross_price <= usd(18100), "cross price is between the orders");
    // the remainder of the GTC buy (10 shares) rests in the day book with its own escrow
    let (owner, price, qty, escrow, owed, side, status) = env.day_slot(0);
    assert_eq!(owner, night_buy.kp.pubkey());
    assert_eq!((price, qty, side, status, owed), (usd(18100), 10 * U, BUY, 1, 0));
    assert_eq!(escrow, 30 * 18_100 * U / 100 - u64_at(&d5, 128 + 56), "remainder escrow = total minus what stays behind the fill");
    pass(&format!("opening cross: clears before the window ends at {}, GTC remainder rests in the day book ({cu} CU)", cross_price as f64 / U as f64));

    env.claim(&night_buy, b5, 0).expect("claim cross buy");
    env.claim(&night_sell, b5, 1).expect("claim cross sell");
    assert_eq!(amount(&env.svm, &night_buy.base), 20 * U);
    assert_eq!(amount(&env.svm, &night_sell.quote), 20 * cross_price);
    env.day_exit(&night_buy, 0, true).expect("cancel rolled remainder");
    assert_eq!(amount(&env.svm, &night_buy.quote), 100_000 * U - 20 * cross_price, "20 shares at the cross price, the remainder returned");
    assert_eq!(env.vaults(), (0, 0));
    pass("opening cross: fills claim at one price, the rested remainder cancels in full");

    // ---------------------------------------------------------------- Day

    // 5% band around 180.80: 171.76 .. 189.84
    let maker1 = env.trader(100 * U, 100_000 * U);
    let maker2 = env.trader(100 * U, 100_000 * U);
    let taker = env.trader(100 * U, 100_000 * U);
    expect_err(env.place_day(&maker1, SELL, usd(19000), U), 14, "price outside the band");
    expect_err(env.place_day(&maker1, BUY, usd(17000), U), 14, "price outside the band");
    pass("band: prices more than 5% from the reference are rejected");

    // book: sells of 10 @ 180.80 (maker1) and 5 @ 180.90 (maker2)
    env.place_day(&maker1, SELL, usd(18080), 10 * U).expect("m1 ask");
    env.place_day(&maker2, SELL, usd(18090), 5 * U).expect("m2 ask");
    assert_eq!(amount(&env.svm, &maker1.base), 90 * U, "the sell went into escrow");
    // a buy of 12 @ 181.00 hits the best price first and pays the maker's price
    let tq = amount(&env.svm, &taker.quote);
    let cu = env.place_day(&taker, BUY, usd(18100), 12 * U).expect("taker buy");
    assert_eq!(amount(&env.svm, &taker.base), 112 * U, "taker received 12 immediately");
    assert_eq!(tq - amount(&env.svm, &taker.quote), 10 * 18_080 * U / 100 + 2 * 18_090 * U / 100, "at maker prices, not at its own limit");
    assert_eq!(env.market_u64(M_LAST), usd(18090));
    // no order is left in the book: the taker filled completely
    let (_, _, q1, e1, o1, _, st1) = env.day_slot(0);
    assert_eq!((q1, e1, o1, st1), (0, 0, 10 * 18_080 * U / 100, 1), "maker1 fully filled");
    let (_, _, q2, e2, o2, _, _) = env.day_slot(1);
    assert_eq!((q2, e2, o2), (3 * U, 3 * U, 2 * 18_090 * U / 100), "maker2 filled for 2");
    assert_eq!(env.day_slot(2).6, 0, "taker did not rest in the book");
    pass(&format!("day book: price then time, taker pays maker prices, fills settle instantly ({cu} CU)"));

    println!("max CU for place: {max_cu}");
}

fn main() {
    clearing_tests();
    let so = std::env::var("VELQUE_SO").unwrap_or_else(|_| "../target/deploy/velque.so".to_string());
    svm_tests(&so);
    println!("ALL PASS");
}
