# Aave v4 — protocol research for a user-position indexer

Research notes backing the indexer in this repo. Scope: **everything needed to reconstruct
user positions for one (Hub, Spoke) pair on one chain from event logs.**

The design must remain multi-hub / multi-spoke / multi-chain *capable* — every identifier
below is therefore treated as scoped, never global — but the first iteration indexes a
single configured pair.

Verified against mainnet on **2026-07-31** at block `25652535`. Facts are tagged:

- **[verified]** — read from `aave/aave-v4` source or confirmed by an RPC call made during this research
- **[docs]** — from official docs/blog, not independently checked
- **[open]** — needs confirmation before we depend on it

---

## 1. Why v4 is structurally different from v2/v3

In v2/v3 a `Pool` owned both the liquidity and the accounting, and a user position was
readable from `aToken` / `variableDebtToken` balances — ERC-20s with `Transfer` events.

v4 splits these **[docs]**:

- a **Hub** holds the actual assets and does system-wide accounting per `assetId`
- a **Spoke** holds *user-facing* state: who supplied what, who borrowed what, collateral
  flags, risk config, liquidation rules
- the Hub grants each Spoke a **credit line** (how much it may draw) and a **debit line**
  (how much it may add back)

Two consequences that drive the whole indexer design:

1. **There are no position tokens.** No `aToken`, no `Transfer` events to follow. A user
   position exists only as a struct in Spoke storage, mutated by Spoke events. Reconstructing
   it means folding the Spoke's event stream — there is no ERC-20 shortcut. **[verified]**
2. **User state lives on the Spoke; valuation lives on the Hub.** The Spoke knows a user's
   *shares*; converting shares to asset amounts needs the Hub's index, which accrues with
   time rather than with events. See §5 — this is the single biggest correctness trap here.

A user's position is keyed by **(spoke, reserveId, user)**. `reserveId` is a per-Spoke index,
*not* a protocol-wide asset id, so it must never be used as a global key. **[verified]**

---

## 2. Deployment surface (Ethereum mainnet)

Addresses from the official docs, each confirmed to have bytecode at block `25652535`
(all returned 1419 bytes, consistent with a uniform proxy). **[verified]**

| Role | Address |
|---|---|
| Core Hub | `0xCca852Bc40e560adC3b1Cc58CA5b55638ce826c9` |
| Plus Hub | `0x06002e9c4412CB7814a791eA3666D905871E536A` |
| Prime Hub | `0x943827DCA022D0F354a8a8c332dA1e5Eb9f9F931` |
| Global Dollar Hub | `0x62d63197660c080236193CA60b70E49A08E90368` |
| **Main Spoke** | `0x94e7A5dCbE816e498b89aB752661904E2F56c485` |
| Bluechip Spoke | `0x973a023A77420ba610f06b3858aD991Df6d85A08` |
| Lido eSpoke | `0xe1900480ac69f0B296841Cd01cC37546d92F35Cd` |
| Giver Position Manager | `0x17A54b8d6D9C68e7fa1C7112AC998EA1BA51d11e` |
| Taker Position Manager | `0x6c044c0D3801499bCAbfAd458B70880bc518e9F7` |
| Config Position Manager | `0x51305839CE822a7b4b12AA7D86eA7005052d575c` |

**Proposed first target: Main Spoke + Core Hub.** All 14 of the Main Spoke's reserves point
at the Core Hub, so this pair is self-contained — no cross-hub reads needed. **[verified]**

Earliest log emitted by the Main Spoke: block **24,720,899** (2026-03-23T14:45:59Z) — a week
before the public launch announcement, i.e. deploy-and-configure preceded launch. This is the
indexer's backfill genesis block. **[verified]**

Aave v4 also went live on Avalanche on 2026-07-15 **[docs]** — the second chain that would
exercise the multi-chain config path. Not indexed in iteration 1.

### Position managers are not the user

