# UI architecture (proposal, 2026-10-22)

How `crates/ui` is laid out so that new screens, dialogs and controls can be added without growing one file. It keeps what already works: immediate-mode drawing into a `Scene` once per frame, layouts that are pure functions of a view (tested for fit and overlap, screenshotted with `--screenshot`), and colours and sizes taken from the theme. What changes is where state lives, how input finds its handler, and how the drawing code is split up.

**Status:** steps 1–3 done: `kit/` (`Ui`, `HitBox`, `Rect`, the shared widgets, text styles in the theme), `render/` (one module per pipeline), and `app/engine.rs` + `app/track.rs` (`DeckSnapshot`, `Track`). The rest is a proposal.

## What hurts today

The crate has about 9,750 lines. `main.rs` (2,140 lines) holds an `App` that does almost everything:

- **One `App` does everything.** It handles winit events, parses arguments, runs the frame loop, takes screenshots and routes the stand-in. It also holds the browser and playlist controller (`act`, `modal_push`, `named`, `type_name`, `move_entry`, …) and builds the view models (`browse_view`, `info_view`, `info_screen_view`).
- **UI state is loose fields on `App`:** `screen`, `keyboard`, `side`, `modal`, `moving`, `shift`, `ask_scan`, `list_drag`, `wheel`, `perf`, `zoom`, `scrub`, `pressed`. Nothing says which screen a field belongs to. `load()` resets eight track fields one by one.
- **Focus is a chain of `if`s.** `key()` and `nav()` decide who gets the input by testing, in order: ask-scan, naming modal, tag key, search keyboard, info screen, browse keys, nav, deck keys. Each new dialog adds another test in more than one place.
- **One flat `Target` enum covers every screen** (45 variants), and `App::press` is one large `match` over it. Screen modules can't own their own touch targets.
- **There is no layering.** The `Scene` draws every rectangle, then the waveforms, the images and finally the text. An overlay therefore can't cover text: the list hides its rows while a modal is up, and the tests check that it does. Taps go to the first hit box found, so modality depends on the order the code pushed the boxes.
- **The engine is read in several places.** The atomics are read in `frame`, `shell_view`, `press`, `drag` and `seek_overview`, and the loop length in beats is computed twice (`App::loop_beats` and `perform::loop_beats`). Spike step "Fast state" (`docs/spikes/ui-shell.md`) moves the UI to its own process, reading a seqlock'd block and sending commands over a socket. With the reads spread out like this, that change touches the whole crate.
- **Layout is untyped.** Rectangles are `(f32, f32, f32, f32)` tuples. The frame's coordinates (rail at 6/100, sections at 106/915, the 822 divider) are repeated in `screen.rs` and `browse.rs`. `Ui::text` takes seven arguments. A chip that needs a taller touch area fixes up `hits.last_mut()` afterwards.
- **The renderer and the stand-in are each one big file.** `gfx.rs` (1,470 lines) holds four pipelines, the offscreen and screenshot path and text preparation. `standin.rs` keeps its own colour constants outside the theme.

## Shape

The data flows one way, once per frame:

```
 inputs ──► Router ──► layer handlers ──► Effects ──► Engine port (Player now, socket later)
 (touch, keys,           (screen +           │
  stand-in, RP2040)       overlay stack)     ▼
                                     UiState changes
 Engine port ──► DeckSnapshot ─┐
 Track (wave, tags) ───────────┼──► screens draw ──► Scene (layered) + HitBoxes ──► render (wgpu)
 UiState ──────────────────────┘
```

- **Inputs become one vocabulary** (`Input`) before any UI code sees them.
- **Handlers change UI state and return `Effect`s.** They never call the `Player`. `perform::press` already works this way, returning a `DeckCommand`. That makes every handler testable without audio or a window.
- **Drawing is a pure function** of the snapshot, the loaded track and the UI state, as it is now.

The pattern is the Elm architecture (model, view, update), adapted to immediate-mode drawing:

| Elm | Here |
| --- | --- |
| Model | `UiState`, plus read-only snapshots of state owned elsewhere: `DeckSnapshot`, the `Browser`, the library's published listings |
| Msg | `Input` and `Target` |
| update | `Layer::tap` / `control` / `text` / `gesture`: change the layer's own state, return `Effect`s |
| Cmd | `Effect`, carried out only by `App` |
| Subscriptions | the engine and library threads, read once per frame |
| view | the screens' draw functions: state → `Scene` + hit boxes, every frame |

