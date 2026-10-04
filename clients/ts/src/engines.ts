/**
 * External engines (protocol 1.2, capability `engines`): speech engines the
 * user adds at run time, such as OpenAI or a local OpenAI-compatible server.
 * Each method sends one core message as is. The profile is named by the
 * `engine` field (a request's `id` is its correlation id). A runtime that refuses external
 * engines (`sonarad --no-external-engines`) answers `E_UNSUPPORTED`.
 *
 * A key is sent only in `add` (`secret`), `setKey`, or with a draft profile
 * in `models`; the runtime stores it in Windows Credential Manager (a
 * draft's only for that request) and never returns it.
 *
 * Sonara names no model or voice of its own (protocol 1.5, runtime 0.19.0):
 * `models` and `voices` list the provider's live, and a profile without one
 * it needs says so in `engine_list` (`missing`).
 *
 * "Send to the engine" (`send_mode`, runtime 0.19.0, #235): `"message"` sends
 * a whole released message in one request (the default of the cloud kinds),
 * `"sentence"` each sentence as it comes (the default of a program and a
 * server on this PC). The view in `engine_list` tells the mode in force, and
 * `explicit.send_mode` whether the profile chose it.
 *
 * A `command` engine (a program on the user's PC) is never added or changed
 * through the protocol: `add` of one, or replacing one, is `E_FORBIDDEN`
 * (protocol 1.3). The user adds it locally (`sonara engines add <id> --kind
 * command`, or `engines.json`); `reload` makes a running runtime read
 * `engines.json` again. Listing, testing, selecting and removing one work.
 */
import type { Reply } from "./types.js";

export type Send = (type: string, fields: Record<string, unknown>) => Promise<Reply>;

/** How text goes to an engine (#235): a whole message, or each sentence. */
export type SendMode = "message" | "sentence";

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
  /** Unset (or null): the kind's default (`message` for the cloud). */
  send_mode?: SendMode | null;
  options?: Record<string, unknown>;
}

export interface EngineAddOptions {
  /** Stored in Credential Manager (forces `key_ref` `credman` when unset). */
  secret?: string;
  /** Change an existing profile; its stored key is kept unless `secret` is given. */
  replace?: boolean;
}

/** `engine_models` of a profile not saved yet (the key typed in a form). */
export interface EngineModelsDraft {
  /** The profile as it would be added (its `id` and `model` may be missing). */
  profile: Omit<EngineProfile, "id"> & { id?: string };
  /** Used for this request only, never stored. */
  secret?: string;
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

  /**
   * `engine_reload` (protocol 1.3): the runtime reads `engines.json` again.
   * It takes no profile. Replies like `list`, plus `problems`.
   */
  reload(): Promise<Reply> {
    return this.send("engine_reload", {});
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

  /**
   * `engine_models` (protocol 1.5): the provider's models now, of a saved
   * engine (`refresh` asks the provider again) or of a draft profile.
   * Replies `{models: [{id, name}], list, takes_model, required, error?}`.
   */
  models(engine: string | EngineModelsDraft, opts: { refresh?: boolean } = {}): Promise<Reply> {
    if (typeof engine !== "string") {
      const fields: Record<string, unknown> = { profile: engine.profile };
      if (engine.secret !== undefined) fields.secret = engine.secret;
      return this.send("engine_models", fields);
    }
    const fields: Record<string, unknown> = { engine };
    if (opts.refresh !== undefined) fields.refresh = opts.refresh;
    return this.send("engine_models", fields);
  }

  /** `voices` of one engine; `refresh` asks the provider again. */
  voices(engine: string, opts: { refresh?: boolean } = {}): Promise<Reply> {
    const fields: Record<string, unknown> = { engine };
    if (opts.refresh !== undefined) fields.refresh = opts.refresh;
    return this.send("voices", fields);
  }
}
