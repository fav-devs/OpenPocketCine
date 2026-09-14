//! The scopes, read from the decoded picture on the CPU.
//!
//! The phones plot on the GPU; the desktop samples a few hundred columns of the
//! picture between frames and rasterises each plate itself. What is *drawn* is the
//! phones' plate: the same axis (`ScopeDisplayScale`'s level table, asked of the core
//! per colour mode and ISO), the same 0 / 100 / 18% grey guides, the same additive
//! trace palette, and the traffic lights and ND chip read by the core from the
//! histograms. Without the core linked a plain 0…255 axis stands in.

use opc_decode::OwnedPicture;

pub const HIST: usize = 256;
/// Waveform columns across the picture, and rows: one per native code.
pub const WAVE_COLS: usize = 256;
pub const WAVE_ROWS: usize = 256;
/// The vectorscope's bins per axis; U and V are halved into it.
pub const VECTOR_N: usize = 128;

/// Plate sizes, as the phones size them.
pub const WAVEFORM_SIZE: (u32, u32) = (250, 153);
pub const PARADE_SIZE: (u32, u32) = (250, 153);
pub const HISTOGRAM_SIZE: (u32, u32) = (250, 77);
pub const VECTORSCOPE_SIZE: (u32, u32) = (190, 190);
pub const LIGHTS_SIZE: (u32, u32) = (74, 168);

/// One frame's worth of counts.
#[derive(Debug, Clone)]
pub struct ScopeSamples {
    pub luma: [u32; HIST],
    pub red: [u32; HIST],
    pub green: [u32; HIST],
    pub blue: [u32; HIST],
    /// `WAVE_COLS × WAVE_ROWS`, row = native code.
    pub wave_luma: Vec<u16>,
    pub wave_red: Vec<u16>,
    pub wave_green: Vec<u16>,
    pub wave_blue: Vec<u16>,
    /// `VECTOR_N × VECTOR_N`, U across and V down, both halved.
    pub vector: Vec<u16>,
    pub pixels: u32,
}

impl Default for ScopeSamples {
    fn default() -> Self {
        Self {
            luma: [0; HIST],
            red: [0; HIST],
            green: [0; HIST],
            blue: [0; HIST],
            wave_luma: vec![0; WAVE_COLS * WAVE_ROWS],
            wave_red: vec![0; WAVE_COLS * WAVE_ROWS],
            wave_green: vec![0; WAVE_COLS * WAVE_ROWS],
            wave_blue: vec![0; WAVE_COLS * WAVE_ROWS],
            vector: vec![0; VECTOR_N * VECTOR_N],
            pixels: 0,
        }
    }
}

/// Limited-range BT.709 to 8-bit RGB, the way the feed pass converts the planes.
fn rgb(y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    let y = (f32::from(y) - 16.0) * (255.0 / 219.0);
    let u = (f32::from(u) - 128.0) * (255.0 / 224.0);
    let v = (f32::from(v) - 128.0) * (255.0 / 224.0);
    let r = y + 1.5748 * v;
    let g = y - 0.1873 * u - 0.4681 * v;
    let b = y + 1.8556 * u;
    (
        r.round().clamp(0.0, 255.0) as u8,
        g.round().clamp(0.0, 255.0) as u8,
        b.round().clamp(0.0, 255.0) as u8,
    )
}

impl ScopeSamples {
    /// Reads a picture, stepping so that no more than about 320 × 180 pixels are
    /// looked at: enough for the plates, cheap enough for every frame.
    pub fn read(picture: &OwnedPicture) -> Self {
        let view = picture.picture();
        let mut out = Self::default();
        if view.width == 0 || view.height == 0 {
            return out;
        }
        let step_x = (view.width / 320).max(1) as usize;
        let step_y = (view.height / 180).max(1) as usize;
        let (chroma_w, _) = view.chroma_size();
        let width = view.width as usize;
        for y in (0..view.height as usize).step_by(step_y) {
            let luma_row = &view.luma[y * view.luma_stride..];
            let chroma_row = (y / 2) * view.chroma_stride;
            for x in (0..width).step_by(step_x) {
                let code = luma_row[x];
                let cx = (x / 2).min(chroma_w as usize - 1);
                let u = view.chroma_blue[chroma_row + cx];
                let v = view.chroma_red[chroma_row + cx];
                let (r, g, b) = rgb(code, u, v);
                out.luma[code as usize] += 1;
                out.red[r as usize] += 1;
                out.green[g as usize] += 1;
                out.blue[b as usize] += 1;
                let column = x * WAVE_COLS / width;
                out.wave_luma[code as usize * WAVE_COLS + column] += 1;
                out.wave_red[r as usize * WAVE_COLS + column] += 1;
                out.wave_green[g as usize * WAVE_COLS + column] += 1;
                out.wave_blue[b as usize * WAVE_COLS + column] += 1;
                out.vector[(v as usize / 2) * VECTOR_N + u as usize / 2] += 1;
                out.pixels += 1;
            }
        }
        out
    }
}

