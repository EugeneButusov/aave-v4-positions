-- `hub_assets_current` is gone; `141` answers for a named instant.
--
-- **The name was where the bug hid.** A view called "current" reads as the
-- obvious right-hand side of any join, and the page took it while asking to be
-- valued at an instant the caller named. Its three readers say
-- `hub_assets_as_of(cut = now())` now, which is the same rows and one visible
-- word about which instant they are.
DROP VIEW IF EXISTS hub_assets_current;
