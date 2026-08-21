//! The statement one page is read with.
//!
//! **Two halves, and the seam is the query's own.** The inner seek is a
//! complete `SELECT` over one wallet's contiguous rows; the outer resolution
//! joins the registry and the Hub onto whatever it returned. Every server
//! parameter is in the seek, so the resolution is one literal and the seek is
//! the only thing assembled.
//!
//! No query builder: none of them speaks ClickHouse's `{name:Type}` parameters,
//! and this SQL has to stay diffable against `clickhouse-position-store.ts`
//! with the `EXPLAIN` findings beside the shapes they justify.

use alloy_primitives::Address;

/// The resolution, with `{seek}` where the paged subquery goes.
///
/// Neither slot is a server parameter — those carry a type, `{name:Type}` — and
/// both are substituted before the statement is sent.
const RESOLVE: &str = r"SELECT
    p.chain_id                      AS chain_id,
    p.user                          AS user,
    p.spoke                         AS spoke,
    toString(p.reserve_id)          AS reserve_id,
    toString(p.supplied_shares)     AS supplied_shares,
    toString(p.drawn_shares)        AS drawn_shares,
    toString(p.premium_shares)      AS premium_shares,
    toString(p.premium_offset_ray)  AS premium_offset_ray,
    toString(p.net_supplied_amount) AS net_supplied_amount,
    toString(p.net_borrowed_amount) AS net_borrowed_amount,
    p.using_as_collateral           AS using_as_collateral,
    toInt32(p.events)               AS events,
    toString(r.asset_id)            AS asset_id,
    r.hub                           AS hub,
    a.underlying                    AS underlying,
    a.decimals                      AS decimals,
    toString(a.liquidity)           AS liquidity,
    toString(a.added_shares)        AS added_shares,
    toString(a.drawn_shares)        AS asset_drawn_shares,
    toString(a.swept)               AS swept,
    toString(a.premium_shares)      AS asset_premium_shares,
    toString(a.premium_offset_ray)  AS asset_premium_offset_ray,
    toString(a.deficit_ray)         AS deficit_ray,
    toString(a.realized_fees)       AS realized_fees,
    a.liquidity_fee                 AS liquidity_fee,
    toString(a.drawn_index)         AS drawn_index,
    toString(a.drawn_rate)          AS drawn_rate,
    toString(toUnixTimestamp(a.index_timestamp)) AS checkpoint_at
FROM (
{seek}
) AS p
-- **A join, not the UNION ALL the collateral flag got.** The two cases
-- differ structurally, and EXPLAIN indexes = 1 shows how.
--
-- The left side prunes. Both branches of user_positions_current report
-- PrimaryKey Keys: chain_id, user, spoke with the wallet predicate as
-- their condition and Search Algorithm: binary search — that is the
-- UNION ALL pushdown the flag was shaped for, doing its job.
--
-- The right sides do not, and cannot. All three read with
-- PrimaryKey Condition: true — no predicate pushed in at all. The step
-- is JOIN FillRightFirst: the right relation is built before a single
-- left row is seen, so there is no key to filter by. A hash join
-- materialises its whole right side by construction.
--
-- **UNION ALL would not change that.** Its advantage is exactly the
-- pushdown above, and the Hub dimension has no predicate to push: it is
-- keyed (chain, hub, asset_id), neither known until the registry
-- resolves. It is not even expressible — UNION ALL needs one grouping
-- key every branch shares, and the Hub dependency is transitive rather
-- than co-keyed.
--
-- Reading the right side whole is fine when it *is* 17 rows, which is
-- what the join was argued on. What is not fine is that producing those
-- 17 reads 8 granules across 5 parts of hub_asset_state — the view
-- collapses and argMaxes the lot on every page. Making that dimension a
-- table is the fix; the README's 'Not here yet' says how.
--
-- LEFT, because a position must survive a reserve the registry has not
-- seen. The nulls that produces are reported as nulls rather than zeros.
LEFT JOIN spoke_reserves_current AS r
       ON r.chain_id = p.chain_id AND r.spoke = p.spoke AND r.reserve_id = p.reserve_id
LEFT JOIN hub_assets_current AS a
       ON a.chain_id = r.chain_id AND a.hub = r.hub AND a.asset_id = r.asset_id
-- Qualified, and it has to be. Unqualified, reserve_id binds to the
-- toString alias above and sorts the decimal digits as text, putting 13
-- before 3 — which then hands the next page a key from the wrong row.
ORDER BY p.spoke, p.reserve_id";