/// Where each code plots and where 18% grey sits, from the core.
#[derive(Debug, Clone, PartialEq)]
pub struct ScopeScale {
    /// Plot level (0…1 of plot height, 0.05 crush to 0.95 clip) per native code.
    pub level: [f32; HIST],
    /// 18% grey on the 0…100 scale.
    pub grey_ire: f32,
}

pub const CRUSH_LEVEL: f32 = 0.05;
pub const CLIP_LEVEL: f32 = 0.95;

impl Default for ScopeScale {
    /// A plain 0…255 axis between the crush and clip lines, with 18% grey at Rec.709's
    /// paper IRE. The stand-in for when the core is not linked.
    fn default() -> Self {
        let mut level = [0.0_f32; HIST];
        for (code, out) in level.iter_mut().enumerate() {
            *out = CRUSH_LEVEL + code as f32 / 255.0 * (CLIP_LEVEL - CRUSH_LEVEL);
        }
        Self {
            level,
            grey_ire: 41.0,
        }
    }
}

impl ScopeScale {
    /// The core's table for this colour mode and ISO.
    #[cfg(opc_core_linked)]
    pub fn from_core(color_mode: i32, iso: i32) -> Self {
        let mut level = [0.0_f32; HIST];
        // Safety: `level` has the 256 slots the core writes.
        let count = unsafe {
            opc_core_sys::opc_scope_level_table(color_mode, iso, level.as_mut_ptr(), level.len())
        };
        if count != HIST as i32 {
            return Self::default();
        }
        // Safety: plain values in.
        let grey_ire = unsafe { opc_core_sys::opc_scope_grey_ire(color_mode, iso) } as f32;
        Self { level, grey_ire }
    }

    #[cfg(not(opc_core_linked))]
    pub fn from_core(_color_mode: i32, _iso: i32) -> Self {
        Self::default()
    }

    /// Plot level of a 0…100 scale value.
    pub fn level_of_ire(ire: f32) -> f32 {
        CRUSH_LEVEL + ire / 100.0 * (CLIP_LEVEL - CRUSH_LEVEL)
    }
}

/// One channel of the traffic lights.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Light {
    pub clip: bool,
    pub crush: bool,
    /// Where the channel's median sits, 0…1; 0.5 is balanced.
    pub level: f32,
}

/// The three lamps.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LightsReading {
    pub red: Light,
    pub green: Light,
    pub blue: Light,
}

impl LightsReading {
    /// The core's reading of the histograms; `previous` keeps a lamp lit until the
    /// energy halves. Without the core, dark.
    #[cfg(opc_core_linked)]
    pub fn from_core(
        samples: &ScopeSamples,
        color_mode: i32,
        iso: i32,
        threshold: f64,
        previous: Option<&LightsReading>,
    ) -> Self {
        let to_i32 =
            |bins: &[u32; HIST]| bins.map(|count| i32::try_from(count).unwrap_or(i32::MAX));
        let red = to_i32(&samples.red);
        let green = to_i32(&samples.green);
        let blue = to_i32(&samples.blue);
        let luma = to_i32(&samples.luma);
        let last = previous.map(|reading| reading.floats());
        let mut out = [0.0_f32; 9];
        // Safety: every pointer is to a 256- or 9-slot array that outlives the call.
        let status = unsafe {
            opc_core_sys::opc_scope_traffic_lights(
                red.as_ptr(),
                green.as_ptr(),
                blue.as_ptr(),
                luma.as_ptr(),
                color_mode,
                iso,
                threshold,
                last.as_ref()
                    .map_or(std::ptr::null(), |floats| floats.as_ptr()),
                out.as_mut_ptr(),
            )
        };
        if status != opc_core_sys::OPC_RELAY_OK {
            return Self::default();
        }
        let light = |at: usize| Light {
            clip: out[at] > 0.5,
            crush: out[at + 1] > 0.5,
            level: out[at + 2],
        };
        Self {
            red: light(0),
            green: light(3),
            blue: light(6),
        }
    }