- **Updates mutate in place.** Handlers take `&mut self` rather than returning a new model, and each handler reaches only its own layer's state and what its context lends it.
- **`update` is split by layer.** The router picks the layer, and each layer handles its own targets. There is no single `update` that grows with every screen.
- **The UI never owns deck or library truth.** It reads snapshots and asks for changes through `Effect`s, so no UI copy of "playing" can drift out of sync with the engine.
- **One rule:** state is written only in handlers and read only in draw code. Drawing never changes state, and handlers never draw. Caches such as the overview and the text cache are the exception, because nothing outside them can see them.
- **Not MVVM:** there is no data binding or change notification, because everything is redrawn every frame. The per-frame view structs (`BrowseView`) are plain projections, not view models with a lifetime.

## Modules

```
crates/ui/src/
  main.rs              args → App → event loop; nothing else (~100 lines)
  args.rs              Args, parse_args, USAGE
  app/
    mod.rs             App: owns UiState, Track, Engine, Gfx; frame() = poll → snapshot → draw → render
    platform.rs        winit ApplicationHandler; WindowEvent → Input (design coords, surface)
    keymap.rs          dev keyboard → Input (the one table of key bindings; hot_cue_key lives here)
    engine.rs          Engine port + DeckSnapshot (the only place that reads DeckState atomics)
    effects.rs         Effect enum and its execution against the port
    track.rs           Track: title, tags, art, wave entries, gain, overview cache, playhead clock
    capture.rs         --screenshot readiness and PNG writing, --run-for
  input.rs             Input, Control, Gesture: the device-independent input vocabulary
  router.rs            focus: hit-test → layer; control/text → top layer first; pressed + gestures
  kit/                 the widget toolkit: no deck, browser or library types in here
    mod.rs             Ui<'_>: theme, scene, hits, pressed; layer push/pop
    rect.rs            Rect { x, y, w, h } and splitting helpers
    text.rs            text styles from the theme; ui.text(s, at, style)
    button.rs chip.rs rule.rs meter.rs scroll.rs list.rs modal_list.rs keyboard.rs
  screens/
    mod.rs             Screens (state of each), ScreenKind, Target (nested), Layer trait
    chrome.rs          the frame every screen shares: rail, tab row, sections, status band, bottom strip
    deck.rs            deck displays shared by screens: zoom waveform, overview, phase meters, cue column
    main.rs            1 · Normal playback
    perform.rs         2 · Performance (PerfState, pads, beat loops)
    browse/
      mod.rs           BrowseState (keyboard, side, shift, list drag, wheel) + handlers
      view.rs          BrowseView from the Browser (moved out of App::browse_view)
      draw.rs          header, list, jacket, side panel
    info.rs            INFO screen (its own view, no longer a BrowseView with `info` set)
  overlays/
    mod.rs             Overlay stack
    menu.rs            MENU / ADD TO PLAYLIST list modals (from playlists.rs)
    naming.rs          name entry with the keyboard
    confirm.rs         yes/no (DELETE …?, REMOVE ALL?)
    ask_scan.rs        "scan this stick?"
    moving.rs          MOVE: the knob moves an entry
  render/              was gfx.rs + scene.rs
    mod.rs             Gfx: surface, resize, to_design, render, screenshot
    scene.rs           Scene with layers
    rects.rs waves.rs images.rs text.rs offscreen.rs
  theme.rs  waves.rs  clock.rs  stats.rs
  standin/             (feature "standin") the hardware stand-in; emits Input, uses kit + theme.shell
```

Rules that keep it from drifting back:

- **`kit` is generic.** It knows the theme, the scene and hit boxes. It never sees a `DeckSnapshot`, a `Browser` or a `library` type, so it can be reused by every screen and by the stand-in.
- **A screen module owns its own state, targets, view model, drawing and handlers.** A new screen is a new module plus one arm in `ScreenKind`, with no edits to `main.rs` or `app/`.
- **Only `app/engine.rs` touches `DeckState` or `Player`,** and only `app/effects.rs` sends anything to it.
- **No literals in screen code** (existing rule). Layout coordinates are named `Rect` constants at the top of each screen module. Shared frame geometry lives in `chrome.rs` only.

