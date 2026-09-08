-- `spoke_reserves_current` is gone; `096` answers for a named instant.
--
-- Three readers, and the third is why this is worth stating: the position
-- store, its Rust port, and `packages/prices` listing the reserves it needs to
-- price. That last one wants the newest listing and now says so —
-- `spoke_reserves_as_of(cut = now())` — rather than getting it from a name.
DROP VIEW IF EXISTS spoke_reserves_current;
