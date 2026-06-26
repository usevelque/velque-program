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