## State

```rust
/// Everything the UI owns. Engine state is not here: it is read fresh each frame.
pub struct UiState {
    pub screen: ScreenKind,
    pub screens: Screens,        // each screen's own state, kept while another screen is up
    pub overlays: Vec<Box<dyn Overlay>>,
    pub pointer: Pointer,        // pressed target, active gesture (scrub, list drag, overview seek)
    pub debug: bool,
    pub wave_settings: WaveSettings,
}

pub struct Screens {
    pub main: MainState,         // zoom step
    pub perform: PerfState,      // pad mode, banks, delete armed
    pub browse: BrowseState,     // keyboard up, side panel, shift, wheel remainder
}
```

- **Screen state is kept while another screen is up.** Switching to MAIN and back leaves the pad bank or the browse panel where it was, as now.
- **`Track` (in `app/track.rs`) is everything derived from the loaded file:** title, tags, art, wave entries, `wave_complete`, track gain, overview cache and playhead clock. Loading a track replaces it with `Track::new(path)` in one step, so no field can be left over from the last track.
- **`DeckSnapshot` is a plain `Copy`-ish struct** read once at the start of the frame: position, rate, flags, cue, loop, hot cues and so on, plus `loop_beats()` and `zoom_span()` as methods. Handlers and drawing both read it. `View`, `ShellView` and the scattered atomics reads go away. In the two-process design the snapshot is filled from the seqlock block, and nothing above `app/engine.rs` changes.
- **The `Browser` stays the browse model.** It is owned by `App` and lent to handlers through the context, because TAG TRACK and loading reach it from any screen.

## Input and focus

```rust
pub enum Input {
    /// A finger or mouse on the device screen, in design pixels.
    Pointer(PointerPhase, f32, f32),
    /// A physical control: from the stand-in, the dev keyboard, or (on the device) the RP2040.
    Control(Control, Press),
    /// A character typed on a real keyboard (search, naming).
    Text(char),
}

pub enum Control {
    BrowseTurn(i32), BrowsePush, Back, TagTrack, Info, Menu,
    Play, Cue, Pad(u8), LoopIn, LoopOut, Reloop, Jog(..), Fader(f64), ...
}
```

- **One vocabulary for every source.** The stand-in, `keymap.rs` and later the control-surface reader all produce `Input`. Today the stand-in emits both `DeckCommand`s and `Nav`s, and the keyboard has its own `match`. After this change "what does BACK do" is answered in one place.
- **Deck controls are global.** Play, cue, the jog, the tempo fader and the loop buttons go to the deck whatever is on screen, as on the hardware. The router maps them straight to `Effect::Deck` before any layer sees them.
- **Everything else goes to the focus stack, top first:** the overlays from the top down, then the current screen, then the global fallbacks (BROWSE opens the browser, INFO toggles, TAG TRACK tags). Each layer returns `Flow::Handled` or `Flow::Pass`. The priority chain in `key()` and `nav()` becomes the order of the stack.
- **Touch goes to the top layer that drew a hit there.** Hit boxes are recorded per scene layer. A modal layer (`Overlay::is_modal`) swallows taps outside its own boxes, so nothing underneath gets a tap that passes through the modal.
- **Gestures are owned by the router**, not by fields on `App`. A hit box has a `HitKind`: `Tap`, `Drag` (scrub, overview seek, list scroll with the 8 px slop) or `Hold`. The router keeps the gesture in progress and sends `Gesture::Begin/Move/End` to the layer that owns the target.

```rust
pub trait Layer {
    fn draw(&self, cx: &DrawCx, ui: &mut Ui);
    fn tap(&mut self, target: Target, cx: &mut Cx) -> Flow { Flow::Pass }
    fn gesture(&mut self, target: Target, g: Gesture, cx: &mut Cx) -> Flow { Flow::Pass }
    fn control(&mut self, c: Control, cx: &mut Cx) -> Flow { Flow::Pass }
    fn text(&mut self, ch: char, cx: &mut Cx) -> Flow { Flow::Pass }
}

/// What a handler may touch.
pub struct Cx<'a> {
    pub deck: &'a DeckSnapshot,
    pub track: &'a Track,
    pub browser: Option<&'a mut Browser>,
    pub library: LibraryView<'a>,    // listing, playlists, scan / batch progress
    pub effects: &'a mut Vec<Effect>,
    pub nav: &'a mut NavRequests,    // show a screen, push / pop / replace an overlay
}

pub enum Flow { Handled, Pass }
```

