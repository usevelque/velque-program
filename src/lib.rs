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