    #[cfg(not(opc_core_linked))]
    pub fn from_core(
        _samples: &ScopeSamples,
        _color_mode: i32,
        _iso: i32,
        _threshold: f64,
        _previous: Option<&LightsReading>,
    ) -> Self {
        Self::default()
    }

    #[cfg(opc_core_linked)]
    fn floats(&self) -> [f32; 9] {
        let mut out = [0.0_f32; 9];
        for (index, light) in [self.red, self.green, self.blue].into_iter().enumerate() {
            out[index * 3] = f32::from(u8::from(light.clip));
            out[index * 3 + 1] = f32::from(u8::from(light.crush));
            out[index * 3 + 2] = light.level;
        }
        out
    }
}

/// The ND chip's reading.
#[derive(Debug, Clone, PartialEq)]
pub struct NdReading {
    /// Stops over 18% grey; positive is hot.
    pub picture_stops: f64,
    /// The glass to reach for, 0 for none.
    pub nd_stops: i32,
    /// "ND32" or "—".
    pub nd_label: String,
    /// "+2.3", "−1.0" or "0.0".
    pub stops_label: String,
}

impl NdReading {
    /// How the chip reads it: stops, a filter factor, or optical density.
    pub fn text(&self, notation: NdNotation) -> String {
        match notation {
            NdNotation::Stops => self.stops_label.clone(),
            NdNotation::Factor => self.nd_label.clone(),
            NdNotation::Density => {
                if self.nd_stops >= 1 {
                    format!("ND {:.1}", f64::from(self.nd_stops) * 0.3)
                } else {
                    "—".to_string()
                }
            }
        }
    }

    #[cfg(opc_core_linked)]
    pub fn from_core(samples: &ScopeSamples, color_mode: i32, iso: i32) -> Option<Self> {
        let luma = samples
            .luma
            .map(|count| i32::try_from(count).unwrap_or(i32::MAX));
        // Safety: probing with a null destination only reports the size needed.
        let needed = unsafe {
            opc_core_sys::opc_scope_nd(color_mode, iso, luma.as_ptr(), std::ptr::null_mut(), 0)
        };
        if needed <= 0 {
            return None;
        }
        let mut bytes = vec![0u8; needed as usize];
        // Safety: `bytes` has exactly the capacity the core asked for.
        let written = unsafe {
            opc_core_sys::opc_scope_nd(
                color_mode,
                iso,
                luma.as_ptr(),
                bytes.as_mut_ptr(),
                bytes.len(),
            )
        };
        if written != needed {
            return None;
        }
        Self::parse(&String::from_utf8_lossy(&bytes))
    }

    #[cfg(not(opc_core_linked))]
    pub fn from_core(_samples: &ScopeSamples, _color_mode: i32, _iso: i32) -> Option<Self> {
        None
    }

    /// `pictureStops<TAB>ndStops<TAB>ndLabel<TAB>stopsLabel`, as the core emits it.
    pub fn parse(text: &str) -> Option<Self> {
        let mut fields = text.trim().split('\t');
        let picture_stops = fields.next()?.parse().ok()?;
        let nd_stops = fields.next()?.parse().ok()?;
        let nd_label = fields.next()?.to_string();
        let stops_label = fields.next()?.to_string();
        Some(Self {
            picture_stops,
            nd_stops,
            nd_label,
            stops_label,
        })
    }
}

/// The meters' floor in dBFS, where a silent bar sits.
#[cfg(opc_core_linked)]
pub fn audio_floor_db() -> f32 {
    // Safety: no arguments.
    unsafe { opc_core_sys::opc_audio_meter_floor_db() as f32 }
}

#[cfg(not(opc_core_linked))]
pub fn audio_floor_db() -> f32 {
    -60.0
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NdNotation {
    Stops,
    #[default]
    Factor,
    Density,
}

impl NdNotation {
    pub const ALL: [Self; 3] = [Self::Stops, Self::Factor, Self::Density];

    pub fn label(self) -> &'static str {
        match self {
            Self::Stops => "Stops",
            Self::Factor => "ND32",
            Self::Density => "ND 0.3",
        }
    }
}

/// The scope options the phones offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WaveMode {
    Luma,
    #[default]
    Rgb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ParadeMode {
    #[default]
    Rgb,
    Yrgb,
}