- **Screens are a closed set,** stored as concrete fields and dispatched with a `match` on `ScreenKind`. Each implements `Layer`.
- **Overlays are `Box<dyn Overlay>` on a stack** (`Overlay: Layer` plus `is_modal`). A handful are drawn per frame, so dynamic dispatch costs nothing that matters. Today's `Modal` enum splits into one overlay per kind: menu, pick, naming, confirm, ask-scan and moving. Each owns its cursor and its text and returns `Effect`s on push.
- **`Target` nests by owner:** `Target::Chrome(ChromeTarget)`, `Target::Main(MainTarget)`, `Target::Perform(PerfTarget)`, `Target::Browse(BrowseTarget)`, `Target::Key(u8)`, `Target::Overlay(OverlayTarget)`. Each module defines its own enum, so adding a pad doesn't touch the others. The layout tests (no overlap, on the panel) keep working on the outer enum.

```rust
pub enum Effect {
    Deck(DeckCommand),
    Load(PathBuf),
    Library(Edit),
    AddFiles(PlaylistId, Vec<PathBuf>),
    NewPlaylist { id, parent, name, files },
    Scan, AnalyzeAll(Vec<PathBuf>), StopAnalyzeAll,
    ToggleFullscreen, Quit,
}
```

## Drawing

- **Layered scene.** `Scene` holds a few fixed layers (`Base`, `Overlay`, `Popup`). Each has its own rects, waves, images and texts, and the renderer draws them in that order. Within a layer the order stays as now: rects and waves, then images, then text. Glyphon needs one `TextRenderer` per layer, three at most. `ui.layer(Layer::Overlay, |ui| …)` routes drawing and hit boxes to that layer. Once this exists the list no longer hides its rows under a modal.
- **`Rect` replaces the tuples.** Its helpers cover what the layouts already do by hand: `inset`, `split_x(at)`, `columns(n, gap)` (pads, keyboard keys, beat loops), `rows(n, h)` (list) and `contains`. Layout stays absolute on the fixed 1024×600 canvas. Taffy is still not needed, as the spike found.
- **Text styles come from the theme.** `TextStyle { size, weight, color, line_height }` tokens (`t.text.label`, `t.text.button`, `t.text.readout`, `t.text.row`, …) replace the seven-argument `Ui::text`. `ui.text(s, at, &t.text.label)` with an `Align` in `at`, and `.max_width(w)` as a builder where needed.
- **Widgets take a draw rectangle and a hit rectangle,** so the header chips' taller finger area is an argument (`Button::new(rect).hit(taller)`) rather than a fix-up of the last hit box.
- **Chrome is composed, not copied.** `chrome::frame(ui, Chrome { lit: Source::Usb, tab: None, bottom: Bottom::Status })` draws the rail, tab row, sections and lower bands. Main, performance, browse and info call it with their own options. The browse keyboard is a `Bottom::Keyboard` option.
- **The stand-in uses `kit` and a `theme.shell` palette.** It becomes a second surface drawn with the same widgets. Its colour constants move into the theme, so no screen code has colour literals.

## Testing

- **Handlers:** `Layer` methods tested with a fake `DeckSnapshot` and `Browser` (fixture folder), asserting the `Effect`s and state changes. `perform.rs` and `playlists.rs` already do this. The `App` methods that can't be tested now move into this form.
- **Layout:** one table-driven test builds every screen × overlay × bottom combination and checks that everything fits the 1024×600 panel, that targets don't overlap within a layer, and that a modal layer's targets cover what they block. It replaces the per-screen copies in `screen.rs`.
- **Router:** focus order (naming swallows text, ask-scan swallows BACK, deck controls bypass overlays) tested on a stack of stub layers.
- **Screenshots:** `--screenshot` stays the visual check; the readiness rules move into `app/capture.rs`.

## Migration

