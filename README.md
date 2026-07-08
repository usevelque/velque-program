# velque-program

The on-chain program behind Velque, an order book for tokenized stocks on Solana built around the hours when Nasdaq is closed.

## How an auction clears

One price for everyone in the window, chosen in this order:

1. the price that trades the most volume;
2. if tied, the price that leaves the smallest imbalance;
3. if tied, the price closest to the reference;
4. if still tied, the midpoint of the remaining range, floored to the tick.

Orders priced better than the clearing price fill first. Orders exactly at the clearing price share what is left pro rata by size, and lot remainders go out in order of arrival. The rule lives in [`src/clearing.rs`](src/clearing.rs) and has no dependencies, so it runs the same on chain and on a laptop. Clearing a full book of 64 orders costs about 49k compute units.

