# Sonara README, visual options menu

A rendered catalog so you can pick. Everything is a URL, so swapping a style means changing one word.
Pick by number and I'll wire it into the header.

---

## A. Banner shapes (capsule-render)

There are **15 shapes**. Here are the useful ones, all in Sonara purple. The `type=` word is what changes.

**A1. waving** (what the mockup uses)
![A1](https://capsule-render.vercel.app/api?type=waving&color=8A2BE2&height=150&section=header&text=Sonara&fontColor=ffffff&fontSize=50&desc=Eyes-free%20TTS%20for%20Claude%20Code&descSize=15&descAlignY=62)

**A2. wave**
![A2](https://capsule-render.vercel.app/api?type=wave&color=8A2BE2&height=150&section=header&text=Sonara&fontColor=ffffff&fontSize=50&desc=Eyes-free%20TTS%20for%20Claude%20Code&descSize=15&descAlignY=62)

**A3. rect** (clean flat bar)
![A3](https://capsule-render.vercel.app/api?type=rect&color=8A2BE2&height=150&section=header&text=Sonara&fontColor=ffffff&fontSize=50&desc=Eyes-free%20TTS%20for%20Claude%20Code&descSize=15&descAlignY=62)

**A4. soft**
![A4](https://capsule-render.vercel.app/api?type=soft&color=8A2BE2&height=150&section=header&text=Sonara&fontColor=ffffff&fontSize=50&desc=Eyes-free%20TTS%20for%20Claude%20Code&descSize=15&descAlignY=62)

**A5. slice**
![A5](https://capsule-render.vercel.app/api?type=slice&color=8A2BE2&height=150&section=header&text=Sonara&fontColor=ffffff&fontSize=50&desc=Eyes-free%20TTS%20for%20Claude%20Code&descSize=15&descAlignY=62)

**A6. cylinder**
![A6](https://capsule-render.vercel.app/api?type=cylinder&color=8A2BE2&height=150&section=header&text=Sonara&fontColor=ffffff&fontSize=50&desc=Eyes-free%20TTS%20for%20Claude%20Code&descSize=15&descAlignY=62)

**A7. egg**
![A7](https://capsule-render.vercel.app/api?type=egg&color=8A2BE2&height=150&section=header&text=Sonara&fontColor=ffffff&fontSize=50&desc=Eyes-free%20TTS%20for%20Claude%20Code&descSize=15&descAlignY=62)

**A8. blur**
![A8](https://capsule-render.vercel.app/api?type=blur&color=8A2BE2&height=150&section=header&text=Sonara&fontColor=ffffff&fontSize=50&desc=Eyes-free%20TTS%20for%20Claude%20Code&descSize=15&descAlignY=62)

**A9. speech**
![A9](https://capsule-render.vercel.app/api?type=speech&color=8A2BE2&height=150&section=header&text=Sonara&fontColor=ffffff&fontSize=50&desc=Eyes-free%20TTS%20for%20Claude%20Code&descSize=15&descAlignY=62)

**A10. venom**
![A10](https://capsule-render.vercel.app/api?type=venom&color=8A2BE2&height=150&section=header&text=Sonara&fontColor=ffffff&fontSize=50&desc=Eyes-free%20TTS%20for%20Claude%20Code&descSize=15&descAlignY=62)

---

## B. Same shape, different color / motion

The color is just as free as the shape. `color` takes a hex, a two-stop gradient, or a random preset. `animation` moves the text.

**B1. purple gradient** (`color=0:8A2BE2,100:1a0033`)
![B1](https://capsule-render.vercel.app/api?type=waving&color=0:8A2BE2,100:1a0033&height=150&section=header&text=Sonara&fontColor=ffffff&fontSize=50&desc=Eyes-free%20TTS%20for%20Claude%20Code&descSize=15&descAlignY=62)

**B2. teal** (`color=2AB7CA`)
![B2](https://capsule-render.vercel.app/api?type=waving&color=2AB7CA&height=150&section=header&text=Sonara&fontColor=ffffff&fontSize=50&desc=Eyes-free%20TTS%20for%20Claude%20Code&descSize=15&descAlignY=62)

**B3. slate** (`color=2b2d42`)
![B3](https://capsule-render.vercel.app/api?type=rect&color=2b2d42&height=150&section=header&text=Sonara&fontColor=ffffff&fontSize=50&desc=Eyes-free%20TTS%20for%20Claude%20Code&descSize=15&descAlignY=62)

**B4. animated text** (`animation=twinkling`, moves on GitHub)
![B4](https://capsule-render.vercel.app/api?type=waving&color=8A2BE2&height=150&section=header&text=Sonara&fontColor=ffffff&fontSize=50&desc=Eyes-free%20TTS%20for%20Claude%20Code&descSize=15&descAlignY=62&animation=twinkling)

---

## C. The tech-icon row (skill-icons)

This is the row you asked about (the standalone icons: Python, Windows, and the orange one is **PyTorch**).
It draws from **400+** icons. Options are the **icon list**, a **light/dark theme**, and **icons per line**.

**C1. Sonara's real stack, dark** (what the mockup uses: `i=python,windows,pytorch`)
![C1](https://skillicons.dev/icons?i=python,windows,pytorch)

**C2. same, light theme** (`&theme=light`)
![C2](https://skillicons.dev/icons?i=python,windows,pytorch&theme=light)

**C3. add the dev tooling** (`i=python,windows,pytorch,git,github,githubactions,vscode`)
![C3](https://skillicons.dev/icons?i=python,windows,pytorch,git,github,githubactions,vscode)

**C4. bigger, wrapped to 5 per line** (`&perline=5`)
![C4](https://skillicons.dev/icons?i=python,windows,pytorch,git,github,githubactions,vscode,markdown,powershell,bash&perline=5)

---

## D. Other ways to show the same tech (alternatives to skill-icons)

**D1. shields.io "for-the-badge" pills with logos** (bigger, label-style; logos from simple-icons)
![py](https://img.shields.io/badge/Python-3776AB?style=for-the-badge&logo=python&logoColor=white)
![win](https://img.shields.io/badge/Windows-0078D6?style=for-the-badge&logo=windows&logoColor=white)
![torch](https://img.shields.io/badge/PyTorch-EE4C2C?style=for-the-badge&logo=pytorch&logoColor=white)

**D2. flat shields with logos** (smaller, matches the top badge row)
![py](https://img.shields.io/badge/Python-3776AB?logo=python&logoColor=white)
![win](https://img.shields.io/badge/Windows-0078D6?logo=windows&logoColor=white)
![torch](https://img.shields.io/badge/PyTorch-EE4C2C?logo=pytorch&logoColor=white)

---

## E. Badge STYLE options (for the top shield row, for reference)

The top pills also have 5 styles. Same badge, different look:

![flat](https://img.shields.io/badge/style-flat-8A2BE2?style=flat)
![flat-square](https://img.shields.io/badge/style-flat--square-8A2BE2?style=flat-square)
![plastic](https://img.shields.io/badge/style-plastic-8A2BE2?style=plastic)
![for-the-badge](https://img.shields.io/badge/style-for--the--badge-8A2BE2?style=for-the-badge)
![social](https://img.shields.io/badge/style-social-8A2BE2?style=social)

---

**Not on this list but possible:** a fully custom banner image (a real designed graphic, not a shape),
which we could generate with the local image models or Codex gpt-image-2. Say the word if you want to explore that.
