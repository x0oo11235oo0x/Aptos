/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

import type { EventKey } from './EventKey';
import type { MoveType } from './MoveType';
import type { U64 } from './U64';

export type VersionedEvent = {
    version: U64;
    key: EventKey;
    sequence_number: U64;
    type: MoveType;
    data: any;
};

