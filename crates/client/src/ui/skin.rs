//! The interface's look: the pixel font and the pixel-art pieces every screen is built
//! from, prepared once at startup.
//!
//! The fonts are drawn as text in `assets/ui/fonts` and the chosen one is built into a
//! TrueType font when the game starts or the setting changes (`voxmar_texgen::font`). The pieces (panels, buttons, slots, wells, the
//! hotbar's frame) are `ui/*` textures from the texture packs, so a pack can reskin the
//! interface the way it retextures blocks; our default pack draws them all in
//! `recipes/ui.ron`. Both live behind fixed handles, so widgets can name a piece without
//! carrying the skin around.

use bevy::asset::uuid::Uuid;
use bevy::prelude::*;
use bevy::sprite::{BorderRect, SliceScaleMode, TextureSlicer};
use serde::{Deserialize, Serialize};
use voxmar_texgen::font::{EM_PIXELS, PixelFont};

use crate::render::ui_image;
use crate::textures::{Rgba, Source};

/// Screen pixels (logical) per texel of the pieces and per pixel of the font.
pub const PX: f32 = 2.0;

/// The pixel font, at every size.
/// The narrowest a key cap is drawn: its sliced frame needs room for both corners, or a
/// one-letter cap loses pixels of its border.
pub const CAP_MIN: f32 = 12.0 * PX;

pub const FONT: Handle<Font> = Handle::Uuid(Uuid::from_u128(0x766f_786d_6172_5f75_695f_666f_6e74_0000), std::marker::PhantomData);

/// Font sizes that draw each font pixel as a whole number of screen pixels.
pub const fn font_size(scale: u32) -> f32 {
    (EM_PIXELS as u32 * scale) as f32
}

/// A piece of pixel art the interface is built from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Piece {
    Panel,
    Button,
    ButtonHover,
    ButtonDown,
    ButtonSelected,
    Primary,
    PrimaryHover,
    PrimaryDown,
    Danger,
    DangerHover,
    DangerDown,
    Well,
    WellHover,
    WellFocus,
    Bar,
    BarHealth,
    BarHunger,
    Meter,
    Knob,
    KnobSmall,
    Tooltip,
    Frame,
    Selection,
    Heart,
    HeartHalf,
    HeartEmpty,
    /// What each worn slot takes, faint in it while it is empty.
    WornHead,
    WornBody,
    WornLegs,
    WornHands,
    WornFeet,
    WornShield,
    WornCharm,
}

impl Piece {
    const ALL: [Piece; 33] = [
        Piece::Panel,
        Piece::Button,
        Piece::ButtonHover,
        Piece::ButtonDown,
        Piece::ButtonSelected,
        Piece::Primary,
        Piece::PrimaryHover,
        Piece::PrimaryDown,
        Piece::Danger,
        Piece::DangerHover,
        Piece::DangerDown,
        Piece::Well,
        Piece::WellHover,
        Piece::WellFocus,
        Piece::Bar,
        Piece::BarHealth,
        Piece::BarHunger,
        Piece::Meter,
        Piece::Knob,
        Piece::KnobSmall,
        Piece::Tooltip,
        Piece::Frame,
        Piece::Selection,
        Piece::Heart,
        Piece::HeartHalf,
        Piece::HeartEmpty,
        Piece::WornHead,
        Piece::WornBody,
        Piece::WornLegs,
        Piece::WornHands,
        Piece::WornFeet,
        Piece::WornShield,
        Piece::WornCharm,
    ];

