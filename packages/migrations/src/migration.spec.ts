import { mkdtemp, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

import { describe, expect, it } from 'vitest';

import { assertOrderable, loadMigrations, ordered, type Migration } from './migration';

const at = (id: string): Migration => ({ id, statement: 'SELECT 1' });

/** A migrations directory, as a package would ship one. */
async function directory(files: Record<string, string>): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), 'migrations-'));
  await Promise.all(
    Object.entries(files).map(([name, body]) => writeFile(join(dir, name), body, 'utf8')),
  );
  return dir;
}

describe('loadMigrations', () => {
  it('reads each .sql file verbatim, taking the id from its name', async () => {
    const body = '-- why this table exists\nCREATE TABLE spoke_events (x UInt8);\n';
    const dir = await directory({ '001_spoke_events.sql': body });

    // Prose and terminator included: both servers accept them, so nothing is
    // stripped — and stripping would mean parsing SQL to know what to strip.
    await expect(loadMigrations([dir])).resolves.toEqual([
      { id: '001_spoke_events', statement: body },
    ]);
  });

  it('ignores anything that is not a .sql file', async () => {
    const dir = await directory({
      '001_spoke_events.sql': 'SELECT 1',
      'README.md': 'notes for whoever adds the next one',
      '.DS_Store': '',
    });

    const loaded = await loadMigrations([dir]);

    expect(loaded.map((m) => m.id)).toEqual(['001_spoke_events']);
  });

  it('interleaves directories by ordinal rather than grouping them', async () => {
    const events = await directory({ '001_spoke_events.sql': 'a', '003_prices.sql': 'c' });
    const hub = await directory({ '002_hub_assets.sql': 'b' });

    // Package boundaries are not schema boundaries: a later migration in one
    // package can depend on an earlier one in another, so the ordinal has to
    // win over which directory was listed first.
    const loaded = await loadMigrations([events, hub]);

    expect(loaded.map((m) => m.id)).toEqual(['001_spoke_events', '002_hub_assets', '003_prices']);
  });

  it('refuses two directories that claim the same ordinal', async () => {
    const events = await directory({ '002_spoke_events.sql': 'a' });
    const hub = await directory({ '002_hub_assets.sql': 'b' });

    await expect(loadMigrations([events, hub])).rejects.toThrow(/ordinal 002 is claimed by both/);
  });
});

describe('ordered', () => {
  it('sorts by ordinal, whatever order the caller assembled', () => {
    // The application concatenates one array per contributing package, and
    // nothing says it does so in schema order.
    const sorted = ordered([at('003_prices'), at('001_spoke_events'), at('002_hub_assets')]);

    expect(sorted.map((m) => m.id)).toEqual(['001_spoke_events', '002_hub_assets', '003_prices']);
  });

  it('leaves the array the caller passed untouched', () => {
    const input = [at('002_hub_assets'), at('001_spoke_events')];

    ordered(input);

    expect(input.map((m) => m.id)).toEqual(['002_hub_assets', '001_spoke_events']);
  });
});

describe('assertOrderable', () => {
  it('rejects two packages claiming the same ordinal, naming both', () => {
    // The failure this exists for: packages own their own migrations, so two of
    // them can each reach for 002 without either author seeing the other. Left
    // alone, the apply order would depend on how the app happened to
    // concatenate the arrays — reproducible until someone reorders the list.
    expect(() => assertOrderable([at('002_hub_assets'), at('002_prices')])).toThrow(
      /ordinal 002 is claimed by both "002_hub_assets" and "002_prices"/,
    );
  });

  it.each(['1_spoke_events', 'spoke_events', '0001_spoke_events', '001-spoke-events', '001_Spoke'])(
    'rejects the unorderable name %s',
    (id) => {
      expect(() => assertOrderable([at(id)])).toThrow(/must be named NNN_snake_case/);
    },
  );

  it('accepts a set whose ordinals are unique', () => {
    expect(() => assertOrderable([at('001_spoke_events'), at('002_hub_assets')])).not.toThrow();
  });

  it('accepts an empty set, so a service with no tables of its own still runs', () => {
    expect(() => assertOrderable([])).not.toThrow();
  });
});
