import type { ApiError, ErrorCode } from './gen/api';
import { t } from '../i18n/i18n';

/** A call that did not work: the nexus's or workspace's error code and text. */
export class ApiFailure extends Error {
  constructor(
    readonly code: ErrorCode,
    message = '',
    /** The HTTP status, for HTTP calls. */
    readonly status = 0,
  ) {
    super(message || code);
    this.name = 'ApiFailure';
  }

  static from(error: ApiError, status = 0): ApiFailure {
    return new ApiFailure(error.code, error.message ?? '', status);
  }
}

export function isFailure(e: unknown, code?: ErrorCode): e is ApiFailure {
  return e instanceof ApiFailure && (code === undefined || e.code === code);
}

/** What to tell the user about an error: our text for its code, and its detail. */
export function errorText(e: unknown): string {
  if (e instanceof ApiFailure) {
    const base = t(`error.${e.code}`);
    const detail = e.message && e.message !== e.code ? e.message : '';
    return detail && detail !== base ? `${base} (${detail})` : base;
  }
  if (e instanceof Error) return e.message;
  return String(e);
}
