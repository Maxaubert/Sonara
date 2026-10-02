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

  constructor(code: ErrorCode, message: string) {
    super(`${code}: ${message}`);
    this.name = "SonaraError";
    this.code = code;
  }
}