Each step is one commit and passes `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and `cargo test` on its own. The app behaves the same after each step.

1. **`Rect` and text styles.** *(Done.)* Add `kit/rect.rs` and the theme's `TextStyle`s; move the `Ui` helpers from `screen.rs` into `kit/`. Change call sites mechanically. `Ui` and `HitBox` are generic over the target type; `screen.rs` aliases them to `Target` until step 5. The scene's own primitive is now `Quad`. `scene.fill` / `rounded` keep loose coordinates: the scene is the renderer's input, and computed coordinates read better there. The stand-in keeps its tuples until step 8.
2. **Split the renderer.** *(Done.)* `gfx.rs` + `scene.rs` → `render/`, with no change in behaviour: `rects.rs`, `waves.rs`, `images.rs` and `text.rs` each own their pipeline, buffers and upload; `offscreen.rs` holds the stand-in's device-screen texture and screenshot readback; `mod.rs` keeps `Gfx` (surface, frame composition) and one `quad_pipeline` helper for all four pipelines.
3. **`DeckSnapshot` and `Track`.** *(Done.)* Read the atomics once per frame in `app/engine.rs`; `View` holds `&DeckSnapshot` instead of copying its fields, and input handlers act on the snapshot the last frame showed (at most a frame old, and what the user saw). `load()` replaces the `Track` in one step. `loop_beats` and `zoom_span` are snapshot methods, once each. (Our type is `Track`, not `LoadedTrack`, which `engined` already has.)
4. **`Effect`.** Handlers push effects; `App` executes them. Then move `act`, `modal_push`, `named` and `type_name` out of `App` into `playlists.rs` as functions that return effects.
5. **`Input`, `keymap.rs` and the router.** The stand-in emits `Input`; `key()` becomes a table. Add the `Layer` trait with screens as layers, and route touch through nested `Target`s.
6. **Overlays.** `Modal`, `ask_scan`, `moving` and naming become overlays on the stack, and the `if` chains go. The layered scene lands here, and with it the rows under a modal no longer need hiding.
7. **Screens into `screens/`, chrome extracted.** `screen.rs` splits into `chrome.rs`, `deck.rs` and `main.rs`. `browse.rs` and its view-building from `main.rs` become `screens/browse/`, and INFO becomes its own screen.
8. **Stand-in onto `kit` and `theme.shell`.**

After step 5 `main.rs` should be under 200 lines. After step 7 no file in `screens/` should be over about 600.

## Not changing

- **Immediate-mode, rebuilt every frame.** It measured fine (`docs/spikes/ui-shell.md`), and the text cache already avoids re-shaping. A retained tree would add invalidation bugs without a measured need.
- **Hand layout on the fixed canvas.**
- **The renderer's pipelines,** the waveform shader path and the overview cache.
- **raw wgpu + glyphon on winit** (decided 2026-10-21).

## Libraries considered (2026-10-22)

We checked whether a library could take over part of this work, and decided to stay custom.

- **egui** (0.37, already on wgpu 30) is the only framework that integrates cleanly: our pipeline would draw through `CallbackTrait::paint`. But what it adds is desktop widgets, its own text system and Tab-order focus. In practice we'd use it as a painter, which is what `Scene` already is.
- **Slint** (1.18, GPL-3.0 option) is the one real alternative: it can share our wgpu device and has a LinuxKMS backend with touch. Against it:
  - its wgpu 30 integration is marked unstable;
  - the LinuxKMS backend is still experimental;
  - it would own the event loop and frame pacing, the same reason GPUI was rejected;
  - moving to it would mean rewriting about 10k lines.

  Look at it again only if the Pi numbers force a different renderer.
- **No** to the others:
  - iced owns the loop and its wgpu 30 support is only on main;
  - Vello's compute renderer is a poor fit for the VideoCore VII, and Masonry/Xilem are pre-1.0;
  - Freya (Skia) and makepad bring their own renderers;
  - floem is stale;
  - kas has its own theme model.
- **Pieces:** keep glyphon for text. Use our own `Rect` helpers rather than Taffy, because nothing reflows. Add `lyon` only if a shape appears that the SDF rectangles can't draw (the jog ring, arcs). No library models focus driven by the encoder, so the router is ours.

## Open questions

- **`Layer` trait or enum for overlays?** A trait object keeps each overlay self-contained. A closed enum is easier to snapshot for tests and `--open`/`--menu` style CLI setup. The proposal uses a trait. Switch to an enum if pattern-matching on the overlay stack starts happening outside the router.
- **Where `Control` is defined.** The RP2040 protocol will need the same control list. If it is defined in a small shared crate (`surface`?), the firmware's host-side reader and the UI agree on one enum.
