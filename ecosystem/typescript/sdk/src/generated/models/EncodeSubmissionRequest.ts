/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

import type { Address } from './Address';
import type { TransactionPayload } from './TransactionPayload';
import type { U64 } from './U64';

export type EncodeSubmissionRequest = {
    sender: Address;
    sequence_number: U64;
    max_gas_amount: U64;
    gas_unit_price: U64;
    expiration_timestamp_secs: U64;
    payload: TransactionPayload;
    secondary_signers?: Array<Address>;
};

