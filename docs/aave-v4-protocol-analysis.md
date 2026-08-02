# Aave v4 — protocol research for a user-position indexer

Research notes backing the indexer in this repo. Scope: **everything needed to reconstruct
user positions for one (Hub, Spoke) pair on one chain from event logs.**

The design must remain multi-hub / multi-spoke / multi-chain *capable* — every identifier
below is therefore treated as scoped, never global — but the first iteration indexes a
single configured pair.

Verified against mainnet on **2026-07-31** at block `25652535`. Facts are tagged:

- **[verified]** — read from `aave/aave-v4` source, or confirmed by an RPC call or a local
  Anvil deployment during this research
- **[docs]** — from official docs/blog, not independently checked

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
   *shares*; converting shares to asset amounts needs Hub state that accrues with time rather
   than with events. This is the central correctness problem — and it is solvable entirely
   off-chain, because the Hub emits its own interest index. See §5.

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

The managers also emit their **own** mirror events — [`SupplyOnBehalfOf`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/position-manager/interfaces/IGiverPositionManager.sol#L17-L24),
[`BorrowOnBehalfOf`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/position-manager/interfaces/ITakerPositionManager.sol#L100-L107), [`RepayOnBehalfOf`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/position-manager/interfaces/IGiverPositionManager.sol#L33-L40),
[`SetUsingAsCollateralOnBehalfOf`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/position-manager/interfaces/IConfigPositionManager.sol#L106-L112), at 2–7 logs per 10k blocks. **[verified]**
These are provenance records, not authoritative state; the Spoke's own events remain the source
of truth. The hazard is therefore **double-counting, not missing data** — an indexer that
matched position-like events across every Aave address would book each action twice. Ingest
them only if you want attribution, and never into the balance fold.

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

`ReserveFlags` bit order, from [`ReserveFlagsMap.sol:11-17`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/libraries/ReserveFlagsMap.sol#L11-L17):
`PAUSED = 0x01`, `FROZEN = 0x02`, `BORROWABLE = 0x04`, `RECEIVE_SHARES_ENABLED = 0x08`.
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
| `0xd986db228cb1fe8392c5f45ff5f2c639b7db6cbd9ca7d1fe70b2de90c2c8c961` | [`Supply(uint256,address,address,uint256,uint256)`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L197-L203) | 10,006 |
| `0xef18174796a5d2f91d51dc5e907a4d7867bbd6e800f6225168e0453d581d0dcd` | [`Borrow(uint256,address,address,uint256,uint256)`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L225-L231) | 5,282 |
| `0xfe7813e2866053d5c3938554e517b554fce6666a6561bed9eaa7419b29fa9b68` | [`Withdraw(uint256,address,address,uint256,uint256)`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L211-L217) | 4,128 |
| `0xd765a0263e8a360da8dd4fdb8c0dc5553adec12a96f29a462cdb45e5bea407dd` | [`Repay(uint256,address,address,uint256,uint256,(int256,int256,uint256))`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L240-L247) | 2,548 |
| `0x2a1f12d996f530f89d8038aa293f9fde81cac44b6dfd6225e3358d09b78a4a37` | [`LiquidationCall(uint256,uint256,address,address,bool,uint256,uint256,(int256,int256,uint256),uint256,uint256,uint256)`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L261-L273) | 90 |
| `0x4763df430bc5274807f8ab4ce0734e7898513638418d6eec0c5285ef85f7f51f` | [`SetUsingAsCollateral(uint256,address,address,bool)`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L292-L297) | 3,198 |
| `0x59932f333b3a5e3fec86e662babe8dd767529ed207420e7468bd220cdfb3f076` | [`ReportDeficit(uint256,address,uint256,(int256,int256,uint256))`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L280-L285) | 0 |
| `0x4fd0c5440d5b8c1dd712c65f039f54384c59e81a139427b0a9155260d974a9a7` | [`RefreshPremiumDebt(uint256,address,(int256,int256,uint256))`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L323-L327) | 0 |

Field layout of the four hot events is identical and convenient:
`(uint256 indexed reserveId, address indexed caller, address indexed user, uint256 shares, uint256 amount)`.
Both `shares` and `amount` are emitted, so **per-event asset amounts are exact and need no
index maths** — the accrual problem in §5 applies to *balances over time*, not to the events.

`Repay` inserts `totalAmountRepaid` before the `PremiumDelta` tuple — a 6-field event where
the others have 5. Every topic above is confirmed against real logs; deriving these signatures
by hand rather than from source is an easy way to get `Repay` wrong and match nothing.

#### Liquidations

`LiquidationCall` is indexed on `(collateralReserveId, debtReserveId, user)` — note the
**liquidator is not indexed**, so "positions liquidated by X" cannot be served by a topic
filter and needs a DB index.

The event itself is complete: borrower, liquidator, both reserves, debt restored, collateral
removed, and the share deltas on both sides. Detecting and attributing a liquidation from logs
alone is straightforward. Three details are not obvious from the signature, all confirmed by
replaying real liquidations on Anvil: **[verified]**

1. **One liquidation is not one event.** `liquidationCall` operates on a single
   `(collateral, debt)` pair, so liquidating a user with several collaterals or debts emits
   several `LiquidationCall` logs in one transaction. A user-facing "liquidation" is a group
   keyed by `(txHash, user)`, not a row per log.

2. **Bad debt is a *separate* event.** When collateral is exhausted and debt remains, the Spoke
   emits `ReportDeficit(reserveId, user, drawnShares, premiumDelta)` alongside the
   `LiquidationCall`, and that is what removes the written-off `drawnShares` from the position.
   An indexer folding only `LiquidationCall` leaves the borrower carrying phantom debt forever.
   Observed: a `LiquidationCall` clearing `4.95e16` drawn shares, immediately followed by
   `ReportDeficit` writing off the remaining `2.95e18`.

3. **When `receiveShares` is true, the liquidator's position grows with no `Supply` event.**
   The collateral never leaves the Hub; ownership moves inside the Spoke, and the only record
   is `collateralSharesToLiquidator` on the `LiquidationCall` itself. Verified: after such a
   liquidation the liquidator's `suppliedShares` equalled `collateralSharesToLiquidator`
   exactly, with no `Supply` log anywhere in the trace. **Crediting supplied shares only on
   `Supply` silently under-counts every liquidator.**

The protocol's liquidation fee is the gap between `collateralSharesLiquidated` and
`collateralSharesToLiquidator`, and settles as a Hub `TransferShares` to the treasury spoke —
consistent with §4.4: spoke-to-spoke, never touching a user position.

**How much of this mainnet actually exercises.** Decoding all 90 `LiquidationCall` events over
full history: **[verified]**

| | |
|---|---|
| events / distinct transactions | 90 / 88 |
| transactions with >1 `LiquidationCall` | **2** (max 2 in one tx) |
| `receiveShares = true` | **0 (0%)** |
| accompanied by `ReportDeficit` | **0** — no bad debt has ever occurred here |
| distinct liquidators / borrowers | 18 / 61 |

So of the three subtleties, only the multi-event grouping is live today. **The share-receiving
and bad-debt branches have never fired on mainnet** — they sit in the same category as premium
and deficit (§5.4): reachable code, zero production data, exercised only on Anvil. The zero
`ReportDeficit` count is the event-side confirmation of the `deficitRay = 0` reading across all
34 assets.

That is an argument for implementing them now rather than later. They are cheap to fold while
the transition table is fresh, they cannot be tested against production data when they do
appear, and the first `receiveShares` liquidation would otherwise silently under-count a
liquidator with nothing in the logs to indicate a problem.

### 4.2 Spoke — user config

| topic0 | signature | observed |
|---|---|---|
| `0x837314749a8459031ad895d39a13552d1627fddc93d64b404bab0ae5f0798da7` | [`RefreshAllUserDynamicConfig(address)`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L306) | 8,696 |
| `0x5790b5f096c9cfee6b98a4e2d4f54ff3fc4ca306df5bc2093d93a36496d917b8` | [`RefreshSingleUserDynamicConfig(address,uint256)`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L311) | 3,165 |
| `0x413bea992b9956f4f10f6c819bf7a6c8ed5baa119a2901fe221ae03171d52277` | [`SetUserPositionManager(address,address,bool)`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L317) | 1,388 |
| `0x9a9082fd74a00ac52b567642a2d8fd3383cb2bd8690f6b2a3b7b37aaf489dac1` | [`UpdateUserRiskPremium(address,uint256)`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L302) | 0 |

`RefreshAllUserDynamicConfig` is the **single most frequent event on the Spoke** (8,696 —
more than `Supply`). It is emitted on config refresh and carries no amounts, but it changes
which risk parameters apply to the user, so it must be ingested even though it moves no value.

### 4.3 Spoke — reserve/market config

| topic0 | signature | observed |
|---|---|---|
| `0xb2d3221c3db1eb0d586556ae23399acdfe3e52ff0fcd184c19069c730f9ca2e9` | [`AddReserve(uint256,uint256,address)`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L152) | 14 |
| `0x18a45d070f507b6387b78837652d7468e733927acc7f9a13d9cc308675735c08` | [`UpdateReservePriceSource(uint256,address)`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L162) | 25 |
| `0xe9495512a0eb05fe0cbdd52286bdeb54cb8e5a8d50e7e17d75f75903a98e2af8` | [`UpdateReserveConfig(uint256,(uint24,bool,bool,bool,bool))`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L157) | 14 |
| `0xfcede5501ba87e3766118ae6ed360a87ee9b6570156ae9cac52d35ff0de0403b` | [`AddDynamicReserveConfig(uint256,uint32,(uint16,uint32,uint16))`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L170-L174) | 14 |
| `0x2d4f2760aaff0dfa53526a8fdd306864689a7d5e43f44ddfeece0f38315c298d` | [`UpdateDynamicReserveConfig(uint256,uint32,(uint16,uint32,uint16))`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L180-L184) | 0 |
| `0x8e04e916c2b397f8ab1cf9a55e94728a44837b3751f72369339ad991d371edc4` | [`UpdatePositionManager(address,bool)`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L189) | 5 |
| `0x9062eec1933c38394d82dc926d7ddcd777a5cd08e1ae6baa94e90047338d3459` | [`UpdateLiquidationConfig((uint128,uint64,uint16))`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L146) | 2 |
| `0x6d87c7e547bc13244d61719fa011b6947b26036a16d69a607c1cf72a77d052bc` | [`SetSpokeImmutables(address,uint16)`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L142) | 1 |

The 14 `AddReserve` events are the authoritative reserve registry — **the indexer can
bootstrap `reserveId → (hub, assetId, underlying)` purely from logs, with no `eth_call`.**
That matters: it keeps backfill archive-free (§8).

### 4.4 Hub events — required for valuation

`Add` / `Remove` / `Draw` / `Restore` / `RefreshPremium` / `ReportDeficit` / `TransferShares` /
`MintFeeShares` / `Sweep` / `Reclaim` / `EliminateDeficit`, all keyed `(assetId, spoke)`, plus
`UpdateAsset` and configurator events. **[verified]**

None of these are per-user, so they are not needed to reconstruct *share* balances. But they
carry the Hub asset state that share→asset conversion depends on, and `UpdateAsset` carries the
interest index itself — see §5.3. **Ingest them.**

| topic0 | signature |
|---|---|
| `0xa1facf110ded5028ee267fa3d5986f2aa4dc14230b79ffd27e95760f14883350` | [`UpdateAsset(uint256,uint256,uint256,uint256)`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/interfaces/IHub.sol#L113-L118) |
| `0xb233dd05ed21346e144167b35a6213bcf04768dbdffdc8339e8b027b94b9f305` | [`Add(uint256,address,uint256,uint256)`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/interfaces/IHubBase.sol#L23) |
| `0x535be2ff85ab4c5d0991e10dc057a4951ea2bac426ffb036eded23036a3942b2` | [`Remove(uint256,address,uint256,uint256)`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/interfaces/IHubBase.sol#L30) |
| `0xe2497bc41b1fa7c4ba996f24dc2affdffb2a5571584db6db0eed8fbbf1dc8517` | [`Draw(uint256,address,uint256,uint256)`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/interfaces/IHubBase.sol#L37-L42) |
| `0x119e7f996dc987b3ae79eb3735f1620c4292f6a7761a1e0f834c445f7798b912` | [`Restore(uint256,address,uint256,(int256,int256,uint256),uint256,uint256)`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/interfaces/IHubBase.sol#L51-L58) |
| `0x3fa96ecf17429fddfbb919a64196f4e43f71b57f0c5c38c49a21c8e1e763d18c` | [`RefreshPremium(uint256,address,(int256,int256,uint256))`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/interfaces/IHubBase.sol#L64) |
| `0x4845ee5c72bde2b62defc8a1ca2f0fc3313b2d9e799997ce4f6776da9773bcbf` | [`ReportDeficit(uint256,address,uint256,(int256,int256,uint256),uint256)`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/interfaces/IHubBase.sol#L72-L78) |
| `0xe97b8576ac531cdc817b933309d0518ca3d26c6b46d490f3ae9fa39426a141ee` | [`EliminateDeficit(uint256,address,address,uint256,uint256)`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/interfaces/IHub.sol#L166-L172) |
| `0xafd21228e21de4a3f779e1cc3617e12672c3da091dcf3812a931036aa0bf633c` | [`MintFeeShares(uint256,address,uint256,uint256)`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/interfaces/IHub.sol#L141-L146) |
| `0x69bb3893073d7a893f3933f3871309fc25acfc72e365b71f554d439a85b20e8b` | [`Sweep(uint256,address,uint256)`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/interfaces/IHub.sol#L152) |
| `0x566111831db1f090374baff3c3f9fc512084f5a9b8f5b199fb475d9c43a8013f` | [`Reclaim(uint256,address,uint256)`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/interfaces/IHub.sol#L158) |
| `0x0d93b0e8579bc9db73c85a1fb79d785ffc47f8e20d346253f809cc98c48292a0` | [`TransferShares(uint256,address,address,uint256)`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/interfaces/IHubBase.sol#L85-L90) |

Volume is modest: 868 Core Hub logs per 10k blocks, half of them `UpdateAsset`. **[verified]**

`TransferShares` is **not position-affecting**.
[`Hub._transferShares:721-728`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/Hub.sol#L721-L728) moves `addedShares` between two
`SpokeData` records only: asset-level totals net to zero and `_userPositions` — which live on
the Spoke, not the Hub — are untouched. It is inter-spoke rebalancing. It matters only if we
track per-spoke share subtotals, never for a user's balance. **[verified]**

Two `ReportDeficit` events exist with **different signatures** — the Spoke's 4-arg form
(`0x59932f…`) and the Hub's 5-arg form (`0x4845ee…`). Decoding must be scoped by emitting
address, not by name. **[verified]**

### 4.5 Topic-collision warning

`Add`, `Remove`, `Draw`, `Supply` etc. are generic names. Since the indexer will eventually
watch several contracts, **every log must be decoded against the ABI of its emitting address**,
never by topic0 alone across a merged stream.

---

## 5. Shares → assets: closed-form, and reproducible off-chain

Balances are stored as shares. Between events, debt grows because the Hub's `drawnIndex`
accrues with *time*, and that accrual emits no event. So summing `Supply.amount −
Withdraw.amount` gives *net principal flow*, **not** current balance.

The question is whether we must `eth_call` for the conversion. **We don't.** The math is a
closed form over Hub asset state, and that state is fully available from logs. Verified
wei-exact against mainnet (see below).

> **Source pinning.** Every formula below is transcribed from `aave/aave-v4` at commit
> [`2524fe4`](https://github.com/aave/aave-v4/tree/2524fe4018a42750300e114f2a8c4355df62a878)
> (`main` as of 2026-07-31). Line numbers refer to that commit and will drift on `main` —
> re-pin before trusting them. Paths are repo-relative.

### 5.1 Debt side — index-based

```
drawnIndex(t) = rayMulUp(drawnIndex_ckpt, RAY + drawnRate * (t - t_ckpt) / SECONDS_PER_YEAR)
debt(user)    = rayMulUp(user.drawnShares, drawnIndex(t))
premiumRay    = premiumShares * drawnIndex(t) - premiumOffsetRay
totalDebt     = debt(user) + ceilRay(premiumRay)
```

| element | source |
|---|---|
| index accrual, incl. the `drawnShares == 0 && premiumShares == 0` short-circuit | [`AssetLogic.getDrawnIndex:153-165`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/libraries/AssetLogic.sol#L153-L165) |
| linear interest term | [`MathUtils.calculateLinearInterest:20-31`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/libraries/math/MathUtils.sol#L20-L31) |
| `RAY = 1e27`, `SECONDS_PER_YEAR = 365 days` | [`MathUtils.sol:11`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/libraries/math/MathUtils.sol#L11), [`:13`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/libraries/math/MathUtils.sol#L13) |
| `drawnShares × index` | [`UserPositionUtils.getDebt:127-133`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/libraries/UserPositionUtils.sol#L127-L133) |
| premium term | [`Premium.calculatePremiumRay:17-23`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/libraries/Premium.sol#L17-L23), called from [`UserPositionUtils.sol:154-164`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/libraries/UserPositionUtils.sol#L154-L164) |
| `rayMulUp` = `ceil(a*b/RAY)` | [`WadRayMath.rayMulUp:88-98`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/libraries/math/WadRayMath.sol#L88-L98) |
| `ceilRay` = `fromRayUp` = `ceil(a/RAY)` | [`WadRayMath.fromRayUp:167-172`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/libraries/math/WadRayMath.sol#L167-L172) |

Interest is **linear between checkpoints, compounding only when a checkpoint lands**.
`SECONDS_PER_YEAR = 365 days`, leap years ignored. Rounding is **up** throughout and must be
replicated exactly. **[verified]**

### 5.2 Supply side — ERC4626-style virtual shares

Not an index. `SharesMath` uses virtual assets/shares (`1e6` each) as anti-manipulation
padding:

```
assets = shares * (totalAddedAssets + 1e6) / (addedShares + 1e6)     // floor

totalAddedAssets = liquidity + swept + ceilRay(aggregatedOwedRay)
                   - realizedFees - unrealizedFees(drawnIndex)
aggregatedOwedRay = drawnShares * drawnIndex + premiumRay + deficitRay
premiumRay        = premiumShares * drawnIndex - premiumOffsetRay
unrealizedFees    = (ceilRay(aggAfter) - ceilRay(aggBefore)) * liquidityFee / 10000
```

| element | source |
|---|---|
| virtual-share conversion, `Math.Rounding.Floor` | [`SharesMath.toAssetsDown:31-42`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/libraries/SharesMath.sol#L31-L42) |
| `VIRTUAL_ASSETS = VIRTUAL_SHARES = 1e6` | [`SharesMath.sol:13-14`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/libraries/SharesMath.sol#L13-L14) |
| wrapper supplying `totalAddedAssets` / `addedShares` | [`AssetLogic.toAddedAssetsDown:107-112`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/libraries/AssetLogic.sol#L107-L112) |
| `totalAddedAssets` | [`AssetLogic.totalAddedAssets:79-96`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/libraries/AssetLogic.sol#L79-L96) |
| `aggregatedOwedRay` | [`AssetLogic._calculateAggregatedOwedRay:229-242`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/libraries/AssetLogic.sol#L229-L242) |
| `unrealizedFees` | [`AssetLogic.getUnrealizedFees:187-226`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/libraries/AssetLogic.sol#L187-L226) |
| `* liquidityFee / 10000`, floor | [`PercentageMath.percentMulDown:15-27`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/libraries/math/PercentageMath.sol#L15-L27), `PERCENTAGE_FACTOR = 1e4` at [`:10`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/libraries/math/PercentageMath.sol#L10) |

Note the supply side depends on the **debt** index — suppliers are paid out of accrued debt,
so the two sides are coupled through `drawnIndex`. **[verified]**

### 5.2b Call chain — how the contracts reach these

Worth recording, because it is what makes the reconciliation job (§9) meaningful: our off-chain
code and the on-chain getters must bottom out in the same primitives.

**Supply** — [`Spoke.getUserSuppliedAssets:573-580`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/Spoke.sol#L573-L580)
→ [`Hub.previewRemoveByShares:471-473`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/Hub.sol#L471-L473)
→ [`AssetLogic.toAddedAssetsDown:107-112`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/libraries/AssetLogic.sol#L107-L112)
→ [`SharesMath.toAssetsDown:31-42`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/libraries/SharesMath.sol#L31-L42)

**Debt** — [`Spoke.getUserDebt:589-597`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/Spoke.sol#L589-L597)
→ [`UserPositionUtils.getDebt(hub, assetId):114-120`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/libraries/UserPositionUtils.sol#L114-L120)
→ [`Hub.getAssetDrawnIndex:508-510`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/Hub.sol#L508-L510)
→ [`AssetLogic.getDrawnIndex:153-165`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/libraries/AssetLogic.sol#L153-L165)
→ back to [`UserPositionUtils.getDebt(drawnIndex):127-133`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/libraries/UserPositionUtils.sol#L127-L133)

Both entry points read `_userPositions[user][reserveId]` on the Spoke and divide against
**Hub-global** totals — confirming that user shares and Hub `addedShares` share one unit,
with no per-spoke scaling. This is what the wei-exact match in §5.4 independently demonstrates.

### 5.3 The key finding: the Hub emits its own index

[`AssetLogic.updateDrawnRate:132-138`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/libraries/AssetLogic.sol#L132-L138) emits, at [`:137`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/libraries/AssetLogic.sol#L137):

```solidity
emit IHub.UpdateAsset(assetId, drawnIndex, newDrawnRate, asset.realizedFees);
```

topic0 `0xa1facf110ded5028ee267fa3d5986f2aa4dc14230b79ffd27e95760f14883350`, `assetId` indexed.

Why the emitted value is the *settled* index rather than a stale one:
[`AssetLogic.accrue:141-150`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/libraries/AssetLogic.sol#L141-L150) writes `asset.drawnIndex = getDrawnIndex()` and sets
`lastUpdateTimestamp = block.timestamp` *before* `updateDrawnRate` reads it — see the
[`:131`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/libraries/AssetLogic.sol#L131) docstring, "Uses last stored index; asset accrual should have already occurred."
That ordering is what makes `(drawnIndex, drawnRate, block.timestamp)` from the log a
self-consistent checkpoint, and it is what the empirical check below confirms.

Checked against live Hub state for 8 assets: the last `UpdateAsset` log's `drawnIndex` and
`drawnRate` **exactly equal** the Hub's stored values, and the log's block timestamp **equals**
`lastUpdateTimestamp`. **[verified]**

So the index is handed to us as an authoritative checkpoint — we never derive it. Two
consequences:

- **We do not model the interest-rate strategy at all.** `drawnRate` arrives in the event;
  we only apply linear interest for the tail since the last checkpoint. This removes the
  single largest source of "reimplement the protocol and get it subtly wrong" risk.
- **Valuation works at any historical `t`**, not just `latest`, with no archive node — because
  a checkpoint stream plus linear interpolation reconstructs the index at any point in time.

Checkpoint density on the Core Hub: **434 `UpdateAsset` events per 10k blocks** (half of all
Hub logs), spread across 15 active assets. Dense enough that interpolation gaps stay short.
**[verified]**

### 5.4 Numerical validation

Implemented both formulas in JS and compared against the contracts for 20 real
(reserve, user) pairs sampled from recent `Supply` / `Borrow` logs:

| side | method compared against | result |
|---|---|---|
| debt | `getUserDebt(reserveId, user)` | **10 / 10 exact** |
| supply | `getUserSuppliedAssets(reserveId, user)` | **10 / 10 exact** |

Zero wei of drift on either side. Balances spanned 0 to 243,354 tokens across WETH, AAVE,
USDC, USDG and cbBTC, including dust (`0.00000011` cbBTC) and zero balances.

Coverage note: `premiumShares`, `premiumOffsetRay`, `deficitRay` and `swept` are zero on **all
34 assets across all four hubs** (Core 17, Plus 7, Prime 7, Global Dollar 3), so mainnet cannot
exercise the premium or deficit branches at any sampling density. **[verified]**

Those branches were instead validated on a **local Anvil deployment** of v4 (§8), driving the
states mainnet never reaches: **[verified]**

| scenario | forced state | result |
|---|---|---|
| risk premium | `premiumShares = 10e18` | `suppliedAssets`, `drawnDebt`, `premiumDebt` all exact |
| bad debt | `deficitRay = 2.95e45` | all three exact |

6/6 exact against `getUserSuppliedAssets` / `getUserDebt`. Combined with the 20/20 on mainnet,
every branch of the §5.1–5.2 formulas is now covered.

Two limits on how far that evidence reaches. Scenario tests prove the formulas at specific
points, not across the input domain — **property-based fuzzing against `AssetLogic` would be
the stronger follow-up**, exercising the overflow and rounding edges no hand-built scenario
thinks to construct. And these branches are validated against a *local* deployment; when
mainnet premium or deficit first becomes nonzero, the reconciliation job (§9) is what confirms
the finding still holds in production. That transition has a known signature — see §9.1.

### 5.5 Resulting design

**Fold logs → shares as the ledger; compute assets on read via the formulas above.**

- Source of truth: `suppliedShares` / `drawnShares` per `(spoke, reserveId, user)`, plus a
  per-`(hub, assetId)` mirror of Hub asset state.
- The API needs **no RPC on the read path** — valuation is arithmetic over indexed state.
- `eth_call` is demoted from *required* to a **reconciliation job** (§9): periodically compare
  computed values against `getUserDebt` / `getUserSuppliedAssets` and alert on drift. That is
  a much better use of it, and it turns the wei-exact match above into a standing invariant
  rather than a one-off check.

The cost: the Hub asset mirror must be folded from the Hub's own events (`Add`, `Remove`,
`Draw`, `Restore`, `MintFeeShares`, `Sweep`, `Reclaim`, `ReportDeficit`, `EliminateDeficit`,
`UpdateAsset`) to keep `liquidity`, `addedShares`, `drawnShares`, `swept`, `realizedFees`,
`deficitRay` and the premium fields current. **This is the highest-risk part of the ingestion
logic** — one mis-folded transition silently corrupts every supply valuation for that asset. It
is also exactly what the reconciliation job (§9) is there to catch.

The transitions, read from `Hub.sol`: **[verified]**

| event | effect on `Asset` |
|---|---|
| `Add` | `addedShares += shares`, `liquidity += amount` |
| `Remove` | `addedShares -= shares`, `liquidity -= amount` |
| `Draw` | `drawnShares += drawnShares`, `liquidity -= drawnAmount` |
| `Restore` | `drawnShares -= drawnShares`, `liquidity += drawnAmount + premiumAmount` |
| `MintFeeShares` | `addedShares += shares` |
| `Sweep` / `Reclaim` | `liquidity ∓ amount`, `swept ± amount` |
| `UpdateAsset` | sets `drawnIndex`, `drawnRate`, `realizedFees`; `lastUpdateTimestamp` = block ts |
| `TransferShares` | no asset-level change (§4.4) |

**The fold reproduces chain state exactly.** Seeding from `getAsset` at one block, replaying 18
Hub events, and comparing against `getAsset` 95 blocks later: **4 assets × 7 fields, all
exact.** **[verified]**

That mainnet window is 95 blocks — bounded by non-archive state access, not by the method — and
covers only `Add` / `Remove` / `Draw` / `Restore` / `UpdateAsset`. Those are the only
transitions that occur in practice: a 10k-block sample contains 906 Hub events and **every one**
is one of those five.

The remaining three were exercised on Anvil (§8), where they can be forced. Replaying
`MintFeeShares`, `Sweep`, `Reclaim` — and the premium/deficit-bearing `Restore` and
`ReportDeficit` — reproduced all **10 mirror fields** exactly in every scenario. **[verified]**
The transition table above is therefore validated end to end, not just on its hot paths.

Staged fallback if the Hub mirror proves troublesome: ship debt valuation first (§5.1 needs
only `UpdateAsset` + user shares — far less state), and use `eth_call` at `latest` for supply
until the mirror is trusted.

---

## 6. Read functions — reconciliation and enrichment

All on the Spoke unless noted; all confirmed callable at `latest`. **[verified]**

| purpose | function | returns |
|---|---|---|
| user account | [`getUserAccountData`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L745) | risk premium, avg collateral factor, **health factor**, total collateral value, total debt value, active-collateral and borrow counts |
| user position | [`getUserPosition`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L736-L739) | raw `UserPosition` struct — the folded ledger compares against this |
| user supply | [`getUserSuppliedShares`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L705) · [`getUserSuppliedAssets`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L698) | shares, and shares converted to assets |
| user debt | [`getUserDebt`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L714) · [`getUserTotalDebt`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L722) · [`getUserPremiumDebtRay`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L729) | drawn and premium components, and their sum |
| user flags | [`getUserReserveStatus`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L691) | `(isUsingAsCollateral, isBorrowing)` for one reserve |
| reserve registry | [`getReserveCount`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L626) · [`getReserve`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L665) · [`getReserveConfig`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L671) · [`getDynamicReserveConfig`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L679-L682) | the reserve table and its versioned risk config |
| reserve totals | [`getReserveSuppliedAssets`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L631) · [`getReserveTotalDebt`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L652) | per-reserve aggregates |
| liquidation | [`getLiquidationConfig`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L622) · [`getLiquidationBonus`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/interfaces/ISpoke.sol#L758-L762) | spoke-wide thresholds (§12.4) and the bonus at a given health factor |

Given §5, these are **not on the read path** — they are the reconciliation oracle (§9). The
indexer computes values arithmetically and uses these calls to prove it stayed correct.

`getUserAccountData` remains the highest-value single call: **health factor** in WAD plus
aggregate collateral/debt value, computed by the protocol itself. Sampled on real borrowers:

| user | HF | activeCollateral | borrows |
|---|---|---|---|
| `0xd2b70EfbF41cF73ABf59adBa08Afaa6d114B56C8` | 1.168 | 3 | 1 |
| `0x29f87413ccDE6e872853a4f7b5D43Dd31d44198c` | 1.272 | 1 | 1 |
| `0xd7AD196009fBe5c4210DB626719AF5439D43e5B9` | 1.962 | 1 | 1 |
| `0x03BD789D919e47D7759E9Cbb5f8A565bc293FcD3` | 2.586 | 1 | 1 |

`totalCollateralValue` and `totalDebtValueRay` are in "units of Value", the latter RAY-scaled.
**`1e26` = 1 USD** — settled in §7.1 from `SpokeUtils.toValue`. **[verified]**

The whole of `getUserAccountData` is reproducible off-chain — see §7.

---

## 7. Health factor — reproducible off-chain, prices included

Short answer: **yes — both the HF arithmetic and every price it depends on are reproducible
from logs.** The computation matches the contract exactly (8/8, §7.2), and all 14 price feeds
are event-derivable (§7.4). No `eth_call` is required to serve health factor.

### 7.1 The formula

From [`Spoke._processUserAccountData:706-790`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/Spoke.sol#L706-L790), iterating only the reserves flagged in the user's `PositionStatus`:

```
value(amount, dec, price) = amount * price * 10^(18 - dec)        // SpokeUtils.toValue

# per collateral reserve, only if collateralFactor > 0 and suppliedShares > 0
collValue_i        = value(previewRemoveByShares(suppliedShares_i), dec_i, price_i)
totalCollateral   += collValue_i
weightedColl      += collateralFactor_i * collValue_i             # Value × BPS

# per borrowed reserve
debtRay_j          = drawnShares_j * drawnIndex + premiumDebtRay_j
totalDebtValueRay += value(debtRay_j, dec_j, price_j)

healthFactor = floor( bpsToWad(weightedColl) * RAY / totalDebtValueRay )
             = type(uint256).max   if totalDebtValueRay == 0
```

`bpsToWad(a) = a * (WAD / 1e4) = a * 1e14`
([`WadRayMath.sol:177-185`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/libraries/math/WadRayMath.sol#L177-L185)).
HF is WAD-scaled: `1e18` = 1.00.

**This resolves the `Value` unit question.**
[`SpokeUtils.toValue:28-40`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/libraries/SpokeUtils.sol#L28-L40) documents it outright: **`1e26` represents 1 USD** — an 18-decimal-normalised amount
times an 8-decimal price ([`ORACLE_DECIMALS = 8`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/libraries/SpokeUtils.sol#L13)). So
`totalCollateralValue / 1e26` is USD, and `totalDebtValueRay / 1e26 / 1e27` is USD.

Note HF uses `collateralFactor`, which is **per-user-config-version** — `_dynamicConfig[reserveId][userPosition.dynamicConfigKey]`, not the reserve's current key. This is the §3 versioning
requirement showing up where it bites.

### 7.2 Validated exactly

Computed HF off-chain from shares + Hub state + per-user collateral factors + oracle prices,
compared against `getUserAccountData` for 8 real borrowers, **all calls pinned to block
25652782**:

| field | result |
|---|---|
| `healthFactor` | **8 / 8 exact** |
| `totalCollateralValue` | **8 / 8 exact** |
| `totalDebtValueRay` | **8 / 8 exact** |
| `activeCollateralCount` / `borrowCount` | **8 / 8 exact** |

HF values spanned 1.167 → 4883.2, plus a no-debt user returning `type(uint256).max`.

**Every read in a comparison must be pinned to one `blockNumber`.** `drawnIndex` accrues per
second, so calls landing on different blocks produce a characteristic false signal: HF agrees
to ~6 dp while `totalCollateralValue` and `totalDebtValueRay` disagree, because the ratio
partly cancels the skew. HF is a *per-block* quantity. Reconciliation must pin the block across
all reads, and API responses should state the block they were computed at.

### 7.3 Inputs — where each comes from

| input | source | from events? |
|---|---|---|
| `suppliedShares`, `drawnShares`, `premiumShares`, `premiumOffsetRay` | Spoke position events | yes (§4.1) |
| `drawnIndex` | Hub `UpdateAsset` + linear interpolation | yes (§5.3) |
| Hub asset state for the supply ratio | Hub event fold | yes (§5.5) |
| `collateralFactor` at the user's config version | `AddDynamicReserveConfig` / `UpdateDynamicReserveConfig` + user's `dynamicConfigKey` | yes (§4.2, §4.3) |
| collateral / borrowing flags | `SetUsingAsCollateral` + borrow/repay transitions | yes (§4.1) |
| `decimals` | `AddReserve` / reserve registry | yes (§4.3) |
| **price per reserve** | Chainlink `AnswerUpdated` + adapter arithmetic (§7.4) | yes — all 14 feeds |

So HF costs us one additional log source — the price feeds — and no calls.

### 7.4 The price layer

`AaveOracle` is **per-Spoke** (it indexes by `reserveId`) and is a thin wrapper: it just calls
`latestAnswer()` on a per-reserve feed and reverts on zero
([`AaveOracle.sol:79-88`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/AaveOracle.sol#L79-L88)).
Main Spoke oracle: `0x99B2B6CEa9C3D2fd8F4d90f86741C44B212a6127`, 8 decimals.

The feeds are *not* uniform. Probing all 14 **[verified]**:

| class | reserves | reconstructable from logs? |
|---|---|---|
| Plain Chainlink proxy (`aggregator()`) | WETH, cbBTC, AAVE, LINK | **yes** — `AnswerUpdated` |
| Capped Chainlink (`ASSET_TO_USD_AGGREGATOR` + static `getPriceCap` = 1.04) | USDC, USDT, RLUSD, USDG, frxUSD | **yes** — feed + constant |
| Two Chainlink feeds composed | WBTC (`ASSET_TO_PEG` × `PEG_TO_BASE`), EURC (`ASSET_TO_USD` × `BASE_TO_USD`) | **yes** — two feeds |
| Fixed price (168-byte contract) | GHO | **yes** — constant |
| Chainlink × on-chain LST ratio | wstETH (`RATIO_PROVIDER` = stETH `0xae7a…`), weETH (`RATIO_PROVIDER` = weETH `0xCd5f…`) | **yes, at rebase granularity** — see 7.4.2 |

[`AnswerUpdated(int256,uint256,uint256)`](https://etherscan.io/address/0x7c7FdFCa295a787ded12Bb5c1A49A8D2cC20E3F8#code) fires **36× per 10k blocks** on the ETH/USD aggregator
`0x7c7FdFCa295a787ded12Bb5c1A49A8D2cC20E3F8`, so the Chainlink half is a normal indexing job.
**[verified]**

#### 7.4.1 Aggregator rotation is a non-issue in our window

`AnswerUpdated` is emitted by the *aggregator*, not by the proxy the oracle points at, and
aggregators rotate across Chainlink "phases" — so in principle a historical backfill must
follow that migration rather than watching one fixed address.

In practice every feed our reserves depend on is on **phase 2**, and in each case that
phase-2 aggregator was **already live at our genesis block** (`AnswerUpdated` present
in a 10k window spanning block 24,720,899). **[verified]**

| feed | proxy | phase | aggregator live at genesis |
|---|---|---|---|
| ETH/USD | `0x5424…5215e` | 2 | yes (78 updates in window) |
| BTC/USD | `0xb41E…C8B0A` | 2 | yes (57) |
| AAVE/USD | `0xF02C…e4a85` | 2 | yes (44) |
| LINK/USD | `0xC7e9…28C183` | 2 | yes (45) |
| USDC/USD | `0xEa67…b496b` | 2 | yes (2) |
| WBTC/BTC | `0xfdFD…FBB23` | 2 | yes (1) |

So **zero rotations occurred over the entire Aave v4 history**. Resolve proxy → aggregator once
per feed and index that address.

Detecting a future rotation is free: Chainlink packs the phase into the **high 64 bits of
`roundId`**, so `phaseId = roundId >> 64`. On the ETH/USD proxy, `latestRound` =
`36893488147419125619` → phase `2`, aggregator round `22387`. **[verified]** Watch that value;
if it increments, re-resolve `aggregator()` and continue the series against the new address.
No migration machinery needed up front — just an assertion that would fire if the assumption
ever broke.

#### 7.4.2 LST ratios are a daily step function, not continuous drift

Two reserves are **liquid staking tokens** — wstETH and weETH. These represent staked ETH plus
accrued rewards, in *wrapped* form: the holder's balance is fixed and each token grows in value,
rather than the balance rebasing upward. So they are not 1:1 with ETH — wstETH is currently
`1.2406` stETH — and pricing them needs an exchange rate on top of ETH/USD. That rate lives in
a token contract rather than in a Chainlink feed, which is what makes these two the awkward
case.

The rates do not drift continuously, though, and treating them as if they did would rule out
indexing them unnecessarily.

The adapter arithmetic is simple and exact — verified at the wei level: **[verified]**

```
wstETH/USD = ETH/USD × stEthPerToken() / 1e18
# 187522000000 × 1240624545593819793 / 1e18 = 232644396038  == latestAnswer()  (exact)
```

And `stEthPerToken()` is not a continuous function. It moves on Lido's oracle report, which
emits [`TokenRebased`](https://etherscan.io/address/0xae7ab96520DE3A18E5e111B5EaAb095312D7fE84#code) — measured at **~1.2 per day** (5 events in 30k blocks). Between reports,
submits and withdrawals mint or burn shares *proportionally*, leaving the ratio intact.

Better still, `TokenRebased` carries `postTotalShares` and `postTotalEther`, so the exact
post-rebase ratio is **in the event payload** — no call needed to learn it.

The payload is sufficient on its own. Reconstructing the ratio as
`postTotalEther × 1e18 / postTotalShares` from the most recent `TokenRebased` reproduces
`stEthPerToken()` **exactly**, 189 blocks (~0.6h) later, despite ordinary Lido deposit and
withdrawal activity in between — submits and withdrawals move `totalPooledEther` and
`totalShares` proportionally, leaving the quotient intact. Sampling the ratio at six blocks
across ~100 blocks likewise returns a single distinct value. **[verified]**

So the LST feeds are a normal indexing job: ~1.2 events/day/asset, exact at each checkpoint,
constant in between, and never needing a call.

#### 7.4.3 Cap adapters apply a static cap

These are the fixed-cap variant, not the stateful growth-rate kind. Verified by computing
`min(rawChainlinkPrice, getPriceCap())` and comparing against the adapter's own
`latestAnswer()`: **[verified]**

| feed | raw | cap | adapter | `min(raw, cap)` |
|---|---|---|---|---|
| USDC | 99971505 | 104000000 | 99971505 | match |
| USDT | 99899080 | 104000000 | 99899080 | match |
| USDG | 99994851 | 104000000 | 99994851 | match |
| frxUSD | 100014824 | 104000000 | 100014824 | match |
| RLUSD | 99998759 | 104000000 | 99998759 | match |

5/5 exact. The cap is a constant 1.04 and `isCapped = false` throughout — none is currently
binding. So these feeds are `chainlinkPrice`, clamped by a constant we can hard-code: fully
reconstructable from logs.

One residual: because no feed is currently at its cap, the clamp branch itself is untested
against real data. The arithmetic is a `min` against a constant, so the risk is low, but a
depeg is the moment it would first matter.

#### 7.4.4 Cost of indexing the price layer

~8 distinct Chainlink aggregators at roughly 36 updates per 10k blocks each, over 931,565
blocks of history: **order 25–30k logs**, comparable to the Main Spoke's 38,580. Plus ~2
LST rebase events per day. This is the same machinery as everything else in the pipeline, at
the same order of magnitude. There is no scaling reason to defer it.

### 7.5 Recommendation

**Index the price layer from the start.** Nothing about it justifies deferral:

- rotation: zero occurrences across all of v4's history, so no migration logic is needed to ship
- LST ratios: ~1.2 events/day with the exact ratio in the event payload, not continuous drift
- volume: ~25–30k logs, the same order as the Spoke stream already ingested

Deferring it buys nothing and leaves an `eth_call` on the read path — the one thing §5 exists to
eliminate. Price feeds are simply another log source, subscribed the same way, folded into the
same store.

That makes **all 14 reserves** event-derivable. The one untested edge is the cap clamp itself
(§7.4.3): no feed is currently at its cap, so the `min` branch has never fired against real
data.

Three properties worth stating in the API contract:

- HF is **derived and block-stamped**, not stored. Return the block it was computed at.
- Price and position data have **independent freshness**. A price feed with no update for an
  hour is normal Chainlink behaviour, not staleness — but HF computed from it should carry the
  price timestamp so consumers can judge for themselves.
- **Every `uint256` must be serialised as a JSON string, never a bare number.** IEEE-754 has
  53 bits of mantissa, so any value above ~9.007e15 is silently rounded on parse — and share
  balances, RAY-scaled debt and `Value` amounts are all far above that. This bit the validation
  harness: an unquoted `301369863013698631` came back as `...624`, a 7-wei error that looked
  exactly like a rounding bug in the formula until the JSON was inspected. Silent, plausible,
  and wrong in the last few digits — the worst failure shape for a financial API.

`getUserAccountData` stays the reconciliation oracle (§9), never the serving path.

---

## 8. Ingestion constraints

### Archive is not required — as long as we never read historical *state*

Worth separating clearly, because public RPCs blur it commercially:

- **historical state** (`eth_call`, `eth_getCode`, `eth_getStorageAt` at an old block) →
  needs an archive node. **Out of scope per the task brief, and our design avoids it entirely.**
- **historical logs** (`eth_getLogs` over old ranges) → retained by any full node; not an
  archive feature.

The design in §5 satisfies this and then some: because the Hub emits its own index
(§5.3), **the entire pipeline — backfill, valuation, and historical queries — reads logs
only.** No archive node, and no `eth_call` on the critical path at all. `eth_call` is used
by the reconciliation job (§9) and by token-metadata enrichment (§12.5), both only at `latest` and
neither while serving a request. This is a deliberate constraint to
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

Three log sources, over genesis `24,720,899` → `25,652,464` (931,565 blocks):

| source | volume over full history | basis |
|---|---|---|
| Main Spoke | **38,580 logs** in 101 requests | measured end-to-end **[verified]** |
| Core Hub | ~81,000 logs | extrapolated from 868/10k blocks **[verified]** |
| Price feeds (~8 aggregators + 2 LST) | ~25–30,000 logs | extrapolated from 36/10k per feed **[verified]** |
| **total** | **~145,000 logs** | ~300 chunked requests |

Full backfill is minutes, not hours, and fits comfortably in a free tier. No need for a
bulk-data provider or a parallel-partition backfiller in iteration 1 — a sequential chunked
backfill is genuinely sufficient, even with the price layer included.

Note the three streams are **independently chunkable and have no ordering dependency between
them** during backfill: positions need Hub state only at *query* time, not at ingest time. They
can be fetched in parallel and joined afterwards, which is the easy win if backfill ever does
get slow.

### Reorgs

Nothing v4-specific, but the brief calls it out. Store `block_number` + `block_hash` per event,
keep the last N blocks (~64–128) "unfinalized", and on a parent-hash mismatch roll back and
re-scan. Because positions are a **fold over events**, rollback means deleting events above the
fork point and replaying — which argues for keeping the raw event log as an immutable table and
treating position balances as a derived projection.

### Local development

`scripts/deploy/examples/AaveV4DeployAnvil.s.sol` in `aave/aave-v4` deploys the **whole
protocol to a local Anvil** (chainId 31337, 2 hubs, 3 spokes) with documented steps.
**[verified — executed]** Real contracts, real events, deterministic, no rate limits, and Anvil
can force reorgs to test the rollback path. It is also the only way to reach premium, deficit
and the cold Hub transitions (§5.4, §5.5).

Two practical notes for standing it up:

- The deploy script leaves **bare hubs and spokes — zero assets, zero reserves**. Driving
  scenarios means configuring assets first; the repo's own `tests/setup/Base.t.sol` fixture does
  this and is the faster starting point than the deploy script for validation work.
- Tests build `SpokeInstance` from raw artifact bytecode via `vm.getCode`, which **leaves the
  `LiquidationLogic` library placeholder unlinked**, so any liquidation reverts with
  *"delegatecall to non-contract address"*. This affects the repo's own liquidation suite in a
  clean checkout. Fix: deploy the library and `vm.etch` it at `ISpoke.getLiquidationLogic()`.

---

## 9. Reconciliation

§5 makes the indexer compute values arithmetically instead of asking the chain. That is what
removes RPC from the read path — but it means **a bug in the fold produces wrong numbers, not
errors**. The Hub asset mirror in particular is folded from nine event types; one mis-handled
transition silently corrupts every supply valuation for that asset, indefinitely, with nothing
to flag it.

The contracts can compute the same values we do. Reconciliation is the standing comparison of
the two, and it is what turns the one-off exactness results in §5.4 and §7.2 into a continuous
invariant.

### 9.1 What it compares

Three tiers, cheapest first. **Which tier drifts is the diagnosis**, so they are worth keeping
distinct rather than collapsing into one "is the data right" check.

| tier | compares | a failure here means |
|---|---|---|
| 1. shares | folded balances vs `getUserSuppliedShares`, `getUserPosition` | ingestion is broken — missed log, bad decode, un-rolled-back reorg |
| 2. Hub mirror | mirrored asset state vs `getAsset` | a mis-folded Hub transition — the highest-risk component (§5.5) |
| 3. derived | computed assets / HF vs `getUserDebt`, `getUserSuppliedAssets`, `getUserAccountData` | if tiers 1–2 are clean, **the formula is wrong, not the data** |

Tier 2 is the best value per unit cost: ~17 `getAsset` calls covers the entire Core Hub, and it
catches corruption *at the source*, before it reaches user-facing numbers.

Tier 3 drifting while 1 and 2 are clean is a specific, predictable signature. Mainnet premium
and deficit are zero today (§5.4); the day an asset acquires either, tier 3 moves alone. Worth
encoding as a named alert rather than rediscovering at 3am.

### 9.2 Tolerance is zero

Not approximate agreement. Rounding is fully specified — `rayMulUp`, `Math.Rounding.Floor`,
`percentMulDown` — and the implementation is wei-exact against the contracts on both mainnet
(20/20, 8/8) and Anvil (6/6). **Any nonzero drift is a bug, not noise.** That removes the usual
argument about whether a threshold is too tight.

With zero tolerance a systemic fault will fire thousands of alerts, so aggregate by
`(tier, asset, field)` before paging.

### 9.3 When it runs

**Anchor on `finalized`, never on the tip.** Comparing against `latest` produces false
mismatches whenever a block reorgs out mid-check — intermittent and far harder to diagnose than
the block-skew trap in §7.2. `finalized` is immutable by definition, so a mismatch there is
always real.

Measured: `finalized` trails by **75 blocks (~15 min)** and `safe` by **43 (~8.6 min)**, and
**state is readable at both** — inside the ~128-block window non-archive providers serve.
**[verified]** So the reorg-immune choice costs nothing in infrastructure.

| trigger | scope | rationale |
|---|---|---|
| backfill completes | full — all assets, all users | gates serving; don't expose computed values until the fold is proven once |
| `finalized` advances (poll ~5 min) | Hub assets **touched since last run** | the high-risk mirror, checked at the source |
| every ~100 blocks | 50–100 sampled users | fold bugs that don't surface at asset level |
| continuous, paged | all users, one full pass / 24h | cold accounts sampling never visits |
| after a reorg rollback | assets + users in the replayed range | highest-probability moment for the fold to be wrong |
| after a deploy touching fold logic | full | prior confidence is void |
| on demand | anything | debugging and CI |

**Reconcile what moved, not everything.** An asset whose state did not change cannot have
drifted, and the Hub event stream already tells us which `assetId`s saw activity. On measured
volumes this matters: 434 `UpdateAsset` events per 10k blocks spread across 15 active assets
means only a handful move in any 5-minute window. Blind-polling all 17 every run is mostly
waste — and worse, it makes the check feel expensive enough that someone eventually reduces its
frequency.

Sampling for tiers 1 and 3 should weight toward recently-touched positions (most likely to
expose a bad transition), the largest positions (blast radius), and users near HF 1.0 (highest
consequence), plus a random tail.

Cost is negligible: the asset tier is 3–4k calls/day at a 5-minute poll; a full user sweep is
roughly `3 × user_count`, trivially absorbed across 24h.

### 9.4 Handling drift

The response depends entirely on which tier moved.

| drifted | recoverable? | action |
|---|---|---|
| Hub mirror only | yes — it is a cache of readable state | auto-heal from `getAsset`, alert loudly |
| user shares | yes, but healing destroys the evidence | snapshot, alert, targeted replay — **never silent overwrite** |
| derived, tiers 1–2 clean | no — resyncing fixes nothing | code bug; halt serving that scope |

**Repair is replay, not patch.** Because raw events are immutable and positions are a derived
projection, repair means rewinding the affected asset or user to the last verified block and
re-folding. That also bisects for free: if replay from known-good yields the correct value, the
stored projection was corrupted; if replay reproduces the drift, the fold logic itself is wrong.
Manual patching of balances should never be a runbook step — it hides the bug and breaks the
"positions are a pure function of the log" invariant everything else rests on.

**Keep ingesting; degrade only the projection.** Raw ingestion is append-only and always safe,
and halting it just means falling further behind while debugging. Serving degrades instead, and
scoped: drift on `assetId 5` quarantines reserves backed by asset 5, not the whole API.
Affected responses carry `verified: false` and `lastVerifiedBlock` rather than a 503 — though
past some magnitude, withholding a number beats confidently serving a wrong one.

**Diff per field.** Each mirror field maps almost 1:1 to the event type that maintains it, so a
field-level diff names the culprit before anyone opens a debugger:

`addedShares` → `Add`/`Remove`/`MintFeeShares` · `drawnShares` → `Draw`/`Restore` ·
`swept` → `Sweep`/`Reclaim` · `deficitRay` → `ReportDeficit`/`EliminateDeficit` ·
`realizedFees` → `UpdateAsset`

**Alert on heal frequency, not just on drift.** The failure mode to design against is auto-heal
running every five minutes, silently papering over a persistent fold bug, with every individual
check passing *after* the heal. A mirror that needs repeated healing is broken even though it is
never observed in a bad state.

Escalation: single asset healing once → log and metric. Recurring on the same asset → page and
quarantine it. Tier-1 drift or multiple assets → page, and **fall back to `eth_call`
passthrough** for reads. Naming that fallback matters: the RPC dependency §5 deliberately
designed away becomes the emergency path — slower, but correct by construction. Having a
known-correct degraded mode is what makes the aggressive compute-everything-off-chain choice
defensible.

### 9.5 What it does not prove

Reconciliation checks correctness **at `finalized`, not throughout history**. A compensating
pair of errors could agree now while past values were wrong. Providers serve state for roughly
the last ~128 blocks, so a short historical window is available; anything deeper needs archive,
which §8 deliberately designs away from.

Verified state therefore trails served state by ~15 minutes. That is the right trade, but it
should be visible rather than implied — expose `lastVerifiedBlock` and a drift counter on
`/health` so the gap is legible.

---

## 10. Live state — Main Spoke, block 25652535 **[verified]**

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

## 11. Enrichment candidates

The brief needs ≥1 additional source, exposed alongside indexed data.

USD values already come from the protocol's own price feeds (§7.4), so an off-chain price API
adds nothing as a currency converter. Its value here is comparison, not conversion:

- **Independent market price — DefiLlama** (`coins.llama.fi`): keyless, batch, returns
  `{price, decimals, symbol, confidence, timestamp}` per `ethereum:0x…`. Tested working.
  **[verified]** Exposed *alongside* the oracle price, it surfaces **oracle deviation** —
  how far the protocol's view of an asset has drifted from the market's.

  That is a genuinely useful signal rather than a decorative one: deviation is what makes
  positions liquidatable, and it is exactly what the capped feeds (§7.4.3) exist to bound. A
  position at HF 1.05 priced by an oracle that is 2% away from market is a materially
  different risk from the same HF with the feeds in agreement.

  It also exercises everything the brief is probing for — separate source, separate cadence,
  separate failure mode, needs caching and staleness handling — while being an actual product
  feature rather than a bolt-on.
  - CoinGecko's free tier now allows **1 contract address per request** — unusable for batch
    pricing without a paid key. **[verified]** DefiLlama is the better default.
- ENS / address labels — nice-to-have, low value here.

`getUserAccountData` is deliberately *not* listed. It is the reconciliation oracle (§7.2), not
an enrichment source — counting it as "additional data" would be counting the protocol as its
own second source.

Recommendation: **DefiLlama as the headline enrichment, framed as oracle-vs-market deviation**
rather than USD conversion.

---

## 12. Conclusion — serving a portfolio view

Target output: a list of positions, each with a type, a token, an amount and a USD value; plus
portfolio-level health and net worth.

Everything below is derivable from indexed logs and arithmetic. **No `eth_call` on the read
path**, at any timestamp, without an archive node.

### 12.1 Position identity and type

A position is keyed `(chain, spoke, reserveId, user)` and exists while `suppliedShares > 0` or
`drawnShares > 0`. Keying on `user`, never `caller` (§2).

| type | condition |
|---|---|
| `borrow` | `drawnShares > 0` |
| `collateral` | `suppliedShares > 0`, flagged as collateral, **and** `collateralFactor > 0` |
| `supply` | `suppliedShares > 0` otherwise |

The third condition is not pedantry. Five of the Main Spoke's fourteen reserves have
`collateralFactor = 0` (§10) — a user can flag them as collateral and they still contribute
nothing to borrowing power. Reporting them as collateral would overstate the user's position.
The collateral factor is the user's pinned `dynamicConfigKey` version, not the reserve's
current one (§3).

### 12.2 Where each output field comes from

| field | source | from logs? |
|---|---|---|
| position list | fold `Supply`/`Withdraw`/`Borrow`/`Repay`/`LiquidationCall`/`ReportDeficit` (§4.1) | yes |
| position type | `SetUsingAsCollateral` + borrow state + `collateralFactor` (§4.1–4.3) | yes |
| token address | Hub `AddAsset(assetId, underlying, decimals)` | yes |
| decimals | same event — 17 observed, one per Core Hub asset **[verified]** | yes |
| name, symbol | ERC-20 `name()` / `symbol()` — in no Aave event | **no — §12.4** |
| amount, supply side | virtual-share formula (§5.2) | yes |
| amount, debt side | index formula + premium (§5.1) | yes |
| USD price | Chainlink `AnswerUpdated` + adapter arithmetic (§7.4), 8 dp | yes |
| USD value | `amount × price`, where `1e26` = 1 USD (§7.1) | yes |
| health factor | §7.1 formula, validated 8/8 exact (§7.2) | yes |
| portfolio totals | summed from the above | yes |

### 12.3 Portfolio aggregates

Two conclusions here matter more than the arithmetic.

**Health factor is per-Spoke, never per-user.** Spokes are isolated: each carries its own
liquidation config, collateral factors and oracle (§1, §7.4). A user with positions on the Main
and Bluechip spokes has **two independent health factors**, and can be liquidated on one while
comfortably healthy on the other. A portfolio view must report HF per spoke. Blending them into
a single number is not a simplification — it is wrong in the one direction that matters, hiding
an imminent liquidation behind unrelated collateral.

This is structural, not a policy choice. `liquidationCall(collateralReserveId, debtReserveId,
user, …)` resolves both ids against `_reserves` — *that* Spoke's reserve map — and uses that
Spoke's `_liquidationConfig` and `ORACLE`; `_calculateUserAccountData` reads only that Spoke's
`_positionStatus` and `_userPositions`. Pairing collateral in one spoke against debt in another
cannot be expressed. **[verified]** For a borrower, two spokes are two independent margin
accounts with non-fungible collateral.

Losses are absorbed explicitly rather than socialised. Bad debt is recorded per
`(assetId, spoke)` as `coveredSpoke.deficitRay`, and
[`Hub.eliminateDeficit`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/Hub.sol#L333-L359)
clears it by burning the covering spoke's **own** `addedShares` — both `addedShares` and
`deficitRay` fall together, so the share price is preserved and other suppliers take no haircut.
It is role-gated; the treasury spoke, which also receives liquidation fees (§4.1), is the
natural caller. **[verified]**

Risk does still cross spokes at the **liquidity** layer, just not the liquidation one. Until a
deficit is eliminated it remains inside `aggregatedOwedRay` (§5.2), so `totalAddedAssets` counts
bad debt as if still owed, and suppliers of that asset — through *any* spoke — hold shares
partly backed by it. Per-spoke credit lines (§1) are what bound that exposure.

**Net worth is not `totalCollateralValue`.** That field counts only reserves that are flagged
collateral *and* have `collateralFactor > 0`. A portfolio balance counts everything supplied.
With five of fourteen reserves at `CF = 0`, the two diverge materially — do not reuse one for
the other.

```
supplied(USD) = Σ over supply+collateral positions of value(amount, decimals, price) / 1e26
debt(USD)     = Σ over borrow positions      of value(amount, decimals, price) / 1e26
netWorth      = supplied − debt
healthFactor  = per spoke, §7.1
```

### 12.4 Distance to liquidation

`healthFactor` is WAD-scaled (`1e18` = 1.00) and `type(uint256).max` with no debt. Serving it
needs no call — §7.2 reproduces it exactly. It is per-spoke (§12.3), so distance-to-liquidation
is per-spoke too.

Risk is banded, not binary. Live Main Spoke config plus the hardcoded threshold: **[verified]**

| value | source | meaning |
|---|---|---|
| `1.00` | `HEALTH_FACTOR_LIQUIDATION_THRESHOLD = 1e18` — a **constant**, not configurable | below this, liquidatable |
| `1.24` | `targetHealthFactor` | liquidation restores the position to *here*, not to 1.0 |
| `0.90` | `healthFactorForMaxBonus` | at or below, the liquidator's bonus is maxed |
| `9000 bps` | `liquidationBonusFactor` | minimum bonus is 90% of maximum |
| `105.55%` / `105.00%` | `maxLiquidationBonus` per reserve | WETH, WBTC / USDC, USDT |
| `$1,000` | `DUST_LIQUIDATION_THRESHOLD = 1000e26` (§7.1 units) | a liquidation may not leave less behind |

The bonus interpolates linearly between the bands — for WETH, 4.99% at HF 1.00 rising to 5.55%
at HF ≤ 0.90. A position between 1.00 and 1.24 is not at risk, but sits below where a
liquidation would have left it.

**Report headroom, not just the ratio.** HF is linear in collateral value, so it reaches 1.0
after a collateral drawdown of `1 − 1/HF`:

| HF | collateral | debt | drawdown to HF 1.0 |
|---|---|---|---|
| 1.1346 | $51,946 | $38,001 | **11.9%** |
| 1.2268 | $76,959 | $49,413 | 18.5% |
| 2.6118 | $48,495 | $15,112 | 61.7% |

Exact for single-asset collateral; a first-order approximation for mixed baskets, and it
ignores the debt asset appreciating — which hurts identically if the user borrowed a volatile
asset against stables.

**A position can be liquidated with no price movement.** `drawnIndex` accrues every second
(§5.1) and risk premium accrues on top, so HF drifts downward in a completely flat market.
Recomputing only on `AnswerUpdated` would systematically miss slow-bleed positions — the ones
nobody is watching. Because `drawnRate` is indexed (§5.3), the interest-only path is
deterministic: solving for HF = 1.0 with prices held constant yields a *time* to liquidation,
which is a strictly better alert than a price trigger.

### 12.5 Out of scope

- **Token name and symbol.** Not present in any Aave event. A one-time ERC-20 read per *Hub asset*
  — 17 here, not the 14 Spoke reserves, since the address comes from `AddAsset` — immutable and
  cached forever, or a static token list. Cheap, but it is enrichment, not indexed data. **[built]**
  — see the README's enrichment section; it reads from the token rather than a list, because a list
  has no `name` and is silent on anything listed after its release.
- **Market price and oracle deviation.** The protocol's own oracle answers "what is this worth
  *to Aave*", which is what drives liquidation and is therefore the right price for a position
  view. An independent market price (§11) is a separate enrichment concern.
- Logos, protocol branding, cross-protocol aggregation, and any other presentation data that
  is not Aave state.

### 12.6 What is computed when

| lifetime | data | refreshed by |
|---|---|---|
| immutable | `underlying`, `decimals`, `hub`, `assetId` per reserve | `AddAsset` / `AddReserve`, once |
| versioned | reserve config, dynamic config per `dynamicConfigKey` | config events, never overwritten in place (§3) |
| per block | `drawnIndex` (checkpoint + linear interpolation), prices | `UpdateAsset`, `AnswerUpdated` (§5.3, §7.4) |
| per request | amounts, USD values, health factor | pure arithmetic over the rows above |

Two contract details that belong in the response, not just the implementation: every payload is
**block-stamped**, since HF and amounts are per-block quantities (§7.2); and every `uint256` is
serialised **as a string**, because float64 silently corrupts share balances (§7.5).

---

## Sources

- [aave/aave-v4](https://github.com/aave/aave-v4) @ [`2524fe4`](https://github.com/aave/aave-v4/tree/2524fe4018a42750300e114f2a8c4355df62a878)
  — all §5 line references are pinned to this commit. Interfaces under `src/spoke/interfaces/`
  and `src/hub/interfaces/`; math under `src/hub/libraries/` and `src/libraries/math/`;
  Anvil deploy script under `scripts/deploy/examples/`
- [Aave v4 docs](https://aave.com/docs/aave-v4) · [addresses](https://aave.com/docs/resources/addresses) · [liquidity model](https://aave.com/docs/aave-v4/liquidity)
- [Aave V4 is Live on Ethereum](https://aave.com/blog/aave-v4-live-ethereum) · [Understanding Aave V4's Architecture](https://aave.com/blog/understanding-aave-v4s-architecture)
- [Anatomy of the Aave v4 contracts](https://jeancvllr.medium.com/anatomy-of-the-aave-v4-contracts-364fa3189d04)
- Live mainnet reads via `eth.drpc.org`, 2026-07-31, block `25652535`