`Supply`/`Withdraw`/`Borrow`/`Repay` carry **both** `caller` and `user`. When a position
manager or gateway acts on someone's behalf, `caller` is the manager and `user` is the
position owner. **Positions must be keyed on `user`; `caller` is provenance only.** Getting
this backwards would attribute large parts of the book to three router addresses. **[verified]**

---

## 3. On-chain position storage

```solidity
struct UserPosition {     // per (spoke, reserveId, user)
  uint120 drawnShares;      // debt, in Hub shares
  uint120 premiumShares;    // risk-premium debt component
  int200  premiumOffsetRay; // premium accounting offset, RAY-scaled
  uint120 suppliedShares;   // collateral/supply, in Hub shares
  uint32  dynamicConfigKey; // which DynamicReserveConfig version applies to this user
}
```
**[verified]** — `src/spoke/interfaces/ISpoke.sol`

```solidity
struct Reserve {          // per (spoke, reserveId)
  address underlying;
  IHubBase hub;
  uint16  assetId;        // id *within that Hub*
  uint8   decimals;
  uint24  collateralRisk;
  ReserveFlags flags;     // uint8: paused/frozen/borrowable/receiveSharesEnabled
  uint32  dynamicConfigKey;
}
```
**[verified]**

Note `dynamicConfigKey` appears on both the reserve and the user position. Risk parameters are
**versioned**, and a user stays pinned to the config version they last refreshed to — that is
what `RefreshAllUserDynamicConfig` / `RefreshSingleUserDynamicConfig` do. To value a position
correctly you need *the user's* config version, not the reserve's current one. This is a real
modelling requirement, not a detail: `DynamicReserveConfig` rows must be stored keyed by
`(reserveId, dynamicConfigKey)` and never overwritten in place. **[verified]**

---

## 4. Event catalogue

Signatures extracted mechanically from the interface sources (structs flattened to tuples),
topics computed with `keccak256`, then **cross-checked against 38,580 real Main Spoke logs**
— every high-volume topic observed on-chain is accounted for below. **[verified]**

### 4.1 Spoke — position-mutating (the core of the indexer)

| topic0 | signature | observed |
|---|---|---|
| `0xd986db228cb1fe8392c5f45ff5f2c639b7db6cbd9ca7d1fe70b2de90c2c8c961` | `Supply(uint256,address,address,uint256,uint256)` | 10,006 |
| `0xef18174796a5d2f91d51dc5e907a4d7867bbd6e800f6225168e0453d581d0dcd` | `Borrow(uint256,address,address,uint256,uint256)` | 5,282 |
| `0xfe7813e2866053d5c3938554e517b554fce6666a6561bed9eaa7419b29fa9b68` | `Withdraw(uint256,address,address,uint256,uint256)` | 4,128 |
| `0xd765a0263e8a360da8dd4fdb8c0dc5553adec12a96f29a462cdb45e5bea407dd` | `Repay(uint256,address,address,uint256,uint256,(int256,int256,uint256))` | 2,548 |
| `0x2a1f12d996f530f89d8038aa293f9fde81cac44b6dfd6225e3358d09b78a4a37` | `LiquidationCall(uint256,uint256,address,address,bool,uint256,uint256,(int256,int256,uint256),uint256,uint256,uint256)` | 90 |
| `0x4763df430bc5274807f8ab4ce0734e7898513638418d6eec0c5285ef85f7f51f` | `SetUsingAsCollateral(uint256,address,address,bool)` | 3,198 |
| `0x59932f333b3a5e3fec86e662babe8dd767529ed207420e7468bd220cdfb3f076` | `ReportDeficit(uint256,address,uint256,(int256,int256,uint256))` | 0 |
| `0x4fd0c5440d5b8c1dd712c65f039f54384c59e81a139427b0a9155260d974a9a7` | `RefreshPremiumDebt(uint256,address,(int256,int256,uint256))` | 0 |

