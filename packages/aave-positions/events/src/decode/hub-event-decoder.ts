import type { Abi } from 'viem';

import { HUB_ABI, isHubStateEvent } from '../aave/hub-events';
import { ContractLogDecoder } from './decoder';

/**
 * The thirteen asset-state events of one Hub.
 *
 * `topic1` is the **asset id** here, not a reserve id — the same column in a
 * different ledger means a different thing, which is half of why the Hub gets
 * its own table.
 */
export class HubEventDecoder extends ContractLogDecoder {
  protected readonly abi: Abi = HUB_ABI;
  protected readonly role = 'hub';

  protected wanted(eventName: string): boolean {
    return isHubStateEvent(eventName);
  }
}