/// Traffic-light crush / clip compensation, in stops: the fraction of the picture
/// that must sit in the clip zone before a lamp fires is `stops / 10`.
pub const LIGHTS_COMPENSATION: [(f64, &str); 5] =
    [(0.0, "0"), (0.25, "¼"), (0.5, "½"), (0.75, "¾"), (1.0, "1")];

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScopeOptions {
    pub wave: WaveMode,
    /// Guide lines on the waveform: clip, crush, middle grey.
    pub wave_guides: (bool, bool, bool),
    pub parade: ParadeMode,
    /// Trace zoom on the vectorscope: 1, 2 or 4.
    pub vector_gain: f32,
    /// Trace brightness, 0…200 on the phones; 100 is nominal.
    pub brightness: u32,
    pub lights_compensation: f64,
    pub nd_notation: NdNotation,
}

impl Default for ScopeOptions {
    fn default() -> Self {
        Self {
            wave: WaveMode::Rgb,
            wave_guides: (true, true, true),
            parade: ParadeMode::Rgb,
            vector_gain: 1.0,
            brightness: 100,
            lights_compensation: 0.0,
            nd_notation: NdNotation::Factor,
        }
    }
}

/// A rasterised plate, tightly packed RGBA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plate {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// OpenZCine's trace palette (`ScopePalette`): additive colours, straight alpha.
const LUMA: [f32; 4] = [222.0, 230.0, 224.0, 1.0];
const OVERLAY_RED: [f32; 4] = [255.0, 64.0, 54.0, 0.55];
const OVERLAY_GREEN: [f32; 4] = [70.0, 240.0, 110.0, 0.55];
const OVERLAY_BLUE: [f32; 4] = [72.0, 148.0, 255.0, 0.62];
const PARADE_RED: [f32; 4] = [255.0, 86.0, 78.0, 1.0];
const PARADE_GREEN: [f32; 4] = [102.0, 232.0, 132.0, 1.0];
const PARADE_BLUE: [f32; 4] = [92.0, 156.0, 255.0, 1.0];
const BOUNDARY: [u8; 4] = [220, 235, 225, 204];
const CLIP_LINE: [u8; 4] = [255, 150, 142, 204];
const MIDDLE_LINE: [u8; 4] = [246, 241, 226, 204];
const GRID_LINE: [u8; 4] = [220, 235, 225, 26];
const PANEL: [u8; 4] = [6, 9, 8, 184];
const HISTO_RED: [f32; 4] = [255.0, 48.0, 44.0, 0.17];
const HISTO_GREEN: [f32; 4] = [0.0, 238.0, 70.0, 0.15];
const HISTO_BLUE: [f32; 4] = [45.0, 76.0, 255.0, 0.19];
const TITLE_H: u32 = 26;
const SIDE_PAD: u32 = 6;
const BOTTOM_PAD: u32 = 2;

/// An additive float canvas the traces accumulate into, resolved to RGBA at the end.
struct Trace {
    width: u32,
    height: u32,
    acc: Vec<[f32; 3]>,
}

impl Trace {
    fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            acc: vec![[0.0; 3]; (width * height) as usize],
        }
    }

    fn add(&mut self, x: i64, y: i64, color: [f32; 4], weight: f32) {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return;
        }
        let at = (y as u32 * self.width + x as u32) as usize;
        let a = color[3] * weight;
        for (channel, value) in self.acc[at].iter_mut().zip(color) {
            *channel += value * a;
        }
    }

    /// Over the panel fill, with a soft knee so dense traces saturate to white
    /// rather than clipping per channel.
    fn resolve(self, base: &mut Plate) {
        for (at, acc) in self.acc.into_iter().enumerate() {
            let energy = acc.iter().copied().fold(0.0_f32, f32::max);
            if energy <= 0.0 {
                continue;
            }
            let knee = 1.0 - (-energy / 255.0).exp();
            let pixel = &mut base.rgba[at * 4..at * 4 + 4];
            for channel in 0..3 {
                let trace = (acc[channel] / energy) * knee * 255.0;
                let under = f32::from(pixel[channel]);
                pixel[channel] = (under + trace * (1.0 - knee * 0.2))
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
            pixel[3] = 255;
        }
    }
}

impl Plate {
    fn panel(width: u32, height: u32) -> Self {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..width * height {
            rgba.extend_from_slice(&PANEL);
        }
        Self {
            width,
            height,
            rgba,
        }
    }