Field layout of the four hot events is identical and convenient:
`(uint256 indexed reserveId, address indexed caller, address indexed user, uint256 shares, uint256 amount)`.
Both `shares` and `amount` are emitted, so **per-event asset amounts are exact and need no
index maths** — the accrual problem in §5 applies to *balances over time*, not to the events.

`Repay` inserts `totalAmountRepaid` before the `PremiumDelta` tuple — a 6-field event where
the others have 5. An early version of these notes had this wrong and the topic did not match
any real log; the table above is the corrected, log-verified form.

`LiquidationCall` is indexed on `(collateralReserveId, debtReserveId, user)` — note the
**liquidator is not indexed**, so "positions liquidated by X" cannot be served by a topic
filter and needs a DB index.

### 4.2 Spoke — user config

| topic0 | signature | observed |
|---|---|---|
| `0x837314749a8459031ad895d39a13552d1627fddc93d64b404bab0ae5f0798da7` | `RefreshAllUserDynamicConfig(address)` | 8,696 |
| `0x5790b5f096c9cfee6b98a4e2d4f54ff3fc4ca306df5bc2093d93a36496d917b8` | `RefreshSingleUserDynamicConfig(address,uint256)` | 3,165 |
| `0x413bea992b9956f4f10f6c819bf7a6c8ed5baa119a2901fe221ae03171d52277` | `SetUserPositionManager(address,address,bool)` | 1,388 |
| `0x9a9082fd74a00ac52b567642a2d8fd3383cb2bd8690f6b2a3b7b37aaf489dac1` | `UpdateUserRiskPremium(address,uint256)` | 0 |

`RefreshAllUserDynamicConfig` is the **single most frequent event on the Spoke** (8,696 —
more than `Supply`). It is emitted on config refresh and carries no amounts, but it changes
which risk parameters apply to the user, so it must be ingested even though it moves no value.

### 4.3 Spoke — reserve/market config

| topic0 | signature | observed |
|---|---|---|
| `0xb2d3221c3db1eb0d586556ae23399acdfe3e52ff0fcd184c19069c730f9ca2e9` | `AddReserve(uint256,uint256,address)` | 14 |
| `0x18a45d070f507b6387b78837652d7468e733927acc7f9a13d9cc308675735c08` | `UpdateReservePriceSource(uint256,address)` | 25 |
| `0xe9495512a0eb05fe0cbdd52286bdeb54cb8e5a8d50e7e17d75f75903a98e2af8` | `UpdateReserveConfig(uint256,(uint24,bool,bool,bool,bool))` | 14 |
| `0xfcede5501ba87e3766118ae6ed360a87ee9b6570156ae9cac52d35ff0de0403b` | `AddDynamicReserveConfig(uint256,uint32,(uint16,uint32,uint16))` | 14 |
| `0x2d4f2760aaff0dfa53526a8fdd306864689a7d5e43f44ddfeece0f38315c298d` | `UpdateDynamicReserveConfig(uint256,uint32,(uint16,uint32,uint16))` | 0 |
| `0x8e04e916c2b397f8ab1cf9a55e94728a44837b3751f72369339ad991d371edc4` | `UpdatePositionManager(address,bool)` | 5 |
| `0x9062eec1933c38394d82dc926d7ddcd777a5cd08e1ae6baa94e90047338d3459` | `UpdateLiquidationConfig((uint128,uint64,uint16))` | 2 |
| `0x6d87c7e547bc13244d61719fa011b6947b26036a16d69a607c1cf72a77d052bc` | `SetSpokeImmutables(address,uint16)` | 1 |

The 14 `AddReserve` events are the authoritative reserve registry — **the indexer can
bootstrap `reserveId → (hub, assetId, underlying)` purely from logs, with no `eth_call`.**
That matters: it keeps backfill archive-free (§7).

### 4.4 Hub events (not needed for iteration 1)

`Add` / `Remove` / `Draw` / `Restore` / `RefreshPremium` / `ReportDeficit` / `TransferShares`,
all keyed `(assetId, spoke)`, plus configurator events. **[verified]**

