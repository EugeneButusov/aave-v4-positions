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
| `0xa1facf110ded5028ee267fa3d5986f2aa4dc14230b79ffd27e95760f14883350` | `UpdateAsset(uint256,uint256,uint256,uint256)` |
| `0xb233dd05ed21346e144167b35a6213bcf04768dbdffdc8339e8b027b94b9f305` | `Add(uint256,address,uint256,uint256)` |
| `0x535be2ff85ab4c5d0991e10dc057a4951ea2bac426ffb036eded23036a3942b2` | `Remove(uint256,address,uint256,uint256)` |
| `0xe2497bc41b1fa7c4ba996f24dc2affdffb2a5571584db6db0eed8fbbf1dc8517` | `Draw(uint256,address,uint256,uint256)` |
| `0x119e7f996dc987b3ae79eb3735f1620c4292f6a7761a1e0f834c445f7798b912` | `Restore(uint256,address,uint256,(int256,int256,uint256),uint256,uint256)` |
| `0x3fa96ecf17429fddfbb919a64196f4e43f71b57f0c5c38c49a21c8e1e763d18c` | `RefreshPremium(uint256,address,(int256,int256,uint256))` |
| `0x4845ee5c72bde2b62defc8a1ca2f0fc3313b2d9e799997ce4f6776da9773bcbf` | `ReportDeficit(uint256,address,uint256,(int256,int256,uint256),uint256)` |
| `0xe97b8576ac531cdc817b933309d0518ca3d26c6b46d490f3ae9fa39426a141ee` | `EliminateDeficit(uint256,address,address,uint256,uint256)` |
| `0xafd21228e21de4a3f779e1cc3617e12672c3da091dcf3812a931036aa0bf633c` | `MintFeeShares(uint256,address,uint256,uint256)` |
| `0x69bb3893073d7a893f3933f3871309fc25acfc72e365b71f554d439a85b20e8b` | `Sweep(uint256,address,uint256)` |
| `0x566111831db1f090374baff3c3f9fc512084f5a9b8f5b199fb475d9c43a8013f` | `Reclaim(uint256,address,uint256)` |
| `0x0d93b0e8579bc9db73c85a1fb79d785ffc47f8e20d346253f809cc98c48292a0` | `TransferShares(uint256,address,address,uint256)` |

