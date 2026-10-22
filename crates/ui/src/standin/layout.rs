//! Where the shell's controls are: its panels, the left column (time mode, quantize, REV,
//! track search, SEARCH, CUE, PLAY), the right column (BACK, TAG TRACK, the browse knob, F1,
//! F2, the tempo fader) and the bottom band (loops, pads, the jog and its settings).

use player_api::HOT_CUES;

use super::draw::Draw;
use super::{
    FADER, FADER_CENTER_Y, FADER_TRAVEL, JOG_CENTER, JOG_RADIUS, PLATE_RADIUS, Part, ShellView,
    Standin,
};
use crate::input::{Jog, SECONDS_PER_TURN};
use crate::kit::{Rect, Ui};
use crate::render::scene::{Align, Scene};
use crate::theme::Theme;

impl Standin {
    /// Lay out the shell into `scene`, in the theme's shell colours.
    pub fn build(&mut self, v: &ShellView, theme: &Theme, scene: &mut Scene) {
        let pal = &theme.shell;
        scene.clear();
        scene.background = pal.body;
        let mut d = Draw {
            ui: Ui::new(theme, scene, None),
            c: pal,
            held: self.held,
        };
        panels(&mut d);
        left_column(&mut d, v);
        right_column(&mut d);
        tempo_fader(&mut d, v);
        loops_and_pads(&mut d, v);
        jog(&mut d, v);
        jog_settings(&mut d, v);
        self.hits = d.ui.into_hits();
    }
}

/// Panels: left column, display, bottom band, right column; and the display's bezel (the
/// device screen is copied over the inside).
fn panels(d: &mut Draw) {
    let pal = d.c;
    for (x, y, w, h) in [
        (6.0, 6.0, 226.0, 938.0),
        (235.0, 6.0, 1041.0, 612.0),
        (235.0, 622.0, 1041.0, 322.0),
        (1280.0, 6.0, 226.0, 938.0),
    ] {
        d.ui.scene
            .rounded(x, y, w, h, 10.0, pal.panel, 1.0, pal.panel_edge);
    }
    d.ui.scene
        .rounded(236.0, 8.0, 1036.0, 608.0, 6.0, pal.black, 1.0, pal.edge);
}

/// The left column, centred on x = 121.
fn left_column(d: &mut Draw, v: &ShellView) {
    let pal = d.c;
    d.label("TIME MODE", 121.0, 24.0, pal.label, 15.0, Align::Center);
    d.label("– AUTO CUE", 121.0, 42.0, pal.sub, 12.0, Align::Center);
    d.round(Part::TimeMode, 105.0, 62.0, 32.0);
    d.key("t", 145.0, 69.0);
    d.label("QUANTIZE", 121.0, 114.0, pal.label, 15.0, Align::Center);
    d.round(Part::Quantize, 105.0, 136.0, 32.0);
    d.led(116.0, 147.0, if v.quantize { pal.amber } else { pal.edge });
    d.key("z", 145.0, 143.0);
    d.rule(31.0, 192.0, 180.0);
    d.label("DIRECTION", 121.0, 210.0, pal.label, 15.0, Align::Center);
    let rev_fill = if v.reverse { pal.red_lit } else { pal.red_fill };
    d.rect_button(
        Part::Rev,
        Rect::new(71.0, 232.0, 100.0, 40.0),
        "REV",
        rev_fill,
        pal.red_edge,
        pal.red_text,
    );
    d.key("r", 179.0, 242.0);
    d.label("TRACK SEARCH", 121.0, 296.0, pal.label, 15.0, Align::Center);
    d.round_label(Part::TrackBack, 66.0, 318.0, 50.0, "|◀◀");
    d.round_label(Part::TrackNext, 126.0, 318.0, 50.0, "▶▶|");
    d.label("SEARCH", 121.0, 392.0, pal.label, 15.0, Align::Center);
    d.round_label(Part::SearchBack, 66.0, 414.0, 50.0, "◀◀");
    d.round_label(Part::SearchFwd, 126.0, 414.0, 50.0, "▶▶");
    d.key("← →", 106.0, 474.0);
    d.rule(31.0, 520.0, 180.0);
    // CUE and PLAY, with their rings lit by state.
    let cue_ring = if v.playing { pal.amber_dim } else { pal.amber };
    d.big(Part::Cue, 69.0, 566.0, "CUE", cue_ring, pal.amber_text);
    d.key("c", 184.0, 608.0);
    d.label("PLAY / PAUSE", 121.0, 706.0, pal.label, 15.0, Align::Center);
    let play_ring = if v.playing { pal.green } else { pal.green_dim };
    d.big(Part::Play, 69.0, 734.0, "▶ ‖", play_ring, pal.green_text);
    d.key("space", 184.0, 776.0);
}

