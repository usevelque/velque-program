//! Velque: an order book for tokenized stocks on Solana, built around the
//! hours when Nasdaq is closed.
//!
//! The session is defined by how fresh the reference price is. While the
//! oracle keeps updating it (Nasdaq is open), it is Day: a continuous book,
//! price-time priority, a band around the reference. When the updates stop,
//! Dark begins: orders accumulate in windows and clear at a single price. The
//! first clearing after the reference turns fresh again is the opening cross.
//!
//! Instructions (first data byte):
//!   0  init_market(window, tick, lot, reference, max_age, band_bps)
//!   1  place(side, price, qty, tif)   auction order (Dark only)
//!   2  cancel(index)                  cancel an order in the current window
//!   3  clear                          clear the window; in Day this is the opening cross
//!   4  claim(index)                   claim the fill and the change for a window
//!   5  set_reference(price)           reference price (authority = oracle)
//!   6  close_book                     close a settled window book
//!
//! Both token programs are supported, SPL Token and Token-2022 (the real
//! xStocks are issued on Token-2022). The vaults are the market's ATAs, and
//! transfers go through transfer_checked of the program that owns the mint.

#![no_std]

pub mod clearing;
pub mod state;

use pinocchio::{
    cpi::{Seed, Signer},
    error::ProgramError,
    sysvars::{clock::Clock, Sysvar},
    AccountView, Address, ProgramResult,
};
use pinocchio_associated_token_account::instructions::CreateIdempotent;
use pinocchio_system::instructions::CreateAccount;
use pinocchio_token::instructions::TransferChecked;

use clearing::{Orders, BUY, CAP, SELL};
use state::*;

