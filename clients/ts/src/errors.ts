/**
 * Error codes. The `E_*` codes of protocol v1 come from the runtime as is;
 * the client adds three of its own for what happens before or after a reply.
 */
export type ErrorCode =
  | "E_AUTH"
  | "E_BAD_REQUEST"
  | "E_UNKNOWN_TYPE"
  | "E_UNSUPPORTED"
  | "E_INCOMPATIBLE"
  | "E_BUSY"
  | "E_ENGINE"
  | "E_NOT_FOUND"
  /** Protocol 1.3: never allowed over the protocol (adding or changing a `command` engine). */
  | "E_FORBIDDEN"
  /** No runtime is running and none could be started (autostart off or no runtimePath). */
  | "E_NOT_RUNNING"
  /** The bundled runtime did not start or wrote no runtime.json within the wait. */
  | "E_START_FAILED"
  /** The connection closed before the reply (the runtime exited or close() was called). */
  | "E_CLOSED"
  | (string & {});

/** Any failure of a Sonara call: a coded protocol error or a client-side one. */
export class SonaraError extends Error {
  readonly code: ErrorCode;
  /**
   * Protocol 1.2: why an external engine failed (`auth`, `quota`,
   * `network`...), on an `E_ENGINE` of `engine_test`.
   */
  readonly reason?: string;

  constructor(code: ErrorCode, message: string, reason?: string) {
    super(`${code}: ${message}`);
    this.name = "SonaraError";
    this.code = code;
    if (reason !== undefined) this.reason = reason;
  }
}
