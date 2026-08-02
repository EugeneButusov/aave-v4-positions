import type { Abi } from 'viem';

import { SPOKE_ABI, isIngestedSpokeEvent } from '../aave/spoke-events';
import { ContractLogDecoder } from './decoder';

/**
 * Everything read out of one Spoke: the eight events that move a position, and
 * the five that decide what it counts as.
 *
 * `topic1` is the reserve id on most of them and the *user* on the two refresh
 * events, so nothing may read a topic by position without knowing the event —
 * which is why the projections go through `body` instead (§4.5).
 */
export class SpokeEventDecoder extends ContractLogDecoder {
  protected readonly abi: Abi = SPOKE_ABI;
  protected readonly role = 'spoke';

  protected wanted(eventName: string): boolean {
    return isIngestedSpokeEvent(eventName);
  }
}