pub const TOKEN: Address = Address::from_str_const("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
pub const TOKEN_2022: Address = Address::from_str_const("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");

pub const SEED_MARKET: &[u8] = b"market";
pub const SEED_BOOK: &[u8] = b"book";
pub const SEED_DAY: &[u8] = b"day";

#[cfg(not(feature = "no-entrypoint"))]
mod entry {
    use super::*;
    pinocchio::program_entrypoint!(process_instruction);
    pinocchio::no_allocator!();
    pinocchio::nostd_panic_handler!();
}

#[repr(u32)]
pub enum VelqueError {
    BadPda = 1,
    BadAccount = 2,
    BadParams = 3,
    WindowClosed = 4,
    WindowOpen = 5,
    BookFull = 6,
    NotOwner = 7,
    BadStatus = 8,
    NotCleared = 9,
    Math = 10,
    Unauthorized = 11,
    SessionDay = 12,
    SessionDark = 13,
    OutOfBand = 14,
    NothingToMove = 15,
    BadProgram = 16,
}

impl From<VelqueError> for ProgramError {
    fn from(e: VelqueError) -> Self {
        ProgramError::Custom(e as u32)
    }
}

pub fn process_instruction(
    program_id: &Address,
    accounts: &mut [AccountView],
    data: &[u8],
) -> ProgramResult {
    let accounts: &[AccountView] = accounts;
    match data.split_first() {
        Some((0, rest)) => init_market(program_id, accounts, rest),
        Some((1, rest)) => place(program_id, accounts, rest),
        Some((2, rest)) => cancel(program_id, accounts, rest),
        Some((3, _)) => clear(program_id, accounts),
        Some((4, rest)) => claim(program_id, accounts, rest),
        Some((5, rest)) => set_reference(program_id, accounts, rest),
        Some((6, _)) => close_book(program_id, accounts),
        _ => Err(ProgramError::InvalidInstructionData),
    }
}

// ================================================================ helpers

fn read_u64(d: &[u8], i: usize) -> Result<u64, ProgramError> {
    d.get(i..i + 8)
        .map(|s| u64::from_le_bytes(s.try_into().unwrap()))
        .ok_or(ProgramError::InvalidInstructionData)
}

fn read_u16(d: &[u8], i: usize) -> Result<u16, ProgramError> {
    d.get(i..i + 2)
        .map(|s| u16::from_le_bytes(s.try_into().unwrap()))
        .ok_or(ProgramError::InvalidInstructionData)
}

fn now() -> Result<i64, ProgramError> {
    Ok(Clock::get()?.unix_timestamp)
}

fn is_token_program(a: &Address) -> bool {
    a == &TOKEN || a == &TOKEN_2022
}

/// Mint of a token account. The first 72 bytes are the same in SPL Token and
/// Token-2022: mint, owner, amount; in 2022 extensions may follow further on.
fn check_token_account(acc: &AccountView, mint: &Address) -> ProgramResult {
    let owner_ok = acc.owned_by(&TOKEN) || acc.owned_by(&TOKEN_2022);
    if !owner_ok || acc.data_len() < 165 {
        return Err(VelqueError::BadAccount.into());
    }
    let d = acc.try_borrow()?;
    if get_addr(&d, 0) != *mint {
        return Err(VelqueError::BadAccount.into());
    }
    Ok(())
}

/// Market snapshot needed by the instructions.
struct MarketView {
    bump: u8,
    base_dec: u8,
    quote_dec: u8,
    authority: Address,
    base_mint: Address,
    quote_mint: Address,
    base_prog: Address,
    quote_prog: Address,
    vbase: Address,
    vquote: Address,
    window_secs: u64,
    tick: u64,
    lot: u64,
    auction_id: u64,
    window_end: i64,
    reference: u64,
    ref_at: i64,
    max_age: u64,
    band_bps: u64,
}

impl MarketView {
    /// Day: the reference was updated no more than max_age ago.
    fn is_day(&self, t: i64) -> bool {
        self.ref_at > 0 && t >= self.ref_at && (t - self.ref_at) as u64 <= self.max_age
    }
}

fn load_market(program_id: &Address, market: &AccountView) -> Result<MarketView, ProgramError> {
    if !market.owned_by(program_id) {
        return Err(VelqueError::BadAccount.into());
    }
    let d = market.try_borrow()?;
    if d.len() != MARKET_LEN || d[0] != MARKET_TAG {
        return Err(ProgramError::InvalidAccountData);
    }
    Ok(MarketView {
        bump: d[m::BUMP],
        base_dec: d[m::BASE_DEC],
        quote_dec: d[m::QUOTE_DEC],
        authority: get_addr(&d, m::AUTHORITY),
        base_mint: get_addr(&d, m::BASE_MINT),
        quote_mint: get_addr(&d, m::QUOTE_MINT),
        base_prog: get_addr(&d, m::BASE_PROG),
        quote_prog: get_addr(&d, m::QUOTE_PROG),
        vbase: get_addr(&d, m::VBASE),
        vquote: get_addr(&d, m::VQUOTE),
        window_secs: get_u64(&d, m::WINDOW_SECS),
        tick: get_u64(&d, m::TICK),
        lot: get_u64(&d, m::LOT),
        auction_id: get_u64(&d, m::AUCTION_ID),
        window_end: get_i64(&d, m::WINDOW_END),
        reference: get_u64(&d, m::REFERENCE),
        ref_at: get_i64(&d, m::REF_AT),
        max_age: get_u64(&d, m::MAX_AGE),
        band_bps: get_u64(&d, m::BAND_BPS),
    })
}

/// One side of the market: vault, mint, token program and decimals.
struct Leg<'a> {
    vault: &'a AccountView,
    mint: &'a AccountView,
    prog: &'a Address,
    decimals: u8,
}

/// Check that the passed accounts are the market's base or quote side.
fn leg<'a>(
    mv: &MarketView,
    base: bool,
    vault: &'a AccountView,
    mint: &'a AccountView,
    prog: &'a AccountView,
) -> Result<Leg<'a>, ProgramError> {
    let (v, mt, p, dec) = if base {
        (&mv.vbase, &mv.base_mint, &mv.base_prog, mv.base_dec)
    } else {
        (&mv.vquote, &mv.quote_mint, &mv.quote_prog, mv.quote_dec)
    };
    if vault.address() != v || mint.address() != mt {
        return Err(VelqueError::BadPda.into());
    }
    if prog.address() != p {
        return Err(VelqueError::BadProgram.into());
    }
    Ok(Leg { vault, mint, prog: prog.address(), decimals: dec })
}

