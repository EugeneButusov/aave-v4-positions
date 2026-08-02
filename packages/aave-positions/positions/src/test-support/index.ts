/**
 * The ledger fixtures, exported so a sibling package can build on the folds
 * this one owns.
 *
 * Published for the reason `@packages/indexing` publishes its own: the
 * enrichment package's listing sources read `hub_asset_state` and
 * `spoke_reserves_current`, so its specs need a migrated database with real
 * rows in it. Two independent definitions of "a Hub that has listed USDC"
 * would drift, and the one that drifted would be the one nobody was reading.
 *
 * **Named re-exports rather than `export *`**, because the two ledgers share
 * several names on purpose — `reportDeficit` exists on both contracts with
 * different signatures (§4.5), and `At` and `USDC` are common to both. A star
 * export would pick a winner silently.
 */
export {
  ALICE,
  BOB,
  CHAIN_ID,
  HUGE,
  ROUTER,
  SECOND_SPOKE,
  SPOKE,
  TABLES,
  addReserve,
  borrow,
  event,
  migratedDatabase,
  setCollateral,
  supply,
  withdraw,
  type At,
} from './spoke-ledger';

export { HUB, HUB_TABLES, addAsset, hubEvent, updateAsset } from './hub-ledger';
