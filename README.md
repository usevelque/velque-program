# velque-program

The on-chain program behind Velque, an order book for tokenized stocks on Solana built around the hours when Nasdaq is closed.

Nasdaq prices a stock for 32.5 hours a week. Tokenized stocks trade all 168. Velque changes how trading works when the exchange price disappears:

| Session | When | How orders match |
| --- | --- | --- |
| **Day** | The reference price is fresh (Nasdaq regular session) | Continuous book, price then time, inside a band around the reference |
| **Dark** | The reference has gone stale (nights, weekends, holidays, halts) | Orders collect in fixed windows and clear together at one price |
| **Opening cross** | The first clear after the reference comes back | The waiting window clears at once and hands its remainder to the Day book |

The session is not a clock inside the program. It follows the freshness of the reference price: if the oracle stops posting, the market is Dark.

## Status

Runs on Solana devnet. Not audited.

| | |
| --- | --- |
| Program ID (devnet) | `MXG3VzXQucitJ4MSWWd1ddEat5FRS8KFF5j1uZRW7jz` |
| Framework | [Pinocchio](https://github.com/anza-xyz/pinocchio) 0.11, `no_std`, no allocator |
| Token programs | SPL Token and Token-2022 (xStocks are Token-2022) |
| Binary size | 104,096 bytes |

## Instructions

The first byte of instruction data selects the instruction.

| # | Instruction | What it does |
| --- | --- | --- |
| 0 | `init_market` | Create a market for a base mint: window length, tick, lot, reference, max age, band, minimum order value. Vaults are the market's associated token accounts. |
| 1 | `place` | Place an auction order in the current window (Dark only). Funds go to escrow. |
| 2 | `cancel` | Cancel an auction order before its window ends. Full refund. |
| 3 | `clear` | Clear the window. Anyone can call it. In Day, with orders waiting, this is the opening cross. |
| 4 | `claim` | Collect the fill and the change for an order in a cleared window. |
| 5 | `set_reference` | Post the reference price. Only the market's oracle key. |
| 6 | `close_book` | Close a fully settled window book and return its rent to whoever paid it. |
| 7 | `place_day` | Place an order in the Day book. It matches at once at resting prices; the rest waits. |
| 8 | `cancel_day` | Cancel a Day order. Refunds the escrow and pays what the order earned. |
| 9 | `claim_day` | Collect what a resting Day order earned. |
| 10 | `close_day` | In Dark, move resting Day orders into the current auction window as until-cancelled orders. |
| 11 | `set_authority` | Hand the oracle role to another key. |

## How an auction clears

One price for everyone in the window, chosen in this order:

1. the price that trades the most volume;
2. if tied, the price that leaves the smallest imbalance;
3. if tied, the price closest to the reference;
4. if still tied, the midpoint of the remaining range, floored to the tick.

Orders priced better than the clearing price fill first. Orders exactly at the clearing price share what is left pro rata by size, and lot remainders go out in order of arrival. The rule lives in [`src/clearing.rs`](src/clearing.rs) and has no dependencies, so it runs the same on chain and on a laptop. Clearing a full book of 64 orders costs about 49k compute units.

## Accounts

| Account | Seeds | Holds |
| --- | --- | --- |
| Market | `["market", base_mint]` | Mints, token programs, vaults, parameters, reference price and its timestamp |
| Window book | `["book", market, auction_id]` | Up to 64 orders of one auction window and its result |
| Day book | `["day", market]` | 64 slots of resting Day orders |

Layouts are fixed offsets with no serializer. They are documented at the top of [`src/state.rs`](src/state.rs).