    /// The texture's name in the packs, and its size and slicing inset in texels (none
    /// for pieces drawn whole).
    fn spec(self) -> (&'static str, (u32, u32), Option<f32>) {
        match self {
            Piece::Panel => ("ui/panel", (16, 16), Some(4.0)),
            Piece::Button => ("ui/button", (10, 10), Some(3.0)),
            Piece::ButtonHover => ("ui/button_hover", (10, 10), Some(3.0)),
            Piece::ButtonDown => ("ui/button_down", (10, 10), Some(3.0)),
            Piece::ButtonSelected => ("ui/button_selected", (10, 10), Some(3.0)),
            Piece::Primary => ("ui/primary", (10, 10), Some(3.0)),
            Piece::PrimaryHover => ("ui/primary_hover", (10, 10), Some(3.0)),
            Piece::PrimaryDown => ("ui/primary_down", (10, 10), Some(3.0)),
            Piece::Danger => ("ui/danger", (10, 10), Some(3.0)),
            Piece::DangerHover => ("ui/danger_hover", (10, 10), Some(3.0)),
            Piece::DangerDown => ("ui/danger_down", (10, 10), Some(3.0)),
            Piece::Well => ("ui/well", (6, 6), Some(2.0)),
            Piece::WellHover => ("ui/well_hover", (6, 6), Some(2.0)),
            Piece::WellFocus => ("ui/well_focus", (6, 6), Some(2.0)),
            Piece::Bar => ("ui/bar", (4, 4), Some(1.0)),
            Piece::BarHealth => ("ui/bar_health", (4, 4), Some(1.0)),
            Piece::BarHunger => ("ui/bar_hunger", (4, 4), Some(1.0)),
            Piece::Meter => ("ui/meter", (8, 8), Some(3.0)),
            Piece::Knob => ("ui/knob", (6, 9), None),
            Piece::KnobSmall => ("ui/knob_small", (6, 6), None),
            Piece::Tooltip => ("ui/tooltip", (8, 8), Some(3.0)),
            Piece::Frame => ("ui/frame", (8, 8), Some(3.0)),
            Piece::Selection => ("ui/selection", (10, 10), Some(3.0)),
            Piece::Heart => ("ui/heart", (9, 9), None),
            Piece::HeartHalf => ("ui/heart_half", (9, 9), None),
            Piece::HeartEmpty => ("ui/heart_empty", (9, 9), None),
            Piece::WornHead => ("ui/slot/helm", (16, 16), None),
            Piece::WornBody => ("ui/slot/chest", (16, 16), None),
            Piece::WornLegs => ("ui/slot/legs", (16, 16), None),
            Piece::WornHands => ("ui/slot/gloves", (16, 16), None),
            Piece::WornFeet => ("ui/slot/boots", (16, 16), None),
            Piece::WornShield => ("ui/slot/shield", (16, 16), None),
            Piece::WornCharm => ("ui/slot/charm", (16, 16), None),
        }
    }

    pub fn handle(self) -> Handle<Image> {
        let index = Piece::ALL.iter().position(|&p| p == self).expect("every piece is listed") as u128;
        Handle::Uuid(Uuid::from_u128(0x766f_786d_6172_5f75_695f_7069_6563_0000 + index), std::marker::PhantomData)
    }

    /// The piece as a node's image: nine-sliced at its inset, each texel `PX` screen
    /// pixels, or stretched over the node when drawn whole.
    pub fn image(self) -> ImageNode {
        let (_, _, inset) = self.spec();
        let mode = match inset {
            Some(inset) => NodeImageMode::Sliced(TextureSlicer {
                border: BorderRect::all(inset),
                center_scale_mode: SliceScaleMode::Stretch,
                sides_scale_mode: SliceScaleMode::Stretch,
                max_corner_scale: PX,
            }),
            None => NodeImageMode::Stretch,
        };
        ImageNode { image: self.handle(), image_mode: mode, visual_box: VisualBox::BorderBox, ..default() }
    }

    /// The piece's size on screen when drawn whole.
    pub fn size(self) -> Vec2 {
        let (_, (w, h), _) = self.spec();
        Vec2::new(w as f32, h as f32) * PX
    }
}

/// The pixel fonts a player can choose from, drawn in `assets/ui/fonts`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum InterfaceFont {
    /// A monospace font with a typewriter's slab serifs.
    #[default]
    Ledger,
    /// A monospace font with square corners.
    Block,
    /// The first, proportional font.
    Voxmar,
}

impl InterfaceFont {
    pub const ALL: [InterfaceFont; 3] = [InterfaceFont::Ledger, InterfaceFont::Block, InterfaceFont::Voxmar];

    /// Its name in the font it builds and in the gate's log; players see [`Self::key`].
    pub fn label(self) -> &'static str {
        match self {
            InterfaceFont::Ledger => "Ledger",
            InterfaceFont::Block => "Block",
            InterfaceFont::Voxmar => "Voxmar",
        }
    }

    /// Its name in the settings, from lang.
    pub fn key(self) -> &'static str {
        match self {
            InterfaceFont::Ledger => "settings.font.ledger",
            InterfaceFont::Block => "settings.font.block",
            InterfaceFont::Voxmar => "settings.font.voxmar",
        }
    }

    fn file(self) -> &'static str {
        match self {
            InterfaceFont::Ledger => "ledger.txt",
            InterfaceFont::Block => "block.txt",
            InterfaceFont::Voxmar => "voxmar.txt",
        }
    }
}

