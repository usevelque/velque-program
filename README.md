# velque-program

The on-chain program behind [Velque](https://usevelque.xyz), an order book for tokenized stocks on Solana built around the hours when Nasdaq is closed.

Nasdaq prices a stock for 32.5 hours a week. Tokenized stocks trade all 168. Velque changes how trading works when the exchange price disappears:

| Session | When | How orders match |
| --- | --- | --- |
| **Day** | The reference price is fresh (Nasdaq regular session) | Continuous book, price then time, inside a band around the reference |
| **Dark** | The reference has gone stale (nights, weekends, holidays, halts) | Orders collect in fixed windows and clear together at one price |
| **Opening cross** | The first clear after the reference comes back | The waiting window clears at once and hands its remainder to the Day book |

The session is not a clock inside the program. It follows the freshness of the reference price: if the oracle stops posting, the market is Dark.

## Status

Runs on Solana devnet. Not audited. Do not use it with funds you cannot afford to lose.

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

## What the keys can do

- **Oracle key** (market authority): post a reference price and hand the role to another key. It cannot move funds.
- **Upgrade authority**: upgrade the program. It is a different key and is not used by any online service.
- **Nobody** can withdraw from the vaults. Tokens leave escrow only as a fill, a refund on cancel, or a claim by the order's owner.

Token-2022 note: xStocks carry a permanent delegate and a pause switch that belong to the issuer. The issuer can move tokens out of any account, including the program's vault, and can pause transfers. The program cannot prevent either.

## Build and test

```bash
cargo build-sbf
cd tests && cargo run --release
```

The test binary first checks the clearing rule on the host, then loads the compiled `.so` into [LiteSVM](https://github.com/LiteSVM/litesvm) and runs the whole life of a market: a Token-2022 stock against an SPL quote token, night auctions, carry-over of until-cancelled orders, the opening cross, the Day book with its band, the end of the day, a full book, a dust order, and a cross into a full Day book. It prints one `PASS` line per check and settles every balance to the unit.

The tests load `../target/deploy/velque.so` by default. Set `VELQUE_SO` to use a different build.

## Check the deployed binary

The program on devnet is built from this repository with the committed `Cargo.lock`. To check it yourself:

```bash
cargo build-sbf
solana program dump MXG3VzXQucitJ4MSWWd1ddEat5FRS8KFF5j1uZRW7jz onchain.so -u devnet
head -c $(stat -c%s target/deploy/velque.so) onchain.so | sha256sum
sha256sum target/deploy/velque.so
```

Both lines should print `34f838b65909c04792430a4c986ad374b41ee35746e54c99140e55dcab27893c`. The dump is padded with zeros up to the allocated size, which is why it is trimmed first. Built with `cargo-build-sbf` 4.1.0 (platform-tools v1.54, rustc 1.89.0); a different toolchain can produce a different binary from the same source.

## Related

- [velque-sdk](https://github.com/usevelque/velque-sdk): JavaScript client and the clearing rule in JS
- [auction-replay](https://github.com/usevelque/auction-replay): recompute any auction from chain data
- [velque-keeper](https://github.com/usevelque/velque-keeper): the crank, the reference oracle and the test-market maker
- [velque-app](https://github.com/usevelque/velque-app): the web app

## License

MIT
