/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */

import type { WriteSet_DirectWriteSet } from './WriteSet_DirectWriteSet';
import type { WriteSet_ScriptWriteSet } from './WriteSet_ScriptWriteSet';

export type WriteSet = (WriteSet_ScriptWriteSet | WriteSet_DirectWriteSet);