/// Builds a font into the fixed handle every text uses, so changing it changes all text.
/// A font file that cannot be read leaves the font that was there.
pub fn set_font(choice: InterfaceFont, fonts: &mut Assets<Font>) {
    let dir = voxmar_server::assets_dir().join("ui/fonts");
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
    let font = read(choice.file())
        .ok_or_else(|| format!("cannot read {}", choice.file()))
        .and_then(|text| PixelFont::parse_with(&text, read).map_err(|e| e.to_string()));
    match font {
        Ok(mut font) => {
            font.fill_from(fallback());
            let _ = fonts.insert(&FONT, Font::from_bytes(font.to_ttf(&format!("Voxmar {}", choice.label()))));
        }
        Err(e) => warn!("cannot build the interface font {}: {e}", dir.join(choice.file()).display()),
    }
}

/// Glyphs for what our fonts don't draw (Japanese, for one), from the pixel fonts listed in
/// `assets/ui/fonts/fallback.ron`, in order. Read once: thousands of kanji take a moment.
pub fn fallback() -> &'static PixelFont {
    static FALLBACK: std::sync::OnceLock<PixelFont> = std::sync::OnceLock::new();
    FALLBACK.get_or_init(|| read_fallback(&voxmar_server::assets_dir().join("ui/fonts")))
}

/// A font in `assets/ui/fonts/fallback.ron`.
#[derive(Deserialize)]
struct Fallback {
    /// A TrueType or OpenType file, relative to `assets/ui/fonts`.
    file: String,
    /// For a pixel-outline font, the pixels its em is drawn on: its glyphs are merged into
    /// ours. None for a smooth font, which text falls back to as it is, at the text's size.
    #[serde(default)]
    pixels_per_em: Option<u32>,
    /// For a pixel-outline font, how big its pixels are drawn, in ours: below 1, its glyphs
    /// keep all their detail and take less room.
    #[serde(default = "full_size")]
    scale: f32,
}

fn full_size() -> f32 {
    1.0
}

/// The fonts listed in the fonts directory `dir`, with their files' bytes, in order.
fn fallback_fonts(dir: &std::path::Path) -> Vec<(Fallback, Vec<u8>)> {
    let list = std::fs::read_to_string(dir.join("fallback.ron")).map_err(|e| e.to_string()).and_then(|source| {
        ron::from_str::<Vec<Fallback>>(&source).map_err(|e| e.to_string())
    });
    match list {
        Ok(list) => list
            .into_iter()
            .filter_map(|entry| match std::fs::read(dir.join(&entry.file)) {
                Ok(data) => Some((entry, data)),
                Err(e) => {
                    warn!("cannot read the fallback font {}: {e}", entry.file);
                    None
                }
            })
            .collect(),
        Err(e) => {
            warn!("cannot read {}: {e}", dir.join("fallback.ron").display());
            Vec::new()
        }
    }
}

/// The pixel fallback fonts' glyphs, merged in order, from the fonts directory `dir`.
pub fn read_fallback(dir: &std::path::Path) -> PixelFont {
    let mut merged = PixelFont::default();
    for (entry, data) in fallback_fonts(dir) {
        let Some(pixels_per_em) = entry.pixels_per_em else { continue };
        match voxmar_texgen::font::import(&data, pixels_per_em, entry.scale) {
            Ok(font) => merged.fill_from(&font),
            Err(e) => warn!("cannot read the fallback font {}: {e}", entry.file),
        }
    }
    merged
}

/// The smooth fallback fonts' files, in order, from the fonts directory `dir`.
pub fn smooth_fallbacks(dir: &std::path::Path) -> Vec<Vec<u8>> {
    fallback_fonts(dir).into_iter().filter(|(entry, _)| entry.pixels_per_em.is_none()).map(|(_, data)| data).collect()
}

/// Reads every piece from the texture packs into its fixed handle. A piece no pack draws
/// is a flat grey stand-in.
pub fn load(source: &mut Source, images: &mut Assets<Image>) {
    for piece in Piece::ALL {
        let (name, (w, h), _) = piece.spec();
        let art = source.read(name).unwrap_or_else(|| {
            warn!("no texture pack draws {name}");
            Rgba { width: w, height: h, pixels: [70, 74, 90, 255].repeat((w * h) as usize) }
        });
        let _ = images.insert(&piece.handle(), ui_image(&art));
    }
}
