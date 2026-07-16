//! Velque: an order book for tokenized stocks on Solana, built around the
//! hours when Nasdaq is closed.

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
    Ok(())
}

