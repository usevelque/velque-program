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

## How an auction clears

One price for everyone in the window, chosen in this order:

1. the price that trades the most volume;
2. if tied, the price that leaves the smallest imbalance;
3. if tied, the price closest to the reference;
4. if still tied, the midpoint of the remaining range, floored to the tick.

Orders priced better than the clearing price fill first. Orders exactly at the clearing price share what is left pro rata by size, and lot remainders go out in order of arrival. The rule lives in [`src/clearing.rs`](src/clearing.rs) and has no dependencies, so it runs the same on chain and on a laptop. Clearing a full book of 64 orders costs about 49k compute units.

