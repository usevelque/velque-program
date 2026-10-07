# Security

Velque runs on Solana devnet with test tokens. The program has not been audited. Treat it accordingly.

## Reporting a vulnerability

Use GitHub's private reporting: open the **Security** tab of this repository and press **Report a vulnerability**. The report stays private between you and the maintainers until a fix is out.

Please do not open a public issue for anything that could move funds, block a market or corrupt a book.

A useful report has:

- the instruction or account involved and what goes wrong;
- a transaction signature on devnet, or a failing case for `tests/src/main.rs`;
- what an attacker gains: funds, a stuck market, a wrong clearing price.

This is a small project. Every report is read, and a first reply usually takes a few days. There is no bounty program.

## In scope

- The program in this repository: escrow, clearing, fills, refunds, the day book, the opening cross, the oracle and upgrade roles.
- The clearing rule in [velque-sdk](https://github.com/usevelque/velque-sdk), where it disagrees with the program.
- The reference price logic in [velque-keeper](https://github.com/usevelque/velque-keeper), where bad data could reach `set_reference`.

## Known limits, not vulnerabilities

- The oracle key decides which session a market is in by posting or not posting the reference price. It cannot move funds.
- The program is upgradeable. The daily [on-chain binary](https://github.com/usevelque/velque-program/actions/workflows/verify.yml) check shows whether what is deployed still matches this source.
- xStocks are Token-2022 mints with a permanent delegate and a pause switch that belong to the issuer. No program can remove those powers.
- The test market's faucet and market maker hold devnet tokens with no value.
