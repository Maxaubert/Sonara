import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";
import { SonaraPlayer } from "@sonara/player/react";
import { bridgeClient } from "./bridge.js";

const client = bridgeClient();

const SAMPLE =
  "Sonara reads text aloud, one sentence at a time. Use previous and next to step through the sentences. " +
  "Pause holds the place, and restart goes back to the first sentence. Mute keeps reading in silence.";

function App() {
  const [text, setText] = useState(SAMPLE);
  const [error, setError] = useState(null);
  const speak = async (opts) => {
    setError(null);
    try {
      await client.speak(text, { ...opts, label: "Demo text" });
    } catch (err) {
      setError(err.message);
    }
  };
  return (
    <>
      <h1>Sonara player demo</h1>
      <p className="note">The page talks to a small Node server that holds the @sonara/client connection to sonarad.</p>
      <SonaraPlayer client={client} />
      <label>
        Text to read
        <textarea name="text" autoComplete="off" value={text} onChange={(e) => setText(e.target.value)} />
      </label>
      <div className="actions">
        <button type="button" onClick={() => speak({ mode: "append" })}>Add to queue</button>
        <button type="button" onClick={() => speak({ mode: "replace", interrupt: true })}>Read now</button>
      </div>
      {error ? <p role="alert">{error}</p> : null}
    </>
  );
}

createRoot(document.getElementById("root")).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