These are Spoke-aggregate flows, not per-user, so they are not required to reconstruct user
positions. They become relevant for share→asset conversion (§5) and for cross-spoke views.

Two `ReportDeficit` events exist with **different signatures** — the Spoke's 4-arg form
(`0x59932f…`) and the Hub's 5-arg form (`0x4845ee…`). Decoding must be scoped by emitting
address, not by name. **[verified]**

### 4.5 Topic-collision warning

`Add`, `Remove`, `Draw`, `Supply` etc. are generic names. Since the indexer will eventually
watch several contracts, **every log must be decoded against the ABI of its emitting address**,
never by topic0 alone across a merged stream.

---

## 5. The shares problem — the main correctness risk

Events carry **shares** and the **amount at that moment**. Balances are stored as shares.
Between two events a user's debt grows because the Hub's `drawnIndex` accrues with *time*,
and that accrual **emits no event**.

So: summing `Supply.amount − Withdraw.amount` gives *net principal flow*, **not** the current
balance. A pure event-fold yields correct **share** balances and correct **historical flows**,
but current asset balances require the Hub index.

Three options, in increasing cost:

1. **Store shares as the source of truth; convert on read.** Fold events to
   `suppliedShares` / `drawnShares` (exact, event-only, archive-free), then convert to assets
   at query time using the *current* Hub index via one `eth_call` at `latest`. Correct, cheap,
   and never needs historical state. **Recommended.**
2. **Periodic snapshot.** Additionally call `getUserSuppliedAssets` / `getUserTotalDebt` /
   `getUserAccountData` on a cadence, store as enriched rows. Gives point-in-time asset values
   and a natural **reconciliation check** against the folded shares — a strong correctness story.
3. **Reimplement the index accrual** off-chain from the interest-rate strategy. Highest
   fidelity for historical valuation, high effort, easy to get subtly wrong. Out of scope.

Plan: **(1) as the ledger + (2) as enrichment and drift detection.** Any invariant test should
assert folded shares equal `getUserSuppliedShares` / on-chain shares, *not* asset amounts.

`premiumShares` / `premiumOffsetRay` (the risk-premium component) add a second accrual track
on top of this. **[open]** — worth confirming whether premium can be reconstructed from
`PremiumDelta` alone before promising premium-accurate debt in the API.

---

## 6. Read functions — reconciliation and enrichment

All on the Spoke unless noted; all confirmed callable at `latest`. **[verified]**

```solidity
getUserAccountData(address) returns (UserAccountData)   // riskPremium, avgCollateralFactor,
                                                        // healthFactor, totalCollateralValue,
                                                        // totalDebtValueRay,
                                                        // activeCollateralCount, borrowCount
getUserPosition(uint256 reserveId, address user)
getUserSuppliedShares / getUserSuppliedAssets(uint256, address)
getUserDebt / getUserTotalDebt / getUserPremiumDebtRay(uint256, address)
getUserReserveStatus(uint256, address) returns (bool, bool)
getReserveCount / getReserve / getReserveConfig / getDynamicReserveConfig(uint256, uint32)
getReserveSuppliedAssets / getReserveTotalDebt(uint256)
getLiquidationConfig() / getLiquidationBonus(...)
```

`getUserAccountData` is the highest-value single call: **health factor** in WAD plus
aggregate collateral/debt value, computed by the protocol itself. Sampled on real borrowers:

| user | HF | activeCollateral | borrows |
|---|---|---|---|
| `0xd2b70EfbF41cF73ABf59adBa08Afaa6d114B56C8` | 1.168 | 3 | 1 |
| `0x29f87413ccDE6e872853a4f7b5D43Dd31d44198c` | 1.272 | 1 | 1 |
| `0xd7AD196009fBe5c4210DB626719AF5439D43e5B9` | 1.962 | 1 | 1 |
| `0x03BD789D919e47D7759E9Cbb5f8A565bc293FcD3` | 2.586 | 1 | 1 |