/// The right column's top: BACK, TAG TRACK, the browse knob with ▲ ▼, F1 and F2.
fn right_column(d: &mut Draw) {
    let pal = d.c;
    d.rect_button(
        Part::Back,
        Rect::new(1290.0, 40.0, 100.0, 42.0),
        "BACK",
        pal.button_top,
        pal.edge,
        pal.text,
    );
    d.rect_button(
        Part::TagTrack,
        Rect::new(1396.0, 40.0, 100.0, 42.0),
        "TAG TRACK",
        pal.button_top,
        pal.edge,
        pal.text,
    );
    d.key("esc", 1290.0, 88.0);
    d.key("x  ⇧x", 1396.0, 88.0);
    d.ui.scene.rounded(
        1312.0,
        110.0,
        152.0,
        186.0,
        14.0,
        pal.button_bottom,
        1.0,
        pal.edge,
    );
    d.label("▲", 1388.0, 116.0, pal.sub, 12.0, Align::Center);
    let knob_fill = if d.down(Part::Browse) {
        pal.button_down
    } else {
        pal.button_top
    };
    d.ui.scene.rounded(
        1332.0,
        146.0,
        112.0,
        112.0,
        56.0,
        knob_fill,
        2.0,
        pal.edge_light,
    );
    d.ui.scene
        .rounded(1358.0, 172.0, 60.0, 60.0, 30.0, pal.knob, 0.0, pal.knob);
    d.ui.hit(Part::Browse, Rect::new(1332.0, 146.0, 112.0, 112.0));
    d.ui.hit(Part::BrowseUp, Rect::new(1312.0, 110.0, 152.0, 36.0));
    d.ui.hit(Part::BrowseDown, Rect::new(1312.0, 258.0, 152.0, 38.0));
    d.label("▼", 1388.0, 268.0, pal.sub, 12.0, Align::Center);
    d.label(
        "BROWSE · TURN: SCROLL · PUSH: LOAD",
        1388.0,
        302.0,
        pal.sub,
        12.0,
        Align::Center,
    );
    d.key("↑↓ ⏎", 1334.0, 320.0);
    d.key("scroll", 1390.0, 320.0);
    d.rect_button(
        Part::F1,
        Rect::new(1290.0, 350.0, 100.0, 40.0),
        "F1",
        pal.button_top,
        pal.edge,
        pal.text,
    );
    d.rect_button(
        Part::F2,
        Rect::new(1396.0, 350.0, 100.0, 40.0),
        "F2",
        pal.button_top,
        pal.edge,
        pal.text,
    );
    d.rule(1290.0, 408.0, 206.0);
}

