# Sonari on Windows — Quick test (Round 1)

Hi! Thanks for helping test Sonari on Windows. This first round takes **~10–15 minutes** and does **not** need Claude Code or any account — we're just checking whether Windows can speak and play sounds through our code.

**How this works:** copy each grey block, paste it into **PowerShell**, press Enter. After each step there's a line saying what you should **see or hear** — just tell Nima what actually happened (and if anything turns **red**, copy that red text and send it to him). Don't worry about breaking anything; nothing here changes system settings.

> Open PowerShell: press the **Windows key**, type `powershell`, press **Enter**.

---

## Step 1 — Install Python (skip if you already have Python 3.10+)

Paste this:
```powershell
winget install -e --id Python.Python.3.12
```
Then **close PowerShell and open a new one** (so Python is on the PATH), and check:
```powershell
python --version
```
✅ **You should see:** something like `Python 3.12.x`.
*(If `winget` isn't found, instead download Python from https://www.python.org/downloads/ — get the 64-bit installer, and on the first screen TICK "Add python.exe to PATH".)*

---

## Step 2 — Get the Sonari code

```powershell
cd $HOME
git clone -b phase-3-windows https://github.com/nimkimi/sonari sonari
cd sonari
```
✅ **You should see:** it downloads into a `sonari` folder and you end up inside it.
*(If `git` isn't found: download https://github.com/nimkimi/sonari/archive/refs/heads/phase-3-windows.zip , unzip it, and `cd` into the unzipped folder instead.)*

---

## Step 3 — Install the Windows speech packages

```powershell
python -m pip install --upgrade pip
python -m pip install winrt-runtime winrt-Windows.Media.SpeechSynthesis winrt-Windows.Media.Playback winrt-Windows.Media.Core winrt-Windows.Storage.Streams
```
✅ **You should see:** a list of "Successfully installed winrt-…" lines.
❗ **If you see red errors here, STOP and send them to Nima** — this tells us whether the speech packages are even available on your Windows.

---

## Step 4 — THE BIG ONE: does Windows actually speak? 🔊

Turn your **volume up**, then paste this (it's one block):
```powershell
$env:PYTHONPATH = "$PWD\src"
python -c "from sonari.platform.windows.tts import WinTtsBackend; h = WinTtsBackend().run('Hello Nima. Sonari is now speaking on Windows.', None, 200); print('returncode:', h.wait(timeout=15))"
```
✅ **You should HEAR:** a voice say *"Hello Nima. Sonari is now speaking on Windows."* and see `returncode: 0`.
👉 **Tell Nima:** Did you hear the voice? Did it sound clear or robotic? What was the returncode? (And copy any red error text.)

---

## Step 5 — Which voices does your Windows have?

```powershell
python -c "from sonari.platform.windows.tts import WinTtsBackend; print([v.display_name for v in WinTtsBackend().list_voices()])"
```
✅ **You should see:** a list like `['Microsoft David', 'Microsoft Zira', ...]`.
👉 **Tell Nima:** the list of names you see.

---

## Step 6 — Do the notification sounds play? 🔊

```powershell
python -c "import time; from sonari.platform.windows.earcon import WinEarconBackend; b=WinEarconBackend(); d=b.default_earcons(); [(_:=b.play(p), time.sleep(0.5)) for p in d.values()]; print('played', list(d))"
```
✅ **You should HEAR:** 6 short distinct beeps/tones in a row, and see `played [...]`.
👉 **Tell Nima:** Did you hear about 6 beeps?

---

## Step 7 — Single-instance check (quick, silent)

```powershell
python -c "import tempfile,os; from sonari.platform import transport; p=os.path.join(tempfile.mkdtemp(),'s'); a=transport.acquire_singleton(p); b=transport.acquire_singleton(p); print('first:', a is not None, ' second_blocked:', b is None)"
```
✅ **You should see:** `first: True  second_blocked: True`.
👉 **Tell Nima:** the line you see (this checks two daemons can't run at once on Windows).

---

## That's it for Round 1!

Send Nima:
1. **Whether you heard the voice (Step 4)** and how it sounded.
2. **Whether you heard the 6 beeps (Step 6)**.
3. The voice list (Step 5) and the single-instance line (Step 7).
4. **Any red error text** from any step (copy-paste it).

If this round works, there's a short Round 2 (installing it as a background service + a real Claude Code test) — but Step 4 is the big question, so thank you! 🙏