**[open]** — `totalCollateralValue` and `totalDebtValueRay` are in "units of Value" with
`totalDebtValueRay` RAY-scaled. The exact base unit of `Value` is not yet pinned down; my
sample decode suggests 8-decimal USD-ish for collateral, but **do not ship a USD figure from
these until the unit is confirmed** — read `AaveOracle` / `SpokeUtils` to settle it.

---

## 7. Ingestion constraints

### Archive is not required — as long as we never read historical *state*

Worth separating clearly, because public RPCs blur it commercially:

- **historical state** (`eth_call`, `eth_getCode`, `eth_getStorageAt` at an old block) →
  needs an archive node. **Out of scope per the task brief, and our design avoids it entirely.**
- **historical logs** (`eth_getLogs` over old ranges) → retained by any full node; not an
  archive feature.

The design in §5 (fold logs for history; `eth_call` only at `latest`) satisfies this: backfill
touches **logs only**, so no archive node is ever needed. This is a deliberate constraint to
state in the README, not an accident.

Caveat: some providers gate `eth_getLogs` history behind an "archive" plan anyway. That is a
billing policy, not a protocol requirement — but it does mean **provider choice matters** and
the RPC URL must be configuration, with the indexer tolerating per-provider range limits.

### Measured provider behaviour (2026-07-31) **[verified]**

| endpoint | historical `eth_getLogs` | max range |
|---|---|---|
| `eth.drpc.org` | yes | 10,000 blocks (free tier) |
| `rpc.mevblocker.io` | yes | ≥10,000 blocks |
| `ethereum-rpc.publicnode.com` | **no** — ~128 blocks, then "archive token required" | ~128 |
| `1rpc.io/eth` | yes | 50 blocks |
| `eth-pokt.nodies.app` | yes | 50 blocks |
| `rpc.ankr.com/eth` | requires API key | — |
| `eth.meowrpc.com` | `eth_getLogs` unsupported | — |

Implication: **the chunk size must be a config value**, and the backfiller should degrade
gracefully (halve the range on "range too large"). Ranges from 50 to 10,000 all need to work.

### Volume — backfill is cheap

Main Spoke, genesis `24,720,899` → `25,652,464` (931,565 blocks): **38,580 logs**, retrieved
in **101 requests** of 10k blocks. Full backfill is minutes, not hours, and fits comfortably
in a free tier. No need for a bulk-data provider or a parallel-partition backfiller in
iteration 1 — a sequential chunked backfill is genuinely sufficient. **[verified]**

### Reorgs

Nothing v4-specific, but the brief calls it out. Store `block_number` + `block_hash` per event,
keep the last N blocks (~64–128) "unfinalized", and on a parent-hash mismatch roll back and
re-scan. Because positions are a **fold over events**, rollback means deleting events above the
fork point and replaying — which argues for keeping the raw event log as an immutable table and
treating position balances as a derived projection.

### Local development

`scripts/deploy/examples/AaveV4DeployAnvil.s.sol` in `aave/aave-v4` deploys the **whole
protocol to a local Anvil** (chainId 31337, 2 hubs, 3 spokes) with documented steps.
**[verified — script read, not yet executed]** This is the strongest option for tests: real
contracts, real events, deterministic, no rate limits, and Anvil can force reorgs to test the
rollback path. Requires Foundry, which is **not currently installed** on this machine.

---

## 8. Live state — Main Spoke, block 25652535 **[verified]**

14 reserves, all on the Core Hub.

