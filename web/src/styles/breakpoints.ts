/**
 * The shell breakpoints from tokens.css (`--breakpoint-*`), for scripts that
 * need `matchMedia`. tokens.test.ts keeps the two in step.
 */

export const PHONE_MAX_WIDTH = 760;
export const WIDE_MIN_WIDTH = 1181;

export const PHONE_QUERY = `(max-width: ${PHONE_MAX_WIDTH}px)`;
export const MEDIUM_QUERY = `(min-width: ${PHONE_MAX_WIDTH + 1}px) and (max-width: ${WIDE_MIN_WIDTH - 1}px)`;
export const WIDE_QUERY = `(min-width: ${WIDE_MIN_WIDTH}px)`;
