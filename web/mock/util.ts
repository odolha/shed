// Small helpers shared by the mock's parts.

import { randomBytes } from 'node:crypto';
import type * as Api from '../src/api/gen/api.ts';
import type { ErrorCode } from '../src/api/gen/api.ts';

/** The API version spoken (a type error here when shed-api's changes). */
export const API_VERSION: typeof Api.API_VERSION = 1;

/** An error a call answers with. */
export class Fail extends Error {
  code: ErrorCode;
  constructor(code: ErrorCode, message = '') {
    super(message || code);
    this.code = code;
  }
}

export function now(): number {
  return Math.floor(Date.now() / 1000);
}

export function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

let counter = 0;

/** A fresh id: `t-3f9a1c2b`. */
export function uid(prefix: string): string {
  counter += 1;
  return `${prefix}-${randomBytes(4).toString('hex')}${counter.toString(16)}`;
}

export function hex(bytes: number): string {
  return randomBytes(bytes).toString('hex');
}

export const env = {
  port: Number(process.env['PORT'] ?? 8787),
  fresh: process.env['MOCK_FRESH'] === '1',
  viewOnly: process.env['MOCK_VIEW'] === '1',
  latency: Number(process.env['MOCK_LATENCY'] ?? 0),
  noReview: process.env['MOCK_NO_REVIEW'] === '1',
};

export const HOME = '/home/you';