    fn blend(&mut self, x: i64, y: i64, color: [u8; 4]) {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return;
        }
        let at = ((y as u32 * self.width + x as u32) * 4) as usize;
        let a = f32::from(color[3]) / 255.0;
        for (channel, value) in color.iter().enumerate().take(3) {
            let under = f32::from(self.rgba[at + channel]);
            self.rgba[at + channel] = (under + (f32::from(*value) - under) * a).round() as u8;
        }
        self.rgba[at + 3] = 255;
    }

    fn hline(&mut self, x0: i64, x1: i64, y: i64, color: [u8; 4], dash: Option<u32>) {
        for x in x0.min(x1)..=x0.max(x1) {
            if dash.is_some_and(|period| (x as u32 / period) % 2 == 1) {
                continue;
            }
            self.blend(x, y, color);
        }
    }

    fn vline(&mut self, x: i64, y0: i64, y1: i64, color: [u8; 4]) {
        for y in y0.min(y1)..=y0.max(y1) {
            self.blend(x, y, color);
        }
    }

    fn fill(&mut self, x: i64, y: i64, w: i64, h: i64, color: [u8; 4]) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.blend(xx, yy, color);
            }
        }
    }

    /// The pixel at `(x, y)` as `(r, g, b, a)`, for tests.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let at = ((y * self.width + x) * 4) as usize;
        self.rgba[at..at + 4].try_into().ok()
    }
}

/// The plot rectangle inside a plate: under the title, inside the pads.
fn plot(width: u32, height: u32) -> (i64, i64, i64, i64) {
    (
        i64::from(SIDE_PAD),
        i64::from(TITLE_H),
        i64::from(width - 2 * SIDE_PAD),
        i64::from(height - TITLE_H - BOTTOM_PAD),
    )
}

/// The y of a plot level inside a plot rectangle: level 1 is the top.
fn level_y(level: f32, top: i64, height: i64) -> i64 {
    top + ((1.0 - level.clamp(0.0, 1.0)) * (height - 1) as f32).round() as i64
}

/// The 0 and 100 lines, 18% grey, the safe-border dots and the faint 25 / 50 / 75 grid.
fn guides(
    plate: &mut Plate,
    scale: &ScopeScale,
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    which: (bool, bool, bool),
) {
    for ire in [25.0, 50.0, 75.0] {
        let line = level_y(ScopeScale::level_of_ire(ire), y, h);
        plate.hline(x, x + w - 1, line, GRID_LINE, None);
    }
    for ire in [5.0, 95.0] {
        let line = level_y(ScopeScale::level_of_ire(ire), y, h);
        plate.hline(x, x + w - 1, line, GRID_LINE, Some(3));
    }
    if which.1 {
        let crush = level_y(CRUSH_LEVEL, y, h);
        plate.hline(x, x + w - 1, crush, BOUNDARY, Some(3));
    }
    if which.0 {
        let clip = level_y(CLIP_LEVEL, y, h);
        plate.hline(x, x + w - 1, clip, CLIP_LINE, Some(3));
    }
    if which.2 {
        let middle = level_y(ScopeScale::level_of_ire(scale.grey_ire), y, h);
        plate.hline(x, x + w - 1, middle, MIDDLE_LINE, None);
    }
}

fn brightness_weight(brightness: u32, pixels: u32) -> f32 {
    // Nominal: a column that holds 1/256 of the samples reads as full.
    let per_column = (pixels as f32 / WAVE_COLS as f32).max(1.0);
    (brightness as f32 / 100.0) * 255.0 / per_column
}

/// Draws one channel's column counts across `w` × `h`, each count at its code's level.
#[allow(clippy::too_many_arguments)]
fn trace_channel(
    trace: &mut Trace,
    counts: &[u16],
    scale: &ScopeScale,
    color: [f32; 4],
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    weight: f32,
) {
    for code in 0..WAVE_ROWS {
        let level = scale.level[code];
        let row_y = level_y(level, y, h);
        let row = &counts[code * WAVE_COLS..(code + 1) * WAVE_COLS];
        for (column, count) in row.iter().enumerate() {
            if *count == 0 {
                continue;
            }
            let px = x + (column as i64 * w) / WAVE_COLS as i64;
            trace.add(px, row_y, color, f32::from(*count) * weight);
        }
    }
}