Volume is modest: 868 Core Hub logs per 10k blocks, half of them `UpdateAsset`. **[verified]**

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
| index accrual, incl. the `drawnShares == 0 && premiumShares == 0` short-circuit | [`AssetLogic.getDrawnIndex`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/libraries/AssetLogic.sol#L153-L165) `:153-165` |
| linear interest term | [`MathUtils.calculateLinearInterest`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/libraries/math/MathUtils.sol#L20-L31) `:20-31` |
| `RAY = 1e27`, `SECONDS_PER_YEAR = 365 days` | `MathUtils.sol:11`, `:13` |
| `drawnShares × index` | [`UserPositionUtils.getDebt`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/libraries/UserPositionUtils.sol#L127-L133) `:127-133` |
| premium term | [`Premium.calculatePremiumRay`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/libraries/Premium.sol#L17-L23) `:17-23`, called from `UserPositionUtils.sol:154-164` |
| `rayMulUp` = `ceil(a*b/RAY)` | [`WadRayMath.sol`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/libraries/math/WadRayMath.sol#L88-L98) `:88-98` |
| `ceilRay` = `fromRayUp` = `ceil(a/RAY)` | `WadRayMath.sol:167-172` |

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
| virtual-share conversion, `Math.Rounding.Floor` | [`SharesMath.toAssetsDown`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/libraries/SharesMath.sol#L31-L42) `:31-42` |
| `VIRTUAL_ASSETS = VIRTUAL_SHARES = 1e6` | `SharesMath.sol:13-14` |
| wrapper supplying `totalAddedAssets` / `addedShares` | [`AssetLogic.toAddedAssetsDown`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/libraries/AssetLogic.sol#L107-L112) `:107-112` |
| `totalAddedAssets` | [`AssetLogic.totalAddedAssets`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/libraries/AssetLogic.sol#L79-L96) `:79-96` |
| `aggregatedOwedRay` | [`AssetLogic._calculateAggregatedOwedRay`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/libraries/AssetLogic.sol#L229-L242) `:229-242` |
| `unrealizedFees` | [`AssetLogic.getUnrealizedFees`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/libraries/AssetLogic.sol#L187-L226) `:187-226` |
| `* liquidityFee / 10000`, floor | [`PercentageMath.percentMulDown`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/libraries/math/PercentageMath.sol#L15-L27) `:15-27`, `PERCENTAGE_FACTOR = 1e4` at `:10` |

Note the supply side depends on the **debt** index — suppliers are paid out of accrued debt,
so the two sides are coupled through `drawnIndex`. **[verified]**

### 5.2b Call chain — how the contracts reach these

Worth recording, because it is what makes the reconciliation job meaningful: our off-chain
code and the on-chain getters must bottom out in the same primitives.

**Supply** — `Spoke.getUserSuppliedAssets` `Spoke.sol:573-580`
→ `Hub.previewRemoveByShares` `Hub.sol:471-473`
→ `AssetLogic.toAddedAssetsDown` `:107-112`
→ `SharesMath.toAssetsDown` `:31-42`

**Debt** — `Spoke.getUserDebt` `Spoke.sol:589-597`
→ `UserPositionUtils.getDebt(hub, assetId)` `:114-120`
→ `Hub.getAssetDrawnIndex` `Hub.sol:508-510`
→ `AssetLogic.getDrawnIndex` `:153-165`
→ back to `UserPositionUtils.getDebt(drawnIndex)` `:127-133`

Both entry points read `_userPositions[user][reserveId]` on the Spoke and divide against
**Hub-global** totals — confirming that user shares and Hub `addedShares` share one unit,
with no per-spoke scaling. This is what the wei-exact match in §5.4 independently demonstrates.

### 5.3 The key finding: the Hub emits its own index

[`AssetLogic.updateDrawnRate`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/libraries/AssetLogic.sol#L132-L138)
`:132-138` emits, at `:137`:

```solidity
emit IHub.UpdateAsset(assetId, drawnIndex, newDrawnRate, asset.realizedFees);
```

topic0 `0xa1facf110ded5028ee267fa3d5986f2aa4dc14230b79ffd27e95760f14883350`, `assetId` indexed.

Why the emitted value is the *settled* index rather than a stale one:
[`AssetLogic.accrue`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/hub/libraries/AssetLogic.sol#L141-L150)
`:141-150` writes `asset.drawnIndex = getDrawnIndex()` and sets
`lastUpdateTimestamp = block.timestamp` *before* `updateDrawnRate` reads it — see the
`:131` docstring, "Uses last stored index; asset accrual should have already occurred."
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

Caveat on coverage: across **all 17 Core Hub assets**, `premiumShares`, `premiumOffsetRay`,
`deficitRay` and `swept` are currently **all zero**. So the supply-side validation exercised
the `premiumRay = 0`, `deficitRay = 0` path only. The general formulas above are transcribed
from source and believed correct, but the premium and deficit branches are **not yet
empirically confirmed** — the local Anvil deployment (§8) is the way to force those states
and test them.

### 5.5 Resulting design

**Fold logs → shares as the ledger; compute assets on read via the formulas above.**

- Source of truth: `suppliedShares` / `drawnShares` per `(spoke, reserveId, user)`, plus a
  per-`(hub, assetId)` mirror of Hub asset state.
- The API needs **no RPC on the read path** — valuation is arithmetic over indexed state.
- `eth_call` is demoted from *required* to a **reconciliation job**: periodically compare
  computed values against `getUserDebt` / `getUserSuppliedAssets` and alert on drift. That is
  a much better use of it, and it turns the wei-exact match above into a standing invariant
  rather than a one-off check.

The cost: the Hub asset mirror must be folded from the Hub's own events (`Add`, `Remove`,
`Draw`, `Restore`, `MintFeeShares`, `Sweep`, `Reclaim`, `ReportDeficit`, `EliminateDeficit`,
`UpdateAsset`) to keep `liquidity`, `addedShares`, `drawnShares`, `swept`, `realizedFees`,
`deficitRay` and the premium fields current. **This is now the highest-risk part of the
ingestion logic** — one mis-folded transition silently corrupts every supply valuation for
that asset. It is also exactly what the reconciliation job is there to catch.

This supersedes §4.4: **Hub events are required after all**, not optional.

Staged fallback if the Hub mirror proves troublesome: ship debt valuation first (§5.1 needs
only `UpdateAsset` + user shares — far less state), and use `eth_call` at `latest` for supply
until the mirror is trusted.

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

Given §5, these are **not on the read path** — they are the reconciliation oracle. The
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

## 7. Health factor — the math is reproducible, prices are the caveat

Short answer: **the HF computation is exactly reproducible off-chain, and 12 of 14 price feeds
are event-reconstructable. Two are not, without a contract read.**

### 7.1 The formula

From [`Spoke._processUserAccountData`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/Spoke.sol#L706-L790)
`:706-790`, iterating only the reserves flagged in the user's `PositionStatus`:

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
[`SpokeUtils.toValue`](https://github.com/aave/aave-v4/blob/2524fe4018a42750300e114f2a8c4355df62a878/src/spoke/libraries/SpokeUtils.sol#L28-L40)
`:28-40` documents it outright: **`1e26` represents 1 USD** — an 18-decimal-normalised amount
times an 8-decimal price (`ORACLE_DECIMALS = 8`, `SpokeUtils.sol:13`). So
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

**Pin the block or the check is meaningless.** An unpinned first attempt showed HF agreeing to
~6 dp but `totalCollateralValue` and `totalDebtValueRay` disagreeing, purely because each
`eth_call` landed on a different block and `drawnIndex` accrues per second. HF is a
*per-block* quantity. Any reconciliation job must pin `blockNumber` across all reads, and any
API response should state the block it was computed at.

### 7.3 Inputs — where each comes from

| input | source | from events? |
|---|---|---|
| `suppliedShares`, `drawnShares`, `premiumShares`, `premiumOffsetRay` | Spoke position events | yes (§4.1) |
| `drawnIndex` | Hub `UpdateAsset` + linear interpolation | yes (§5.3) |
| Hub asset state for the supply ratio | Hub event fold | yes (§5.5) |
| `collateralFactor` at the user's config version | `AddDynamicReserveConfig` / `UpdateDynamicReserveConfig` + user's `dynamicConfigKey` | yes (§4.2, §4.3) |
| collateral / borrowing flags | `SetUsingAsCollateral` + borrow/repay transitions | yes (§4.1) |
| `decimals` | `AddReserve` / reserve registry | yes (§4.3) |
| **price per reserve** | `AaveOracle.getReservePrice` → `IPriceFeed.latestAnswer()` | **partly — see 7.4** |

Everything except price is already in the pipeline. So HF costs us the price layer and nothing
else.

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
| Chainlink × on-chain LST ratio | wstETH (`RATIO_PROVIDER` = stETH `0xae7a…`), weETH (`RATIO_PROVIDER` = weETH `0xCd5f…`) | **no** — ratio is contract state |

`AnswerUpdated(int256,uint256,uint256)` fires **36× per 10k blocks** on the ETH/USD aggregator
`0x7c7FdFCa295a787ded12Bb5c1A49A8D2cC20E3F8`, so the Chainlink half is a normal indexing job.
**[verified]**

Three wrinkles worth planning for:

1. **Proxy vs aggregator.** `AnswerUpdated` is emitted by the *aggregator*, not the proxy the
   oracle points at. Aggregators rotate (Chainlink phases), so historical reconstruction must
   follow that migration rather than watching one fixed address.
2. **LST ratios.** wstETH and weETH multiply ETH/USD by an exchange rate read from the token
   contract. That rate drifts continuously with staking rewards and is not a log. Options:
   sample it periodically and interpolate, or accept `eth_call` for these two.
3. **Cap adapters.** `getPriceCap` currently reads as a static 1.04 with `isCapped = false`.
   Some Aave adapters instead use a *growth-rate* cap with a stored snapshot, which is stateful.
   Confirm which variant these are before assuming a constant. **[open]**

### 7.5 Recommendation

HF is worth serving, but stage it:

- **v1 — HF at `latest`, computed from indexed positions + a price read.** All the
  position-side inputs come free from the pipeline; only prices need an `eth_call`. Cheap,
  exact, and the §7.2 check becomes a standing invariant.
- **v2 — historical HF**, once Chainlink `AnswerUpdated` is indexed. Gets 12/14 reserves
  exactly; wstETH and weETH need sampled ratios, so flag those as approximate rather than
  quietly serving a slightly-wrong number.

The honest framing for the API: HF is **derived and block-stamped**, not indexed. Return the
block it was computed at, and treat the on-chain `getUserAccountData` as the reconciliation
oracle rather than the serving path.

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
solely by the reconciliation job, and only at `latest`. This is a deliberate constraint to
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

## 9. Live state — Main Spoke, block 25652535 **[verified]**

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

## 10. Enrichment candidates

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

## 11. Open questions

1. **Cap-adapter variant** (§7.4) — `getPriceCap` reads as a static 1.04, but some Aave
   adapters use a *growth-rate* cap with a stored snapshot, which is stateful and would not be
   reconstructable from a constant. Confirm which variant the six capped feeds are.
2. **LST exchange rates** (§7.4) — wstETH and weETH prices multiply ETH/USD by a ratio read
   from token contract state, which is not a log. Decide between periodic sampling with
   interpolation and accepting an `eth_call` for those two reserves.
3. **Premium and deficit branches of the supply formula are unvalidated** (§5.4) — all 17 Core
   Hub assets currently have `premiumShares = deficitRay = swept = 0`, so mainnet cannot
   exercise them. Force these states on local Anvil before trusting supply valuation.
4. Whether the Hub asset mirror can be folded from Hub events with zero drift over a full
   backfill (§5.5). The reconciliation job answers this empirically — run it over history
   before relying on computed supply values.
5. Chainlink aggregator rotation (§7.4) — `AnswerUpdated` comes from the aggregator, not the
   proxy. Confirm how phase migrations surface before backfilling historical prices.
6. Do position managers ever emit position events from *their own* address, or always via the
   Spoke? Assumed Spoke-only; affects whether we watch >1 address.
7. Exact `Reserve.flags` bit order in `ReserveFlagsMap.sol` — needed to decode paused/frozen.
8. Whether `TransferShares` on the Hub can move a *user's* position between spokes. If yes it
   is position-affecting, not just valuation-affecting.

*Resolved during this pass:* off-chain valuation without archive access (§5 — yes, wei-exact);
whether the interest-rate strategy must be reimplemented (§5.3 — no, `drawnRate` is emitted);
the unit of `Value` (§7.1 — `1e26` = 1 USD); and whether health factor is reproducible
off-chain (§7.2 — yes, 8/8 exact when block-pinned).

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