/// The seek: one wallet's contiguous rows, paged.
///
/// **`{narrowing}` is the only clause that is not always here, and that was
/// measured.** `spoke` narrows by equality when it is given and is absent when
/// it is not, because `({spoke:String} = '' OR spoke = {spoke:String})` — the
/// shape that would make this a single constant — prunes to the same granule
/// but drops `Search Algorithm: binary search` to `generic exclusion search`,
/// which is the property the resolution's comment cites as evidence.
///
/// **The resume key needs no slot at all.** `('', 0)` is below every real key,
/// since a Spoke address is never the empty string — so the beginning of the
/// listing is a value rather than a missing predicate. Measured: same
/// condition, same granule, same binary search as omitting it.
const SEEK: &str = r"    SELECT *
    FROM user_positions_current
    -- The leading pair of the sorting key, so the scan starts at this
    -- wallet's rows rather than filtering its way to them.
    WHERE chain_id = {chainId:UInt32}
      AND user = {user:String}
      -- Deliberately `!= 0` rather than §12.1's `> 0`. Shares cannot go
      -- negative on chain, so a negative fold is drift, and it should
      -- surface as a visibly wrong number for §9 to catch rather than
      -- vanish behind the filter that hides closed positions.
      AND (supplied_shares != 0 OR drawn_shares != 0)
      -- The whole of what the pinned prefix leaves free, compared as a
      -- pair even when the narrowing below pins the first half. One
      -- comparison rather than two branches: a reserve_id-only special
      -- case would silently read a resume point from one Spoke against
      -- another's rows if the two ever disagreed.
      AND (spoke, reserve_id) > ({afterSpoke:String}, {afterReserve:UInt256}){narrowing}
    ORDER BY user, spoke, reserve_id
    -- One more than asked. The extra row's presence is what says there
    -- is a next page; counting the whole result set to find out would
    -- defeat keyset paging.
    LIMIT {limit:UInt32}";

/// What fills `{narrowing}` when the caller named a Spoke.
const NARROWED_TO_ONE_SPOKE: &str = "\n      AND spoke = {spoke:String}";

/// The statement for one page.
pub(super) fn list(spoke: Option<Address>) -> String {
    let narrowing = if spoke.is_some() {
        NARROWED_TO_ONE_SPOKE
    } else {
        ""
    };

    RESOLVE
        .replace("{seek}", SEEK)
        .replace("{narrowing}", narrowing)
}

// The relaxation the valuation modules take: the arithmetic in a vector is the
// vector, and an instant a test names is clearer added than saturated.
/// The text, before any server sees it. Nothing here opens a connection: this
/// module builds a string, and what the string does to a database is
/// [`super::position_store`]'s to prove.
#[cfg(test)]
mod tests {
    use alloy_primitives::Address;

    use super::{RESOLVE, list};

    #[test]
    fn splices_the_seek_where_the_subquery_goes() {
        let statement = list(None);

        assert!(!statement.contains("{seek}"), "{statement}");
        assert!(!statement.contains("{narrowing}"), "{statement}");
        assert!(statement.contains("FROM (\n    SELECT *"), "{statement}");
        assert!(
            statement.contains("LIMIT {limit:UInt32}\n) AS p"),
            "{statement}"
        );
    }

    #[test]
    fn narrows_to_one_spoke_only_when_one_is_named() {
        assert!(!list(None).contains("AND spoke = {spoke:String}"));
        assert!(list(Some(Address::ZERO)).contains("AND spoke = {spoke:String}"));
    }

    /// Every parameter is inside the seek, which is what lets the
    /// resolution be one literal rather than a second thing to assemble.
    #[test]
    fn the_resolution_binds_no_parameters() {
        assert!(!RESOLVE.replace("{seek}", "").contains(":UInt"));
        assert!(!RESOLVE.replace("{seek}", "").contains(":String"));
    }

    /// Unqualified, `reserve_id` binds to the `toString` alias and sorts
    /// the decimal digits as text, putting 13 before 3 — which hands the
    /// next page a key from the wrong row.
    #[test]
    fn orders_the_page_by_the_source_column_not_the_alias() {
        let statement = list(None);

        assert!(
            statement.contains("ORDER BY p.spoke, p.reserve_id"),
            "{statement}"
        );
        assert!(
            !statement.contains("\nORDER BY p.spoke, reserve_id"),
            "{statement}"
        );
    }

    /// A `reserve_id`-only comparison would resume at "> 7" and lose the
    /// second Spoke's reserve 7, which sorts after the first Spoke's.
    #[test]
    fn resumes_on_the_whole_of_what_the_pinned_prefix_leaves_free() {
        assert!(
            list(None).contains(
                "AND (spoke, reserve_id) > ({afterSpoke:String}, {afterReserve:UInt256})"
            )
        );
    }

    /// Shares cannot go negative on chain, so a negative fold is drift and
    /// should surface as a wrong number rather than vanish behind the
    /// filter that hides closed positions.
    #[test]
    fn hides_a_closed_position_without_hiding_a_negative_one() {
        assert!(list(None).contains("AND (supplied_shares != 0 OR drawn_shares != 0)"));
    }
}
