# sonara-client

Talk to the Sonara text-to-speech runtime from Python 3.9+. Standard library only.

```python
import sonara_client

with sonara_client.connect("my-app", runtime_path=r"C:\path\to\sonarad.exe") as sonara:
    item = sonara.speak("Hello.", label="greeting")
    sonara.set("rate", 240)
    with sonara.subscribe(["state", "items"]) as events:
        for e in events:
            if e["event"] == "item" and e["item_id"] == item and e["phase"] != "started":
                break
```

- `connect(client_name, *, runtime_path=None, home=None, autostart=True, require=(), extensions=(), keep_alive=False)`: finds the shared runtime through `runtime.json`, starts the bundled `sonarad.exe` when none is usable (`runtime_path`, default `SONARA_RUNTIME`), takes over an idle incompatible one.
- `speak(text, mode=None, interrupt=None, label=None)` returns the item id; `control(action)`; `set(key, value)` / `get(key)`; `voices(engine=None)`.
- `subscribe(events)` opens an event stream on its own connection: iterate it, or `read(timeout)`.
- `channels`, `agent`, `system`: extension namespaces that send the protocol's extension messages as they are.
- Errors are `SonaraError` with a `code`. The client is thread-safe.

Guide: [Bundle Sonara in your app](https://github.com/Maxaubert/Sonara/blob/main/docs/bundling.md). Tests: `python -m pytest clients/python/tests -q` (the end-to-end ones need `cargo build -p sonarad`).

MIT. Ship `THIRD_PARTY_NOTICES.md` with an app that bundles the runtime.
