# SoulSeek downloads (first downloader)

Status 2026-10-23: built. The crate is `crates/soulseek`, and the browse screen's search uses it (`crates/ui/src/screens/browse/soulseek.rs`). This is the first downloader integration; later ones should take the same shape.

## What the DJ does

1. SEARCH on the browse screen. While searching, the header shows two source chips: **LIBRARY** (the stick as the browser shows it: its folders, or our library's tracks) and **SOULSEEK**. The chosen source is kept for the next search, and switching takes the text typed so far along.
2. On SOULSEEK, typing does not search as it goes. The keyboard's DONE key reads **SEARCH**; it (or Enter on a real keyboard) sends the query, and the keyboard goes down so the hits show.
3. Hits come in for 20 s (`SEARCH_WINDOW`) and are listed **best quality first**. Each row shows the file name, the peer, the size, the quality (`FLAC 16/44.1`, `MP3 320`, `MP3 245 VBR`) and the length. The header shows `SEARCHING n`, then `n FILES`.
4. To pick a hit, tap it twice (select, then pick) or push the knob on it. Picking downloads it in the background into `SoulSeek Downloads/` at the stick's root. The row shows `QUEUED`, then the percentage, then `DOWNLOADED` (or `FAILED (why)`). Picking a downloaded hit loads it. BACK leaves the search.
5. While there are downloads, a **QUEUE** chip in the browse header shows how many are under way and how far they are together (`QUEUE 2 · 43 %`, or `QUEUE · 3` once all have finished). Tapping it shows the queue in place of the list, in the order the downloads were asked for. **CANCEL** stops the selected download and removes its partial file. On a finished download the chip reads **REMOVE**: it takes the download off the list and leaves the file. Picking a downloaded file loads it; picking a failed one tries again. BACK closes the queue.
6. **Account:** MENU on a playback screen (main, PERFORM, INFO) opens SETTINGS. Its SOULSEEK section has USERNAME, PASSWORD and FORGET ACCOUNT. A field is typed with the on-screen keyboard (or a real one). DONE keeps the account and logs in with it from the next search; BACK gives up. The password shows as dots except while it is typed. A status line says how the last login went.

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
- **Account:** kept on the player's own storage, never on a stick, because a stick travels between players. The file is `soulseek-account` in the config folder (`$DJPLAYER_CONFIG`, else `~/.config/djplayer`), readable by its owner only. The `SOULSEEK_USERNAME` and `SOULSEEK_PASSWORD` environment variables are the fallback when no file is there. Without an account the SOULSEEK chip is still there, and the list points to MENU. The password is never printed (`Credentials`' `Debug`). It is stored in plain text: the Pi has no keyring, and the file's permissions are what protect it.
- **SETTINGS is its own screen** (`screens/settings.rs`), reached with MENU from the playback screens. On the browse screen MENU stays the playlist menu. The screen has one section for now; later settings join it as more sections.
- **The queue is a view of the browse screen**, like the info panel, not a screen of its own. Its chip lives in the browse header, because downloads start there.
- **Screenshots** use `soulseek::Demo`, a stand-in network whose searches find a fixed spread of qualities and whose downloads stop at set states (done, 43 %, waiting, failed), with `--soulseek-demo`.

## Known gaps

- Nothing is shared back. The client listens for peers (it needs to, to receive files from peers behind a firewall) but shares no folders. Sharing is the network's etiquette, and some peers won't send to users who share nothing.
- A failed download can be picked again (in the results or the queue); a `.part` file left behind is resumed.
- The queue's progress shows only on the browse screen, not on the playback screens.
- Two hits with the same file name overwrite each other in the downloads folder.
- We log in with the library's default client version (major 176, minor 1, "unidentified"). Before release, reserve a minor version for this player with the library's maintainers (soulseek-rs issue 12).
- Downloads are not followed after the UI exits. The client stops with the UI, and an unfinished transfer leaves its `.part` file.