/// WAVE: luma, or the three channels overlaid.
pub fn waveform(samples: &ScopeSamples, scale: &ScopeScale, options: &ScopeOptions) -> Plate {
    let (width, height) = WAVEFORM_SIZE;
    let mut plate = Plate::panel(width, height);
    let (x, y, w, h) = plot(width, height);
    guides(&mut plate, scale, x, y, w, h, options.wave_guides);
    let mut trace = Trace::new(width, height);
    let weight = brightness_weight(options.brightness, samples.pixels);
    match options.wave {
        WaveMode::Luma => trace_channel(
            &mut trace,
            &samples.wave_luma,
            scale,
            LUMA,
            x,
            y,
            w,
            h,
            weight,
        ),
        WaveMode::Rgb => {
            trace_channel(
                &mut trace,
                &samples.wave_red,
                scale,
                OVERLAY_RED,
                x,
                y,
                w,
                h,
                weight,
            );
            trace_channel(
                &mut trace,
                &samples.wave_green,
                scale,
                OVERLAY_GREEN,
                x,
                y,
                w,
                h,
                weight,
            );
            trace_channel(
                &mut trace,
                &samples.wave_blue,
                scale,
                OVERLAY_BLUE,
                x,
                y,
                w,
                h,
                weight,
            );
        }
    }
    trace.resolve(&mut plate);
    plate
}

/// PARADE: R, G and B side by side, with a luma lane first for YRGB.
pub fn parade(samples: &ScopeSamples, scale: &ScopeScale, options: &ScopeOptions) -> Plate {
    let (width, height) = PARADE_SIZE;
    let mut plate = Plate::panel(width, height);
    let (x, y, w, h) = plot(width, height);
    guides(&mut plate, scale, x, y, w, h, (true, true, true));
    let lanes: Vec<(&[u16], [f32; 4])> = match options.parade {
        ParadeMode::Rgb => vec![
            (&samples.wave_red, PARADE_RED),
            (&samples.wave_green, PARADE_GREEN),
            (&samples.wave_blue, PARADE_BLUE),
        ],
        ParadeMode::Yrgb => vec![
            (&samples.wave_luma, LUMA),
            (&samples.wave_red, PARADE_RED),
            (&samples.wave_green, PARADE_GREEN),
            (&samples.wave_blue, PARADE_BLUE),
        ],
    };
    let count = lanes.len() as i64;
    let gap = 3;
    let lane_w = (w - gap * (count - 1)) / count;
    let mut trace = Trace::new(width, height);
    let weight = brightness_weight(options.brightness, samples.pixels) * count as f32;
    for (index, (counts, color)) in lanes.into_iter().enumerate() {
        let lane_x = x + index as i64 * (lane_w + gap);
        if index > 0 {
            plate.vline(lane_x - 2, y, y + h - 1, GRID_LINE);
        }
        trace_channel(
            &mut trace, counts, scale, color, lane_x, y, lane_w, h, weight,
        );
    }
    trace.resolve(&mut plate);
    plate
}

/// HISTO: the three channel fills and the luma line, on the waveform's axis so a
/// column here is the same level as a row there. The clip zone at 95 is marked.
pub fn histogram(samples: &ScopeSamples, scale: &ScopeScale) -> Plate {
    let (width, height) = HISTOGRAM_SIZE;
    let mut plate = Plate::panel(width, height);
    let (x, y, w, h) = plot(width, height);
    // Native counts into display buckets, conserving the total (`remapHistogram`).
    let remap = |bins: &[u32; HIST]| {
        let mut out = [0u32; HIST];
        for (code, count) in bins.iter().enumerate() {
            let bucket = (scale.level[code] * 255.0).round().clamp(0.0, 255.0) as usize;
            out[bucket] += count;
        }
        out
    };
    let channels = [
        (remap(&samples.red), HISTO_RED),
        (remap(&samples.green), HISTO_GREEN),
        (remap(&samples.blue), HISTO_BLUE),
    ];
    let luma = remap(&samples.luma);
    let peak = channels
        .iter()
        .flat_map(|(bins, _)| bins.iter().copied())
        .chain(luma.iter().copied())
        .max()
        .unwrap_or(0)
        .max(1) as f32;
    // Column at display level: 0.05 at the left edge, 0.95 at the right.
    let column_of = |bucket: usize| -> i64 {
        let level = bucket as f32 / 255.0;
        x + (((level - CRUSH_LEVEL) / (CLIP_LEVEL - CRUSH_LEVEL)).clamp(0.0, 1.0) * (w - 1) as f32)
            .round() as i64
    };
    for ire in [25.0, 50.0, 75.0] {
        let line = x + (ire / 100.0 * (w - 1) as f32).round() as i64;
        plate.vline(line, y, y + h - 1, GRID_LINE);
    }
    let clip_zone = x + (0.95 * (w - 1) as f32).round() as i64;
    plate.vline(clip_zone, y, y + h - 1, CLIP_LINE);
    let mut trace = Trace::new(width, height);
    for (bins, color) in &channels {
        for (bucket, count) in bins.iter().enumerate() {
            if *count == 0 {
                continue;
            }
            let column = column_of(bucket);
            let bar = ((*count as f32 / peak) * (h - 1) as f32).round() as i64;
            for row in 0..=bar {
                trace.add(
                    column,
                    y + h - 1 - row,
                    *color,
                    255.0 / color[3].max(0.01) / 255.0 * 1.6,
                );
            }
        }
    }
    trace.resolve(&mut plate);
    for (bucket, count) in luma.iter().enumerate() {
        if *count == 0 {
            continue;
        }
        let column = column_of(bucket);
        let bar = ((*count as f32 / peak) * (h - 1) as f32).round() as i64;
        plate.blend(
            column,
            y + h - 1 - bar,
            [LUMA[0] as u8, LUMA[1] as u8, LUMA[2] as u8, 230],
        );
    }
    plate
}