/// The tempo fader: its slot, scale, the cap at the deck's rate, and the rate.
fn tempo_fader(d: &mut Draw, v: &ShellView) {
    let pal = d.c;
    d.label("TEMPO", 1390.0, 418.0, pal.label, 15.0, Align::Center);
    let Rect {
        x: fx,
        y: fy,
        w: fw,
        h: fh,
    } = FADER;
    d.ui.scene
        .rounded(fx, fy, fw, fh, 6.0, pal.panel, 1.0, pal.edge);
    d.ui.scene.fill(1388.0, 456.0, 4.0, 458.0, pal.black);
    d.label("–", 1336.0, 452.0, pal.sub, 12.0, Align::Left);
    d.label("0", 1334.0, 678.0, pal.sub, 12.0, Align::Left);
    d.ui.scene.fill(1352.0, 685.0, 12.0, 2.0, pal.text);
    d.label("+", 1335.0, 902.0, pal.sub, 12.0, Align::Left);
    // The cap follows the deck's rate on this range (pitch bends move it too).
    let v_rate = ((v.rate - 1.0) / v.range.0).clamp(-1.0, 1.0) as f32;
    let cap_y = FADER_CENTER_Y + v_rate * FADER_TRAVEL - 18.0;
    let cap = if d.down(Part::Fader) {
        pal.button_down
    } else {
        pal.fader_cap
    };
    d.ui.scene
        .rounded(1362.0, cap_y, 56.0, 36.0, 4.0, cap, 1.0, pal.fader_cap_edge);
    d.ui.scene.fill(1362.0, cap_y + 17.0, 56.0, 2.0, pal.text);
    d.ui.hit(Part::Fader, FADER);
    d.key("+ / −", 1438.0, 666.0);
    d.key("0", 1438.0, 690.0);
    d.label(
        &format!("{:+.2} %", (v.rate - 1.0) * 100.0),
        1390.0,
        932.0,
        pal.sub,
        12.0,
        Align::Center,
    );
}

/// The bottom band's left: loop IN / OUT / RELOOP and the hot cue pads.
fn loops_and_pads(d: &mut Draw, v: &ShellView) {
    let pal = d.c;
    d.label("IN / 4BEAT", 282.0, 632.0, pal.sub, 12.0, Align::Center);
    d.label("OUT", 352.0, 632.0, pal.sub, 12.0, Align::Center);
    d.label("RELOOP / EXIT", 440.0, 632.0, pal.sub, 12.0, Align::Center);
    d.ui.scene.fill(310.0, 680.0, 14.0, 2.0, pal.edge_light);
    d.ui.scene.fill(380.0, 680.0, 40.0, 2.0, pal.edge_light);
    let loop_fill = if v.looping { pal.amber_text } else { pal.amber };
    d.amber(Part::LoopIn, 254.0, 652.0, 56.0, "IN", loop_fill);
    d.amber(Part::LoopOut, 324.0, 652.0, 56.0, "OUT", loop_fill);
    d.round(Part::Reloop, 420.0, 660.0, 40.0);
    d.key("i", 272.0, 716.0);
    d.key("o", 342.0, 716.0);
    d.key("l", 430.0, 716.0);
    d.key("b · [ ]", 478.0, 670.0);
    d.ui.scene.rounded(
        242.0,
        748.0,
        322.0,
        184.0,
        8.0,
        pal.pads_panel,
        1.0,
        pal.pads_panel_edge,
    );
    d.label(
        "HOT CUE A–H · OUR PADS",
        254.0,
        756.0,
        pal.sub,
        12.0,
        Align::Left,
    );
    d.key("1–8 · ⇧ del", 478.0, 754.0);
    for i in 0..8u8 {
        let (col, row) = (f32::from(i % 4), f32::from(i / 4));
        let (x, y) = (254.0 + col * 76.0, 780.0 + row * 74.0);
        let set = usize::from(i) < HOT_CUES && v.hot_cues & (1 << i) != 0;
        let c = pal.pads[usize::from(i)];
        let fill = if d.down(Part::Pad(i)) {
            pal.button_down
        } else if set {
            pal.pad_set
        } else {
            pal.key_fill
        };
        d.ui.scene
            .rounded(x, y, 68.0, 66.0, 6.0, fill, 1.0, pal.edge);
        if set {
            d.ui.scene
                .rounded(x, y, 68.0, 66.0, 6.0, c.alpha(0.05), 0.0, c);
            d.ui.scene.rounded(x, y, 68.0, 4.0, 2.0, c, 0.0, c);
        }
        let letter = ((b'A' + i) as char).to_string();
        d.label(&letter, x + 34.0, y + 21.0, pal.text, 20.0, Align::Center);
        d.ui.hit(Part::Pad(i), Rect::new(x, y, 68.0, 66.0));
    }
}

