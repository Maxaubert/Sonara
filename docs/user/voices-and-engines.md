# Voices and speech engines

This page explains the voices Sonara can use: the built-in Kokoro and Windows voices, and speech engines you add yourself (cloud services, a server on your PC, or your own program).

## Pick a voice

On the **Speech** page, **Engine** picks who speaks and **Voice** picks the voice (the play button previews it). **Rate** sets the speed, 100 to 400 words per minute (default 250).

| Engine | Needs | Works offline |
|---|---|---|
| Kokoro (default) | A one-time model download | Yes, after the download |
| Windows voices (OneCore) | Nothing | Yes |
| An engine you added | The service or program, usually a key | Only if it runs on your PC |

Default voice: `af_sarah`.

## Kokoro

| Fact | Value |
|---|---|
| Voices | 28 English voices, such as `af_sarah` and `af_heart` |
| Model size | About 350 MB, downloaded once |
| Where | `%LOCALAPPDATA%\Sonara\models\` |
| Checked | Against pinned SHA-256 values |
| After download | Runs fully offline (Microsoft's ONNX Runtime, shipped in the runtime zip) |
| Interrupted download | Resumes. A failed one is retried later |

Until Kokoro is ready, Sonara speaks with a Windows voice. `/sonara:doctor` shows the download progress.

## Windows voices (OneCore)

No download is needed. Natural voices sound better: open *Settings > Time & language > Speech > Add voices* and add an English voice.

**Windows voices listed but silent?** On some PCs the voices are listed while their data files are missing, so speech fails with "file not found". Two fixes:

- Remove and add *English (United States)* again under *Settings > Time & language > Speech > Manage voices*.
- Or run this in an elevated prompt: `DISM /Online /Add-Capability /CapabilityName:Language.TextToSpeech~~~en-US~0.0.1.0`

Kokoro voices are not affected.

## Speech engines you add

Besides Kokoro and the Windows voices, Sonara can read with an engine you add. **Nothing is sent anywhere until you add an engine and choose Use.**

| Kind | What it is | Key | Speaking rate it follows |
|---|---|---|---|
| `openai-compatible`, preset `openai` | OpenAI's speech API | Yes | All (100 to 400) |
| `openai-compatible`, other presets | A server on your PC or network that speaks OpenAI's API | Usually none | All, if the server follows it |
| `elevenlabs` | ElevenLabs (cloned voices too) | Yes | 140 to 240 wpm. Faster stays at 240 |
| `azure` | Azure AI Speech (a region and a voice) | Yes | All |
| `google` | Google Cloud Text-to-Speech | Yes | All |
| `gemini` | Gemini speech | Yes (free key from [Google AI Studio](https://aistudio.google.com), *Get API key*) | Roughly: "speaking slowly" or "speaking quickly", no exact speed |
| `cartesia` | Cartesia | Yes | 120 to 300 wpm |
| `deepgram` | Deepgram (the voice is a speech model) | Yes | 140 to 300 wpm. Deepgram's speed support is not confirmed live |
| `command` | A program of yours on this PC, such as Piper: text in, WAV or PCM out | None | The program gets `{rate}` and `{speed}` |

Presets of `openai-compatible`: `openai`, `kokoro-fastapi`, `localai`, `speaches`, `openedai-speech`, `chatterbox-api`, `chatterbox-server`, `generic`. Chatterbox servers ignore the rate. You can add up to 16 engines.

### No model or voice is built in

Providers add and retire models and voices all the time, so Sonara names none of its own and never picks one for you. It lists what the provider offers now, fetched with your key, and you choose.

- A provider without a list (OpenAI's voices, Cartesia's models) gets a text field and a link to where it names them.
- An engine that still needs one says *Choose a model* or *Choose a voice*, and Kokoro reads meanwhile.
- Examples on this page use `<model>` and `<voice>` as placeholders.

### Add one from the settings page

1. Open `/sonara:settings`, then the **Engines** page (or choose *Add new engine...* at the end of the **Engine** list on the Speech page).
2. Press **Add an engine**.
3. Fill the form. Required fields have a star.
4. Paste the key. The models and voices are then listed. Choose them.
5. Press **Save engine**. **Test saved engine** speaks one sentence with no stand-in.
6. On the Engines page, press **Use** on the engine's row. Each row also has test, edit and remove.

| Form field | Meaning |
|---|---|
| Provider | The kind from the table above |
| Server | The preset (for `openai-compatible`) |
| Label, ID | A display name, and a short id (up to 32 characters) |
| Address | The service address. Each kind has a default |
| API key | *Saved key*, *Environment variable* or *No key* |
| Model, Voice | Chosen from the provider's lists when it has them |
| Send to the engine | See [Send modes](#send-modes) |
| More options | Timeout, Wait for audio, Characters per request, Read ahead, and Other options (JSON) |

The Engines page is shown when the runtime allows external engines.

### Add one with the CLI

Run these from Git Bash. Replace `<plugin folder>` with the plugin's folder.

```bash
S="bash <plugin folder>/bin/sonara"
$S engines add gemini --kind gemini          # add Gemini
$S engines key gemini                        # paste the key when asked (never an argument)
$S engines models gemini                     # the models Google offers now
$S engines voices gemini                     # and its voices
$S engines add gemini --kind gemini --model <model> --voice <voice> --replace
$S engines add gpu --preset kokoro-fastapi --url http://127.0.0.1:8880/v1 --no-key --voice <voice>
$S engines test gemini                       # one sentence, with no stand-in
$S engines use gemini                        # read with it; `use kokoro` goes back
$S engines list                              # every engine, its key and status
```

| Command | What it does |
|---|---|
| `engines list` | Your engines, their key and status |
| `engines add <id> [options]` | Add an engine. `--replace` changes an existing one |
| `engines key <id>` | Store the key (typed without echo or piped in). `--clear` deletes it |
| `engines models <id>` | The provider's models now |
| `engines voices <id>` | The provider's voices now |
| `engines test <id> [text]` | Speak one sentence with no stand-in |
| `engines use <id>` | Read with it from now on. `use kokoro` goes back |
| `engines remove <id>` | Remove it and its stored key. `--keep-key` keeps the key |

`engines add` options: `--kind`, `--preset`, `--url`, `--model`, `--voice`, `--label`, `--send-mode message|sentence`, `--key-env NAME`, `--no-key`, `--option KEY=VALUE`, `--replace`. The default kind is `openai-compatible`. `sonara engines --help` lists them all.

A key is never an argument. `engines add --key` is refused.

### A program on this PC

A `command` engine runs a program, so it is never added from the settings page or by another app over the protocol. Add it yourself with the CLI, or in `engines.json` in the Sonara home. The settings page then lists it so you can use, test or remove it.

```bash
$S engines add piper --kind command --option output=file --option 'argv=["C:/piper/piper.exe","-m","C:/piper/<model>.onnx","-f","{out}"]'
```

- The program must be an `.exe` that exists. `.bat` and `.cmd` are refused.
- The text read is never an argument. It goes on stdin, or in a file at `{in}` with `--option input=file`.
- Placeholders in arguments: `{voice}`, `{rate}`, `{speed}`, `{out}`, `{in}`.

### Which voice is used

While an added engine is in use, the voice you pick on the Speech page is the one you hear, in replies, previews and *Test*. Otherwise the engine's own voice (the one in its form) is used.

## Send modes

Every added engine has **Send to the engine** (`--send-mode` on the CLI).

| Mode | Meaning | Default for |
|---|---|---|
| **Full message in one request** (`message`) | What Sonara is about to read at once goes as one request. A reply read when done is one request, not one per sentence | Every cloud provider |
| **As it comes in** (`sentence`) | Each sentence is its own request, read as soon as it is made | A program, or a speech server on this PC |

Details for `message`:

- The text before a question is one request, and the question is its own short one. With *Reading: Immediately*, each paragraph is one request.
- Audio plays as it arrives (Gemini, ElevenLabs, OpenAI with `pcm`, Cartesia), so reading starts with the first audio.
- Holding Up reads the message again without a new request.
- A very long message is split only past the provider's limit (at paragraphs, then sentences). `--option chunk_chars=<n>` lowers it.
- Skipping, flushing or a new prompt cancels a message that is still arriving. A provider that already started may still bill that one request.

Kokoro and the Windows voices always read sentence by sentence.

## Gemini

Gemini's free tier limits requests per minute and per day, so Sonara sends a whole message per request by default.

| Topic | Behaviour |
|---|---|
| Request size | Up to 2000 characters. A longer message goes in as few parts as fit. Set **Characters per request** (More options) between 200 and 5000 |
| Per-minute limit reached | Kokoro reads, and Sonara waits as long as Google asks |
| Daily limit reached | Kokoro reads until Google's daily reset (midnight Pacific time) |
| Streaming | A sentence starts playing with its first audio. If none comes within 12 seconds, Kokoro reads that part. Change this with **Wait for audio** (More options, 1000 to 60000 ms) |
| Speed | Gemini has no speed setting. Away from the default rate, Sonara asks it to speak slowly or quickly. This is a direction, not an exact speed |

**Free-tier privacy.** On the free tier, Google may use the text it reads to improve its products. With billing on, it does not. See [PRIVACY.md](../../PRIVACY.md#external-voices-opt-in).

## Keys

| Where | How |
|---|---|
| Windows Credential Manager | The default. Stored as `sonara:<name>` |
| An environment variable you name | The name must end in `_API_KEY` or `_SPEECH_KEY`, or be `SPEECH_KEY`, or start with `SONARA_` |

- A key is never written to a file, shown again, or logged.
- A key goes only to the address it was entered for. If you point an engine at another address or region, enter its key again.
- A program's key goes only to that program (in `SONARA_ENGINE_KEY`).

## When things go wrong

| Situation | What Sonara does |
|---|---|
| The engine fails (no key, a refused key, no credit, no network, a busy or broken server) | Kokoro reads that sentence and says once why, for example "OpenAI cannot be reached. Reading with the built-in voice." |
| Two failures in a row | Sonara stops asking the engine for a while (30 seconds, doubling up to 300), then tries again |
| An answer stalls or breaks off midway | What was played stays. Kokoro reads the rest from the sentence it had reached (that sentence may be heard twice) |
| You mute | **Nothing is sent.** Kokoro reads instead, voice lists are not fetched, and a sentence on its way is cut. Only *Test* and a voice preview, which you ask for, still reach the engine |

The settings page and `sonara engines list` show the reason for a failure.

**Speed.** Sonara prepares the next sentences while one plays, so a cloud voice keeps up once it is reading. The first sentence waits for one round trip.

## Privacy

A cloud engine receives the text Sonara reads aloud, under that provider's terms. A server or program on your PC keeps it on your PC. See [PRIVACY.md](../../PRIVACY.md#external-voices-opt-in).

## See also

- [Settings](settings.md): the Speech page and every other setting
- [Summaries](summaries.md): a short recap instead of the whole reply
- [Troubleshooting](troubleshooting.md): no sound, wrong voice, doctor rows
- [Engine protocol reference](../protocol-v1-engines.md): every option of every kind
- [PRIVACY.md](../../PRIVACY.md): what is sent where
