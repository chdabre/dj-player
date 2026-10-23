# SoulSeek downloads (first downloader)

Status 2026-10-23: built. The crate is `crates/soulseek`, and the browse screen's search uses it (`crates/ui/src/screens/browse/soulseek.rs`). This is the first downloader integration; later ones should take the same shape.

## What the DJ does

1. SEARCH on the browse screen. While searching, the header shows two source chips: **LIBRARY** (the stick as the browser shows it: its folders, or our library's tracks) and **SOULSEEK**. The chosen source is kept for the next search, and switching takes the text typed so far along.
2. On SOULSEEK, typing does not search as it goes. The keyboard's DONE key reads **SEARCH**; it (or Enter on a real keyboard) sends the query, and the keyboard goes down so the hits show.
3. Hits come in for 20 s (`SEARCH_WINDOW`) and are listed **best quality first**. Each row shows the file name, the peer, the size, the quality (`FLAC 16/44.1`, `MP3 320`, `MP3 245 VBR`) and the length. The header shows `SEARCHING n`, then `n FILES`.
4. To pick a hit, tap it twice (select, then pick) or push the knob on it. Picking downloads it in the background into `SoulSeek Downloads/` at the stick's root. The row shows `QUEUED`, then the percentage, then `DOWNLOADED` (or `FAILED (why)`). Picking a downloaded hit loads it. BACK leaves the search.

## Decisions

- **Library:** [`soulseek-rs-lib`](https://crates.io/crates/soulseek-rs-lib) 19.1. It is MIT (compatible with GPL-3.0-or-later), its only dependency is `libc`, and it uses threads, not async. It needs Rust 1.91, so the workspace's `rust-version` went from 1.89 to 1.91.
- **On the UI's side, like `browser`, not behind the `Port`.** The downloads land on the same stick on the same host, and the engine has no part in them. The UI holds a `Soulseek` handle. Handlers push `Effect::SoulseekSearch` and `Effect::Download`, and read a `Snapshot` taken once per frame through `Cx`. *Open:* when the player becomes its own process, decide whether downloads move into it, so that they outlive a UI restart.
- **A thread of its own.** The network sits behind the `Network` trait (`Live` is the real one), so the worker is tested offline with a scripted network. The client logs in on the first search or download, and again after the session drops.
- **Ranking** (`quality::best_first`):
  1. lossless first;
  2. then the rate: a lossless file's PCM rate (from the sample rate and bit depth; CD quality where the peer doesn't say), or a lossy file's bitrate;
  3. then peers with a free upload slot;
  4. then peer speed.

  Only audio files are listed: FLAC, WAV, AIFF, ALAC, MP3, M4A/AAC, OGG and Opus.
- **Library records:** a downloaded file is an ordinary file on the stick. It gets its record the way folder-mode files do (`library.md`): on first load, or with SCAN. The browser's folder tree shows it under `SoulSeek Downloads`.
- **Account:** the `SOULSEEK_USERNAME` and `SOULSEEK_PASSWORD` environment variables. Without them (or without a stick) the SOULSEEK chip is still there, and the list says how to set them. The password is never printed (`Credentials`' `Debug`).

## Known gaps

- Nothing is shared back. The client listens for peers (it needs to, to receive files from peers behind a firewall) but shares no folders. Sharing is the network's etiquette, and some peers won't send to users who share nothing.
- There is no cancel. A failed download can be picked again; a `.part` file left behind is resumed.
- Two hits with the same file name overwrite each other in the downloads folder.
- The account is set by environment variables only, not on the device. A settings screen comes with the settings work.
- We log in with the library's default client version (major 176, minor 1, "unidentified"). Before release, reserve a minor version for this player with the library's maintainers (soulseek-rs issue 12).
- Downloads are not followed after the UI exits. The client stops with the UI, and an unfinished transfer leaves its `.part` file.
