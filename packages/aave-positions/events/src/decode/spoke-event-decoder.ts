import type { Abi } from 'viem';

import { SPOKE_ABI, isPositionEvent } from '../aave/spoke-events';
import { ContractLogDecoder } from './decoder';

/**
 * The eight position events of one Spoke.
 *
 * `topic1` is the reserve id on every one of them; `topic2` and `topic3` mean
 * different things per event.
 */
export class SpokeEventDecoder extends ContractLogDecoder {
  protected readonly abi: Abi = SPOKE_ABI;
  protected readonly role = 'spoke';

  protected wanted(eventName: string): boolean {
    return isPositionEvent(eventName);
  }
}