/// VECTOR: the chroma trace about the centre, the 75% graticule and the 123°
/// skin-tone line. Trace zoom magnifies the trace only; the graticule stays at unity.
pub fn vectorscope(samples: &ScopeSamples, options: &ScopeOptions) -> Plate {
    let (width, height) = VECTORSCOPE_SIZE;
    let mut plate = Plate::panel(width, height);
    let cx = i64::from(width) / 2;
    let cy = i64::from(height) / 2;
    let radius = (i64::from(width.min(height)) / 2 - 8) as f32;
    // Graticule: unity circle, the 75% ring, the axes and the skin line.
    for step in 0..720 {
        let angle = step as f32 * std::f32::consts::PI / 360.0;
        for (fraction, color) in [(1.0, BOUNDARY), (0.75, GRID_LINE)] {
            let x = cx + (angle.cos() * radius * fraction).round() as i64;
            let y = cy - (angle.sin() * radius * fraction).round() as i64;
            plate.blend(x, y, color);
        }
    }
    plate.hline(cx - radius as i64, cx + radius as i64, cy, GRID_LINE, None);
    plate.vline(cx, cy - radius as i64, cy + radius as i64, GRID_LINE);
    let skin = 123.0_f32.to_radians();
    for step in 0..radius as i64 {
        let x = cx + (skin.cos() * step as f32).round() as i64;
        let y = cy - (skin.sin() * step as f32).round() as i64;
        plate.blend(x, y, MIDDLE_LINE);
    }
    let mut trace = Trace::new(width, height);
    let per_bin = (samples.pixels as f32 / 512.0).max(1.0);
    let weight = (options.brightness as f32 / 100.0) * 255.0 / per_bin;
    let half = VECTOR_N as f32 / 2.0;
    for (index, count) in samples.vector.iter().enumerate() {
        if *count == 0 {
            continue;
        }
        let u = (index % VECTOR_N) as f32 - half;
        let v = (index / VECTOR_N) as f32 - half;
        let px = cx + (u / half * radius * options.vector_gain).round() as i64;
        let py = cy - (v / half * radius * options.vector_gain).round() as i64;
        trace.add(px, py, LUMA, f32::from(*count) * weight);
    }
    trace.resolve(&mut plate);
    plate
}