| id | symbol | dec | supplied | debt | CF% |
|---|---|---|---|---|---|
| 0 | WETH | 18 | 19,515.85 | 289.65 | 83.0 |
| 1 | wstETH | 18 | 8,002.43 | 0.00 | 80.0 |
| 2 | weETH | 18 | 707.73 | 0.00 | 80.0 |
| 3 | WBTC | 8 | 668.31 | 2.17 | 78.0 |
| 4 | cbBTC | 8 | 167.11 | 0.11 | 78.0 |
| 5 | AAVE | 18 | 18,160.76 | 0.00 | 76.0 |
| 6 | LINK | 18 | 378,423.50 | 0.00 | 71.0 |
| 7 | USDC | 6 | 6,762,360.60 | 7,452,352.68 | 78.0 |
| 8 | USDT | 6 | 12,652,413.90 | 7,421,216.12 | 78.0 |
| 9 | EURC | 6 | 30,650.98 | 60,469.69 | 0.0 |
| 10 | RLUSD | 18 | 22.41 | 0.15 | 0.0 |
| 11 | USDG | 6 | 61,105,348.06 | 20,172,185.37 | 0.0 |
| 12 | frxUSD | 18 | 29,598,508.08 | 11,485,140.11 | 0.0 |
| 13 | GHO | 18 | 699,458.68 | 635,952.95 | 0.0 |

Two things to design for, visible in this table:

- **`CF = 0` for reserves 9–13** — these are borrow-only, never collateral. A position row can
  legitimately have supply in a zero-CF reserve contributing nothing to collateral value.
- **debt > supplied on reserves 7, 9, 13.** Not an inconsistency: the Spoke draws from *shared
  Hub liquidity*, so per-spoke debt is not bounded by per-spoke supply. Any "utilisation"
  metric computed per spoke would be wrong and must be sourced from the Hub. This is exactly
  the kind of v3 intuition that breaks in v4.

---

## 9. Enrichment candidates

The brief needs ≥1 additional source, exposed alongside indexed data.

- **Off-chain USD prices — DefiLlama** (`coins.llama.fi`): keyless, batch, returns
  `{price, decimals, symbol, confidence, timestamp}` per `ethereum:0x…`. Tested working.
  Turns share balances into USD position values. **[verified]**
  - CoinGecko's free tier now allows **1 contract address per request** — unusable for batch
    pricing without a paid key. **[verified]** DefiLlama is the better default.
- **On-chain health factor** via `getUserAccountData` — protocol-computed risk, cheap, exact.
  Arguably "the same source" rather than an additional one, so best as a *second* enrichment
  rather than the headline one.
- ENS / address labels — nice-to-have, low value here.

Recommendation: **DefiLlama prices as the headline enrichment** (clearly a separate source,
separate cadence, separate failure mode, needs caching + staleness handling — all of which
demonstrate the pipeline design the brief is asking about), with `getUserAccountData` sampling
as a second, on-chain enrichment.

---

## 10. Open questions

1. Unit of `Value` in `getUserAccountData` (§6) — blocks any USD figure derived from it.
2. Whether premium debt is fully reconstructable from `PremiumDelta` deltas alone (§5).
3. Do position managers ever emit position events from *their own* address, or always via the
   Spoke? Assumed Spoke-only; affects whether we watch >1 address.
4. Exact `Reserve.flags` bit order in `ReserveFlagsMap.sol` — needed to decode paused/frozen.
5. Whether `TransferShares` on the Hub can move a *user's* position between spokes. If yes,
   it is position-affecting and §4.4's "Hub events not needed" is wrong.

---

## Sources

- [aave/aave-v4](https://github.com/aave/aave-v4) — interfaces under `src/spoke/interfaces/`,
  `src/hub/interfaces/`; Anvil deploy script under `scripts/deploy/examples/`
- [Aave v4 docs](https://aave.com/docs/aave-v4) · [addresses](https://aave.com/docs/resources/addresses) · [liquidity model](https://aave.com/docs/aave-v4/liquidity)
- [Aave V4 is Live on Ethereum](https://aave.com/blog/aave-v4-live-ethereum) · [Understanding Aave V4's Architecture](https://aave.com/blog/understanding-aave-v4s-architecture)
- [Anatomy of the Aave v4 contracts](https://jeancvllr.medium.com/anatomy-of-the-aave-v4-contracts-364fa3189d04)
- Live mainnet reads via `eth.drpc.org`, 2026-07-31, block `25652535`