/// Transfer from the user into the vault: signed by the user.
fn pay_in(leg: &Leg, from: &AccountView, owner: &AccountView, amount: u64) -> ProgramResult {
    if amount == 0 {
        return Ok(());
    }
    TransferChecked::<&AccountView>::new(from, leg.mint, leg.vault, owner, amount, leg.decimals)
        .invoke_with_program(leg.prog)
}

/// Transfer out of the vault: signed by the market PDA.
fn pay_out(leg: &Leg, to: &AccountView, market: &AccountView, mv: &MarketView, amount: u64) -> ProgramResult {
    if amount == 0 {
        return Ok(());
    }
    let bump = [mv.bump];
    let seeds = [Seed::from(SEED_MARKET), Seed::from(mv.base_mint.as_ref()), Seed::from(&bump)];
    TransferChecked::<&AccountView>::new(leg.vault, leg.mint, to, market, amount, leg.decimals)
        .invoke_signed_with_program(&[Signer::from(&seeds)], leg.prog)
}

fn book_key(market: &Address, auction_id: u64, program_id: &Address) -> (Address, u8) {
    Address::find_program_address(&[SEED_BOOK, market.as_ref(), &auction_id.to_le_bytes()], program_id)
}

fn day_key(market: &Address, program_id: &Address) -> (Address, u8) {
    Address::find_program_address(&[SEED_DAY, market.as_ref()], program_id)
}

/// Create the book for window `id` if it does not exist yet. `payer` pays the rent.
fn ensure_book(
    program_id: &Address,
    payer: &AccountView,
    book: &AccountView,
    market: &Address,
    id: u64,
    window_end: i64,
) -> ProgramResult {
    let (bk, bump) = book_key(market, id, program_id);
    if book.address() != &bk {
        return Err(VelqueError::BadPda.into());
    }
    if book.data_len() > 0 {
        if !book.owned_by(program_id) {
            return Err(VelqueError::BadAccount.into());
        }
        return Ok(());
    }
    let idb = id.to_le_bytes();
    let bb = [bump];
    let seeds = [Seed::from(SEED_BOOK), Seed::from(market.as_ref()), Seed::from(&idb), Seed::from(&bb)];
    CreateAccount::with_minimum_balance(payer, book, BOOK_LEN as u64, program_id, None)?
        .invoke_signed(&[Signer::from(&seeds)])?;
    let mut bv = *book;
    let mut d = bv.try_borrow_mut()?;
    d[0] = BOOK_TAG;
    d[b::BUMP] = bump;
    put_addr(&mut d, b::MARKET, market);
    put_u64(&mut d, b::AUCTION_ID, id);
    put_i64(&mut d, b::WINDOW_END, window_end);
    put_addr(&mut d, b::PAYER, payer.address());
    Ok(())
}

/// Create the market's day book if it does not exist yet. `payer` pays the rent.
fn ensure_day(program_id: &Address, payer: &AccountView, day: &AccountView, market: &Address) -> ProgramResult {
    let (dk, bump) = day_key(market, program_id);
    if day.address() != &dk {
        return Err(VelqueError::BadPda.into());
    }
    if day.data_len() > 0 {
        if !day.owned_by(program_id) {
            return Err(VelqueError::BadAccount.into());
        }
        return Ok(());
    }
    let bb = [bump];
    let seeds = [Seed::from(SEED_DAY), Seed::from(market.as_ref()), Seed::from(&bb)];
    CreateAccount::with_minimum_balance(payer, day, DAY_LEN as u64, program_id, None)?
        .invoke_signed(&[Signer::from(&seeds)])?;
    let mut dv = *day;
    let mut d = dv.try_borrow_mut()?;
    d[0] = DAY_TAG;
    d[dh::BUMP] = bump;
    put_addr(&mut d, dh::MARKET, market);
    put_addr(&mut d, dh::PAYER, payer.address());
    Ok(())
}

/// Next sequence number in the day book queue.
fn next_seq(market: &AccountView) -> Result<u64, ProgramError> {
    let mut mk = *market;
    let mut d = mk.try_borrow_mut()?;
    let s = get_u64(&d, m::DAY_SEQ) + 1;
    put_u64(&mut d, m::DAY_SEQ, s);
    Ok(s)
}