/// LIGHTS: three lamps, one per channel, lit red at the top for clip and blue at the
/// bottom for crush, with the channel's balance as a bar between.
pub fn traffic_lights(reading: &LightsReading) -> Plate {
    let (width, height) = LIGHTS_SIZE;
    let mut plate = Plate::panel(width, height);
    let lamp_w = 16_i64;
    let gap = 6_i64;
    let x0 = (i64::from(width) - (3 * lamp_w + 2 * gap)) / 2;
    let top = i64::from(TITLE_H);
    let bottom = i64::from(height) - 8;
    let inner = bottom - top;
    for (index, (light, tint)) in [
        (reading.red, [255u8, 86, 78, 255]),
        (reading.green, [102u8, 232, 132, 255]),
        (reading.blue, [92u8, 156, 255, 255]),
    ]
    .into_iter()
    .enumerate()
    {
        let x = x0 + index as i64 * (lamp_w + gap);
        plate.fill(x, top, lamp_w, inner, [255, 255, 255, 20]);
        // The balance bar from the middle: up when the median sits high.
        let mid = top + inner / 2;
        let level = (light.level.clamp(0.0, 1.0) - 0.5) * inner as f32;
        let bar_y = mid - level.max(0.0).round() as i64;
        let bar_h = level.abs().round().max(2.0) as i64;
        plate.fill(
            x + 3,
            bar_y,
            lamp_w - 6,
            bar_h,
            [tint[0], tint[1], tint[2], 140],
        );
        plate.hline(x, x + lamp_w - 1, mid, GRID_LINE, None);
        if light.clip {
            plate.fill(x, top, lamp_w, 14, [255, 64, 54, 255]);
        }
        if light.crush {
            plate.fill(x, bottom - 14, lamp_w, 14, [72, 148, 255, 255]);
        }
    }
    plate
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(width: u32, height: u32, y: u8, u: u8, v: u8) -> OwnedPicture {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        let (r, g, b) = rgb(y, u, v);
        for _ in 0..width * height {
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
        OwnedPicture::from_rgba(width, height, &rgba)
    }

    #[test]
    fn a_flat_picture_lands_in_one_bin_and_one_waveform_row() {
        let picture = flat(64, 36, 128, 128, 128);
        let samples = ScopeSamples::read(&picture);
        assert!(samples.pixels > 0);
        let (code, count) = samples
            .luma
            .iter()
            .enumerate()
            .max_by_key(|(_, count)| **count)
            .expect("bins");
        assert_eq!(*count, samples.pixels, "every sample in one bin");
        assert!((code as i32 - 128).abs() <= 1, "the grey it was: {code}");
        let rows_used = (0..WAVE_ROWS)
            .filter(|row| {
                samples.wave_luma[row * WAVE_COLS..(row + 1) * WAVE_COLS]
                    .iter()
                    .any(|c| *c > 0)
            })
            .count();
        assert_eq!(rows_used, 1);
        assert_eq!(
            u32::from(samples.vector[64 * VECTOR_N + 64]),
            samples.pixels,
            "neutral chroma at the centre"
        );
    }

    #[test]
    fn the_plates_come_out_at_the_phones_sizes_and_draw_the_trace() {
        let picture = flat(64, 36, 200, 128, 128);
        let samples = ScopeSamples::read(&picture);
        let scale = ScopeScale::default();
        let options = ScopeOptions {
            wave: WaveMode::Luma,
            ..ScopeOptions::default()
        };
        let wave = waveform(&samples, &scale, &options);
        assert_eq!((wave.width, wave.height), WAVEFORM_SIZE);
        // The trace row for the grey's code on the plain axis (the RGB round trip may
        // land a code either side): brighter than the panel somewhere near it.
        let (x, y, w, h) = plot(wave.width, wave.height);
        let code = samples
            .luma
            .iter()
            .enumerate()
            .max_by_key(|(_, count)| **count)
            .map(|(code, _)| code)
            .expect("bins");
        let row = level_y(scale.level[code], y, h);
        let column = (x + w / 2) as u32;
        let lit = (row - 3..=row + 3)
            .filter_map(|r| wave.pixel(column, r as u32))
            .any(|pixel| pixel[0] > PANEL[0] + 40);
        assert!(lit, "trace drawn near row {row}");
        let quiet = wave.pixel(column, (row + 40) as u32).expect("in plate");
        assert!(quiet[0] < 60, "nothing elsewhere: {quiet:?}");
        assert_eq!(parade(&samples, &scale, &options).width, PARADE_SIZE.0);
        assert_eq!(histogram(&samples, &scale).height, HISTOGRAM_SIZE.1);
        assert_eq!(vectorscope(&samples, &options).width, VECTORSCOPE_SIZE.0);
        assert_eq!(
            traffic_lights(&LightsReading::default()).height,
            LIGHTS_SIZE.1
        );
    }

    #[test]
    fn the_nd_reading_is_parsed_and_read_in_three_notations() {
        let reading = NdReading::parse("2.3\t2\tND4\t+2.3").expect("parsed");
        assert_eq!(reading.text(NdNotation::Stops), "+2.3");
        assert_eq!(reading.text(NdNotation::Factor), "ND4");
        assert_eq!(reading.text(NdNotation::Density), "ND 0.6");
        assert_eq!(NdReading::parse("nope"), None);
        let none = NdReading::parse("-0.2\t0\t—\t-0.2").expect("parsed");
        assert_eq!(none.text(NdNotation::Density), "—");
    }
}
