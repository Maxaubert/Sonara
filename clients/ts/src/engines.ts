/**
 * External engines (protocol 1.2, capability `engines`): speech engines the
 * user adds at run time, such as OpenAI or a local OpenAI-compatible server.
 * Each method sends one core message as is. The profile is named by the
 * `engine` field (a request's `id` is its correlation id). A runtime that refuses external
 * engines (`sonarad --no-external-engines`) answers `E_UNSUPPORTED`.
 *
 * A key is sent only in `add` (`secret`) or `setKey`; the runtime stores it
 * in Windows Credential Manager and never returns it.
 */
import type { Reply } from "./types.js";

export type Send = (type: string, fields: Record<string, unknown>) => Promise<Reply>;

/** A profile as sent in `engine_add` (spec section 5). */
export interface EngineProfile {
  id: string;
  kind: string;
  label?: string;
  url?: string;
  model?: string;
  voice?: string;
  /** `"none"`, `"credman"` or `"env:NAME"`. */
  key_ref?: string | null;
  options?: Record<string, unknown>;
}

export interface EngineAddOptions {
  /** Stored in Credential Manager (forces `key_ref` `credman` when unset). */
  secret?: string;
  /** Change an existing profile; its stored key is kept unless `secret` is given. */
  replace?: boolean;
}

export interface EngineTestOptions {
  text?: string;
  voice?: string;
  /** Play it over whatever is read (default true). */
  play?: boolean;
}

export class EnginesApi {
  constructor(private readonly send: Send) {}

  /** `engine_list`: the profiles, the built-in engines, kinds and presets. */
  list(): Promise<Reply> {
    return this.send("engine_list", {});
  }

  /** `engine_add`. It does not select the engine: `set engine` does. */
  add(profile: EngineProfile, opts: EngineAddOptions = {}): Promise<Reply> {
    const fields: Record<string, unknown> = { engine: profile };
    if (opts.secret !== undefined) fields.secret = opts.secret;
    if (opts.replace !== undefined) fields.replace = opts.replace;
    return this.send("engine_add", fields);
  }

  /** `engine_remove`; the stored key goes too unless `forgetKey` is false. */
  remove(id: string, opts: { forgetKey?: boolean } = {}): Promise<Reply> {
    const fields: Record<string, unknown> = { engine: id };
    if (opts.forgetKey !== undefined) fields.forget_key = opts.forgetKey;
    return this.send("engine_remove", fields);
  }

  /** `engine_key`: store a key, or delete it with `null`. */
  setKey(id: string, secret: string | null): Promise<Reply> {
    return this.send("engine_key", { engine: id, secret });
  }

  /** `engine_test`: one synthesis with no fallback, at the current rate. */
  test(id: string, opts: EngineTestOptions = {}): Promise<Reply> {
    const fields: Record<string, unknown> = { engine: id };
    if (opts.text !== undefined) fields.text = opts.text;
    if (opts.voice !== undefined) fields.voice = opts.voice;
    if (opts.play !== undefined) fields.play = opts.play;
    return this.send("engine_test", fields);
  }

  /** `voices` of one engine; `refresh` asks the provider again. */
  voices(engine: string, opts: { refresh?: boolean } = {}): Promise<Reply> {
    const fields: Record<string, unknown> = { engine };
    if (opts.refresh !== undefined) fields.refresh = opts.refresh;
    return this.send("voices", fields);
  }
}