/// Put an entry into a free day book slot. None if there is no room.
fn day_insert(d: &mut [u8], owner: &[u8], price: u64, qty: u64, escrow: u64, side: u8, seq: u64) -> Option<usize> {
    for i in 0..DAY_CAP {
        let o = day_off(i);
        if d[o + de::STATUS] == D_EMPTY {
            d[o..o + 32].copy_from_slice(owner);
            put_u64(d, o + de::PRICE, price);
            put_u64(d, o + de::QTY, qty);
            put_u64(d, o + de::ESCROW, escrow);
            put_u64(d, o + de::OWED, 0);
            put_u64(d, o + de::SEQ, seq);
            d[o + de::SIDE] = side;
            d[o + de::STATUS] = D_LIVE;
            return Some(i);
        }
    }
    None
}

// ================================================================ init_market

/// Accounts:
///  0 authority (oracle)            [signer, writable]
///  1 market PDA ["market", base]   [writable]
///  2 base_mint
///  3 quote_mint
///  4 vbase = ATA(market, base)     [writable]
///  5 vquote = ATA(market, quote)   [writable]
///  6 base token program
///  7 quote token program
///  8 Associated Token program
///  9 System program
///
/// Data: window_secs, tick, lot, reference, max_age, band_bps (all u64).
fn init_market(program_id: &Address, accounts: &[AccountView], data: &[u8]) -> ProgramResult {
    let [authority, market, base_mint, quote_mint, vbase, vquote, base_prog, quote_prog, _ata, system, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if !authority.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let window_secs = read_u64(data, 0)?;
    let tick = read_u64(data, 8)?;
    let lot = read_u64(data, 16)?;
    let reference = read_u64(data, 24)?;
    let max_age = read_u64(data, 32)?;
    let band_bps = read_u64(data, 40)?;
    if window_secs == 0 || tick == 0 || lot == 0 || reference % tick != 0 || max_age == 0 || band_bps == 0 || band_bps > 10_000 {
        return Err(VelqueError::BadParams.into());
    }
    if !is_token_program(base_prog.address()) || !is_token_program(quote_prog.address()) {
        return Err(VelqueError::BadProgram.into());
    }
    if !base_mint.owned_by(base_prog.address()) || !quote_mint.owned_by(quote_prog.address()) {
        return Err(VelqueError::BadAccount.into());
    }
    let (base_dec, quote_dec) = {
        let bd = base_mint.try_borrow()?;
        let qd = quote_mint.try_borrow()?;
        if bd.len() < 82 || qd.len() < 82 {
            return Err(VelqueError::BadAccount.into());
        }
        (bd[44], qd[44])
    };

    let bm = base_mint.address();
    let (market_key, market_bump) = Address::find_program_address(&[SEED_MARKET, bm.as_ref()], program_id);
    if market.address() != &market_key {
        return Err(VelqueError::BadPda.into());
    }
    let mb = [market_bump];
    let market_seeds = [Seed::from(SEED_MARKET), Seed::from(bm.as_ref()), Seed::from(&mb)];
    CreateAccount::with_minimum_balance(authority, market, MARKET_LEN as u64, program_id, None)?
        .invoke_signed(&[Signer::from(&market_seeds)])?;

    // vaults: the market's ATAs; the ATA program itself checks the address and the size for 2022 extensions
    CreateIdempotent { funding_account: authority, account: vbase, wallet: market, mint: base_mint, system_program: system, token_program: base_prog }
        .invoke()?;
    CreateIdempotent { funding_account: authority, account: vquote, wallet: market, mint: quote_mint, system_program: system, token_program: quote_prog }
        .invoke()?;

    let t = now()?;
    let mut mk = *market;
    let mut d = mk.try_borrow_mut()?;
    d[0] = MARKET_TAG;
    d[m::BUMP] = market_bump;
    d[m::BASE_DEC] = base_dec;
    d[m::QUOTE_DEC] = quote_dec;
    put_addr(&mut d, m::AUTHORITY, authority.address());
    put_addr(&mut d, m::BASE_MINT, bm);
    put_addr(&mut d, m::QUOTE_MINT, quote_mint.address());
    put_addr(&mut d, m::BASE_PROG, base_prog.address());
    put_addr(&mut d, m::QUOTE_PROG, quote_prog.address());
    put_addr(&mut d, m::VBASE, vbase.address());
    put_addr(&mut d, m::VQUOTE, vquote.address());
    put_u64(&mut d, m::WINDOW_SECS, window_secs);
    put_u64(&mut d, m::TICK, tick);
    put_u64(&mut d, m::LOT, lot);
    put_u64(&mut d, m::AUCTION_ID, 0);
    put_i64(&mut d, m::WINDOW_START, t);
    put_i64(&mut d, m::WINDOW_END, t + window_secs as i64);
    put_u64(&mut d, m::REFERENCE, reference);
    put_i64(&mut d, m::REF_AT, 0); // start in Dark: no reference has arrived yet
    put_u64(&mut d, m::MAX_AGE, max_age);
    put_u64(&mut d, m::BAND_BPS, band_bps);
    Ok(())
}

// ================================================================ place (auction)

/// Accounts:
///  0 owner                          [signer, writable]
///  1 market
///  2 book PDA of the current window [writable]  created by the first order
///  3 owner's account: base on sell, quote on buy [writable]
///  4 vault of this side             [writable]
///  5 mint of this side
///  6 token program of this side
///  7 System program
fn place(program_id: &Address, accounts: &[AccountView], data: &[u8]) -> ProgramResult {
    let [owner, market, book, src, vault, mint, prog, _system, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if !owner.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let side = *data.first().ok_or(ProgramError::InvalidInstructionData)?;
    let price = read_u64(data, 1)?;
    let qty = read_u64(data, 9)?;
    let tif = data.get(17).copied().unwrap_or(TIF_ONE);
    let mv = load_market(program_id, market)?;
    if (side != BUY && side != SELL) || tif > TIF_GTC || price == 0 || price % mv.tick != 0 || qty == 0 || qty % mv.lot != 0 {
        return Err(VelqueError::BadParams.into());
    }
    let t = now()?;
    if mv.is_day(t) {
        return Err(VelqueError::SessionDay.into());
    }
    if t >= mv.window_end {
        return Err(VelqueError::WindowClosed.into());
    }
    let lg = leg(&mv, side == SELL, vault, mint, prog)?;
    check_token_account(src, mint.address())?;

    let mk = market.address();
    ensure_book(program_id, owner, book, mk, mv.auction_id, mv.window_end)?;
    let amount = if side == SELL { qty } else { quote_for(price, qty, mv.base_dec, true).ok_or(VelqueError::Math)? };
    pay_in(&lg, src, owner, amount)?;

    let mut bv = *book;
    let mut d = bv.try_borrow_mut()?;
    if d[0] != BOOK_TAG || d[b::STATE] != 0 {
        return Err(VelqueError::BadAccount.into());
    }
    let n = count(&d);
    if n >= CAP {
        return Err(VelqueError::BookFull.into());
    }
    let o = entry_off(n);
    put_addr(&mut d, o + e::OWNER, owner.address());
    put_u64(&mut d, o + e::PRICE, price);
    put_u64(&mut d, o + e::QTY, qty);
    put_u64(&mut d, o + e::FILLED, 0);
    put_u64(&mut d, o + e::ESCROW, amount);
    d[o + e::SIDE] = side;
    d[o + e::STATUS] = LIVE;
    d[o + e::TIF] = tif;
    set_count(&mut d, n + 1);
    Ok(())
}

// ================================================================ cancel (auction)

/// Accounts:
///  0 owner          [signer]
///  1 market
///  2 book           [writable]
///  3 refund account [writable]
///  4 vault          [writable]
///  5 mint
///  6 token program
fn cancel(program_id: &Address, accounts: &[AccountView], data: &[u8]) -> ProgramResult {
    let [owner, market, book, dest, vault, mint, prog, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if !owner.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let idx = read_u16(data, 0)? as usize;
    let mv = load_market(program_id, market)?;
    if now()? >= mv.window_end {
        return Err(VelqueError::WindowClosed.into());
    }
    let mk = market.address();
    let (bk, _) = book_key(mk, mv.auction_id, program_id);
    if book.address() != &bk || !book.owned_by(program_id) {
        return Err(VelqueError::BadPda.into());
    }
    let (side, amount) = {
        let mut bv = *book;
        let mut d = bv.try_borrow_mut()?;
        if d[0] != BOOK_TAG || d[b::STATE] != 0 || idx >= count(&d) {
            return Err(VelqueError::BadStatus.into());
        }
        let o = entry_off(idx);
        if get_addr(&d, o + e::OWNER) != *owner.address() {
            return Err(VelqueError::NotOwner.into());
        }
        if d[o + e::STATUS] != LIVE {
            return Err(VelqueError::BadStatus.into());
        }
        d[o + e::STATUS] = CANCELLED;
        let amount = get_u64(&d, o + e::ESCROW);
        put_u64(&mut d, o + e::ESCROW, 0);
        (d[o + e::SIDE], amount)
    };
    let lg = leg(&mv, side == SELL, vault, mint, prog)?;
    check_token_account(dest, mint.address())?;
    pay_out(&lg, dest, market, &mv, amount)
}

// ================================================================ clear

/// Cost of a buy fill, capped at the entry's escrow.
fn buy_cost(price: u64, filled: u64, escrow: u64, decimals: u8) -> Result<u64, ProgramError> {
    let c = quote_for(price, filled, decimals, true).ok_or(VelqueError::Math)?;
    Ok(c.min(escrow))
}

/// Price, fills, escrow. Returns the clearing price and the number of orders
/// to carry over. The clearing arrays live in their own stack frame.
#[inline(never)]
fn settle_book(d: &mut [u8], mv: &MarketView, t: i64) -> Result<(u64, usize), ProgramError> {
    let n = count(d);
    let mut o = Orders::empty();
    o.n = n;
    for i in 0..n {
        let off = entry_off(i);
        o.price[i] = get_u64(d, off + e::PRICE);
        o.qty[i] = get_u64(d, off + e::QTY);
        o.side[i] = d[off + e::SIDE];
        o.live[i] = d[off + e::STATUS] == LIVE;
    }
    let out = clearing::find_price(&o, mv.reference, mv.tick);
    let mut fills = [0u64; CAP];
    clearing::allocate(&o, out.price, out.volume, mv.lot, &mut fills);

    let mut rolls = 0usize;
    for i in 0..n {
        let off = entry_off(i);
        put_u64(d, off + e::FILLED, fills[i]);
        d[off + e::ROLL] = 0;
        if !o.live[i] || d[off + e::TIF] != TIF_GTC || fills[i] >= o.qty[i] {
            continue;
        }
        let escrow = get_u64(d, off + e::ESCROW);
        let keep = if o.side[i] == BUY { buy_cost(out.price, fills[i], escrow, mv.base_dec)? } else { fills[i] };
        put_u64(d, off + e::ESCROW, keep);
        put_u64(d, off + e::ROLL_ESCROW, escrow - keep);
        d[off + e::ROLL] = 1;
        rolls += 1;
    }
    d[b::STATE] = 1;
    put_u64(d, b::CLEAR_PRICE, out.price);
    put_u64(d, b::VOLUME, out.volume);
    put_u64(d, b::IMBALANCE, out.imbalance);
    put_u64(d, b::REFERENCE, mv.reference);
    put_i64(d, b::CLEARED_AT, t);
    Ok((out.price, rolls))
}

/// Carry the flagged orders over into the next window's book (Dark).
fn roll_into_book(src: &mut [u8], dst: &mut [u8]) {
    let mut k = count(dst);
    for i in 0..count(src) {
        let so = entry_off(i);
        if src[so + e::ROLL] != 1 {
            continue;
        }
        let dof = entry_off(k);
        dst[dof..dof + 32].copy_from_slice(&src[so..so + 32]);
        put_u64(dst, dof + e::PRICE, get_u64(src, so + e::PRICE));
        put_u64(dst, dof + e::QTY, get_u64(src, so + e::QTY) - get_u64(src, so + e::FILLED));
        put_u64(dst, dof + e::FILLED, 0);
        put_u64(dst, dof + e::ESCROW, get_u64(src, so + e::ROLL_ESCROW));
        dst[dof + e::SIDE] = src[so + e::SIDE];
        dst[dof + e::STATUS] = LIVE;
        dst[dof + e::TIF] = TIF_GTC;
        k += 1;
    }
    set_count(dst, k);
}

/// Accounts:
///  0 cranker              [signer, writable]  anyone; pays the rent if needed
///  1 market               [writable]
///  2 current window book  [writable]
///  3 next window book     [writable]
///  4 day book             [writable]
///  5 System program
///
/// Dark: only after the window ends, GTC remainders roll into the next window.
/// Day: this is the opening cross. What has accumulated clears right away,
/// without waiting for the window to end, GTC remainders rest in the day book.
fn clear(program_id: &Address, accounts: &[AccountView]) -> ProgramResult {
    let [cranker, market, book, next_book, day, _system, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if !cranker.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let mv = load_market(program_id, market)?;
    let t = now()?;
    let mk = market.address();
    let (bk, _) = book_key(mk, mv.auction_id, program_id);
    if book.address() != &bk {
        return Err(VelqueError::BadPda.into());
    }
    let exists = book.data_len() > 0;
    if exists && !book.owned_by(program_id) {
        return Err(VelqueError::BadAccount.into());
    }
    let has_orders = exists && {
        let d = book.try_borrow()?;
        (0..count(&d)).any(|i| d[entry_off(i) + e::STATUS] == LIVE)
    };
    let cross = mv.is_day(t) && has_orders;
    if !cross && t < mv.window_end {
        return Err(VelqueError::WindowOpen.into());
    }
    let next_id = mv.auction_id + 1;
    let next_end = t + mv.window_secs as i64;

    let mut last_price = 0u64;
    if exists {
        let rolls = {
            let mut bv = *book;
            let mut d = bv.try_borrow_mut()?;
            if d[0] != BOOK_TAG || d[b::STATE] != 0 {
                return Err(VelqueError::BadStatus.into());
            }
            let (price, rolls) = settle_book(&mut d, &mv, t)?;
            last_price = price;
            rolls
        };
        if rolls > 0 {
                ensure_book(program_id, cranker, next_book, mk, next_id, next_end)?;
                let mut sb = *book;
                let mut src = sb.try_borrow_mut()?;
                let mut nb = *next_book;
                let mut dst = nb.try_borrow_mut()?;
                roll_into_book(&mut src, &mut dst);
        }
    }

    let mut mkv = *market;
    let mut d = mkv.try_borrow_mut()?;
    let cleared = get_u64(&d, m::CLEARED);
    put_u64(&mut d, m::AUCTION_ID, next_id);
    put_i64(&mut d, m::WINDOW_START, t);
    put_i64(&mut d, m::WINDOW_END, next_end);
    if last_price > 0 {
        put_u64(&mut d, m::LAST_PRICE, last_price);
    }
    put_u64(&mut d, m::CLEARED, cleared + 1);
    Ok(())
}

// ================================================================ claim (auction)

/// Accounts:
///  0 owner                 [signer]
///  1 market
///  2 cleared window book   [writable]
///  3 owner's base account  [writable]
///  4 owner's quote account [writable]
///  5 vbase                 [writable]
///  6 vquote                [writable]
///  7 base_mint
///  8 quote_mint
///  9 base program
/// 10 quote program
fn claim(program_id: &Address, accounts: &[AccountView], data: &[u8]) -> ProgramResult {
    let [owner, market, book, base_dest, quote_dest, vbase, vquote, base_mint, quote_mint, base_prog, quote_prog, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if !owner.is_signer() {
        return Err(ProgramError::MissingRequiredSignature);
    }
    let idx = read_u16(data, 0)? as usize;
    let mv = load_market(program_id, market)?;
    let bl = leg(&mv, true, vbase, base_mint, base_prog)?;
    let ql = leg(&mv, false, vquote, quote_mint, quote_prog)?;
    check_token_account(base_dest, &mv.base_mint)?;
    check_token_account(quote_dest, &mv.quote_mint)?;
    if !book.owned_by(program_id) {
        return Err(VelqueError::BadAccount.into());
    }
    let mk = market.address();
    let (base_out, quote_out) = {
        let mut bv = *book;
        let mut d = bv.try_borrow_mut()?;
        if d.len() != BOOK_LEN || d[0] != BOOK_TAG || get_addr(&d, b::MARKET) != *mk {
            return Err(VelqueError::BadAccount.into());
        }
        let id = get_u64(&d, b::AUCTION_ID);
        let expect = Address::derive_address(&[SEED_BOOK, mk.as_ref(), &id.to_le_bytes()], Some(d[b::BUMP]), program_id);
        if book.address() != &expect {
            return Err(VelqueError::BadPda.into());
        }
        if d[b::STATE] != 1 {
            return Err(VelqueError::NotCleared.into());
        }
        if idx >= count(&d) {
            return Err(VelqueError::BadStatus.into());
        }
        let o = entry_off(idx);
        if get_addr(&d, o + e::OWNER) != *owner.address() {
            return Err(VelqueError::NotOwner.into());
        }
        if d[o + e::STATUS] != LIVE {
            return Err(VelqueError::BadStatus.into());
        }
        d[o + e::STATUS] = CLAIMED;
        let clear_price = get_u64(&d, b::CLEAR_PRICE);
        let filled = get_u64(&d, o + e::FILLED);
        let escrow = get_u64(&d, o + e::ESCROW);
        put_u64(&mut d, o + e::ESCROW, 0);
        if d[o + e::SIDE] == BUY {
            let cost = buy_cost(clear_price, filled, escrow, mv.base_dec)?;
            (filled, escrow - cost)
        } else {
            let proceeds = quote_for(clear_price, filled, mv.base_dec, false).ok_or(VelqueError::Math)?;
            (escrow.checked_sub(filled).ok_or(VelqueError::Math)?, proceeds)
        }
    };
    pay_out(&bl, base_dest, market, &mv, base_out)?;
    pay_out(&ql, quote_dest, market, &mv, quote_out)
}

// ================================================================ set_reference

/// Accounts: 0 authority [signer], 1 market [writable]. Data: price u64.
/// An update makes the reference fresh: the market switches to Day.
fn set_reference(program_id: &Address, accounts: &[AccountView], data: &[u8]) -> ProgramResult {
    let [authority, market, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    let price = read_u64(data, 0)?;
    let mv = load_market(program_id, market)?;
    if !authority.is_signer() || authority.address() != &mv.authority {
        return Err(VelqueError::Unauthorized.into());
    }
    if price == 0 || price % mv.tick != 0 {
        return Err(VelqueError::BadParams.into());
    }
    let t = now()?;
    let mut mkv = *market;
    let mut d = mkv.try_borrow_mut()?;
    put_u64(&mut d, m::REFERENCE, price);
    put_i64(&mut d, m::REF_AT, t);
    Ok(())
}

// ================================================================ close_book

/// Close a cleared window book that owes nothing to anyone.
/// The rent goes to whoever paid for the book. Anyone can call this.
///
/// Accounts: 0 book [writable], 1 payer from the book header [writable]
fn close_book(program_id: &Address, accounts: &[AccountView]) -> ProgramResult {
    let [book, payer, ..] = accounts else {
        return Err(ProgramError::NotEnoughAccountKeys);
    };
    if !book.owned_by(program_id) {
        return Err(VelqueError::BadAccount.into());
    }
    {
        let d = book.try_borrow()?;
        if d.len() != BOOK_LEN || d[0] != BOOK_TAG || d[b::STATE] != 1 {
            return Err(VelqueError::NotCleared.into());
        }
        if get_addr(&d, b::PAYER) != *payer.address() {
            return Err(VelqueError::BadAccount.into());
        }
        for i in 0..count(&d) {
            let o = entry_off(i);
            let settled = match d[o + e::STATUS] {
                CANCELLED | CLAIMED => true,
                LIVE => get_u64(&d, o + e::FILLED) == 0 && get_u64(&d, o + e::ESCROW) == 0,
                _ => true,
            };
            if !settled {
                return Err(VelqueError::BadStatus.into());
            }
        }
    }
    let mut bv = *book;
    let mut pv = *payer;
    pv.set_lamports(pv.lamports().checked_add(bv.lamports()).ok_or(VelqueError::Math)?);
    bv.set_lamports(0);
    bv.close()
}

