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

use clearing::{Orders, BUY, CAP, SELL};
use state::*;

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