/// The jog: ring, top plate, centre display, and a marker that turns with the track.
fn jog(d: &mut Draw, v: &ShellView) {
    let pal = d.c;
    let (cx, cy) = JOG_CENTER;
    let ring = if matches!(v.jog, Some(Jog::Bend { .. })) {
        pal.jog_ring_active
    } else {
        pal.jog_ring
    };
    d.circle(cx, cy, JOG_RADIUS, ring, 2.0, pal.edge);
    let plate = if matches!(v.jog, Some(Jog::Scratch)) {
        pal.jog_plate_active
    } else {
        pal.jog_plate
    };
    d.circle(cx, cy, PLATE_RADIUS, plate, 2.0, pal.edge_light);
    d.circle(cx, cy, 46.0, pal.black, 3.0, pal.blue);
    let vinyl = if v.vinyl { "VINYL" } else { "CDJ" };
    d.label(vinyl, cx, cy - 9.0, pal.text, 13.0, Align::Center);
    let turn = (v.position / SECONDS_PER_TURN).fract() as f32 * std::f32::consts::TAU;
    let (mx, my) = (cx + 104.0 * turn.sin(), cy - 104.0 * turn.cos());
    d.circle(mx, my, 6.0, pal.jog_marker, 0.0, pal.black);
    d.label("– REV", 574.0, 918.0, pal.sub, 12.0, Align::Left);
    d.label("FWD +", 862.0, 918.0, pal.sub, 12.0, Align::Left);
}

/// Right of the jog: JOG MODE, the tempo range, KEYLOCK and the jog's help.
fn jog_settings(d: &mut Draw, v: &ShellView) {
    let pal = d.c;
    d.label("JOG MODE", 924.0, 658.0, pal.label, 15.0, Align::Left);
    let (jf, je, jt) = if v.vinyl {
        (pal.blue_fill, pal.blue, pal.blue_text)
    } else {
        (pal.button_top, pal.edge, pal.sub)
    };
    d.rect_button(
        Part::JogMode,
        Rect::new(1160.0, 646.0, 90.0, 44.0),
        "VINYL",
        jf,
        je,
        jt,
    );
    d.key("v", 1126.0, 658.0);
    d.label(
        &format!("TEMPO  {}", v.range.1),
        924.0,
        732.0,
        pal.label,
        15.0,
        Align::Left,
    );
    d.label(
        "±6 / ±10 / ±16 / WIDE",
        924.0,
        750.0,
        pal.sub,
        12.0,
        Align::Left,
    );
    d.round(Part::TempoRange, 1210.0, 728.0, 36.0);
    d.key("g", 1166.0, 737.0);
    d.label("KEYLOCK", 924.0, 812.0, pal.label, 15.0, Align::Left);
    d.round(Part::Keylock, 1212.0, 804.0, 32.0);
    d.led(1223.0, 815.0, if v.keylock { pal.amber } else { pal.edge });
    d.key("k", 1166.0, 810.0);
    d.rule(924.0, 880.0, 326.0);
    d.label("JOG", 924.0, 890.0, pal.label, 15.0, Align::Left);
    d.label(
        "TOP: SCRATCH · RING: PITCH BEND",
        964.0,
        892.0,
        pal.sub,
        12.0,
        Align::Left,
    );
    d.key("drag top", 924.0, 912.0);
    d.key("drag ring", 994.0, 912.0);
}
