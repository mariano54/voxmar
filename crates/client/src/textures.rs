//! Texture sources. Block, colormap and HUD textures are read at runtime from a stack of
//! texture packs: the player's chosen pack, their Minecraft install if they have one
//! (nothing from Minecraft is shipped or needed), and our own default pack, which draws
//! everything the game uses. Whatever no pack provides gets a simple generated placeholder.

use std::collections::HashMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use voxmar_engine::block::{BlockId, BlockRegistry, RenderKind, TextureRef};
use voxmar_texgen::{Image, Library, Manifest};

use crate::settings::TextureOrder;

#[derive(Clone)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// The frames of an animated block texture after its first, which the texture itself is:
/// a strip of square frames one under another, as Minecraft's are, shown for `ticks`
/// twentieths of a second each.
pub struct Animation {
    pub frames: Vec<Rgba>,
    pub ticks: u8,
}

/// Twentieths of a second a frame shows for when no `.mcmeta` says: a fire's four frames
/// take 0.6 s, slow enough to read as flickering rather than buzzing.
const FRAME_TICKS: u8 = 3;

impl Rgba {
    /// Its square frames, top first, if it is a strip of more than one; itself otherwise.
    fn frames(&self) -> Vec<Rgba> {
        let side = self.width;
        if side == 0 || self.height <= side || self.height % side != 0 {
            return vec![self.clone()];
        }
        let size = (side * side * 4) as usize;
        self.pixels.chunks(size).map(|pixels| Rgba { width: side, height: side, pixels: pixels.to_vec() }).collect()
    }

    /// What [`Self::fill_transparent`] gives its clear texels: the average colour of those
    /// at least half opaque.
    pub fn fill_colour(&self) -> [u8; 3] {
        let (mut sum, mut count) = ([0u32; 3], 0u32);
        for pixel in self.pixels.as_chunks::<4>().0.iter().filter(|p| p[3] >= 128) {
            for c in 0..3 {
                sum[c] += pixel[c] as u32;
            }
            count += 1;
        }
        sum.map(|s| s.checked_div(count).unwrap_or(0) as u8)
    }

    /// How many of its texels are not wholly opaque, and how many are less than half.
    fn see_through(&self) -> (usize, usize) {
        let texels = self.pixels.as_chunks::<4>().0;
        (texels.iter().filter(|p| p[3] != 255).count(), texels.iter().filter(|p| p[3] < 128).count())
    }

    fn solid(width: u32, height: u32, color: [u8; 4]) -> Self {
        Self { width, height, pixels: color.repeat((width * height) as usize) }
    }

    /// Makes every texel opaque, giving transparent ones the average opaque colour. For
    /// textures drawn on full cubes (a cactus side has transparent margins in Minecraft's
    /// model), which would otherwise show holes at the edges.
    pub fn fill_transparent(&mut self) {
        let average = self.fill_colour();
        for pixel in self.pixels.as_chunks_mut::<4>().0 {
            if pixel[3] < 128 {
                pixel[..3].copy_from_slice(&average);
            }
            pixel[3] = 255;
        }
    }

    /// The top square of an image (first frame of an animation strip), nearest-resampled
    /// to `size`.
    /// This drawn over `under` (both the same size) by its alpha: an ore over its rock.
    fn over(&self, under: &Rgba) -> Rgba {
        let pixels = self
            .pixels
            .chunks(4)
            .zip(under.pixels.chunks(4))
            .flat_map(|(over, under)| {
                let a = over[3] as u32;
                let mix = |o: u8, u: u8| ((o as u32 * a + u as u32 * (255 - a) + 127) / 255) as u8;
                [mix(over[0], under[0]), mix(over[1], under[1]), mix(over[2], under[2]), under[3].max(over[3])]
            })
            .collect();
        Rgba { width: self.width, height: self.height, pixels }
    }

    fn square(&self, size: u32) -> Rgba {
        let side = self.width.min(self.height);
        let mut pixels = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                let (sx, sy) = (x * side / size, y * side / size);
                let i = ((sy * self.width + sx) * 4) as usize;
                pixels.extend_from_slice(&self.pixels[i..i + 4]);
            }
        }
        Rgba { width: size, height: size, pixels }
    }

    /// Every texel's colour multiplied by `tint` (a block's [`voxmar_engine::block::BlockTint::Fixed`]),
    /// its transparency kept.
    fn multiplied(mut self, tint: [u8; 3]) -> Rgba {
        for texel in self.pixels.chunks_exact_mut(4) {
            for (channel, &by) in texel.iter_mut().zip(&tint) {
                *channel = (*channel as u32 * by as u32 / 255) as u8;
            }
        }
        self
    }
}

/// Where a pack's files live.
enum Files {
    Zip(zip::ZipArchive<File>),
    Folder(PathBuf),
}

impl Files {
    fn open(path: &Path) -> Result<Self, String> {
        if path.is_dir() {
            return Ok(Files::Folder(path.to_owned()));
        }
        let file = File::open(path).map_err(|e| e.to_string())?;
        zip::ZipArchive::new(file).map(Files::Zip).map_err(|e| e.to_string())
    }

    fn read(&mut self, name: &str) -> Option<Vec<u8>> {
        match self {
            Files::Zip(archive) => {
                let mut file = archive.by_name(name).ok()?;
                let mut bytes = Vec::with_capacity(file.size() as usize);
                file.read_to_end(&mut bytes).ok()?;
                Some(bytes)
            }
            Files::Folder(root) => std::fs::read(root.join(name)).ok(),
        }
    }

    fn contains(&self, name: &str) -> bool {
        match self {
            Files::Zip(archive) => archive.index_for_name(name).is_some(),
            Files::Folder(root) => root.join(name).is_file(),
        }
    }

    /// The pack's recipe books, `recipes/*.ron`, as (path, source) in name order.
    fn recipe_books(&mut self) -> Vec<(String, String)> {
        let mut names: Vec<String> = match self {
            Files::Zip(archive) => archive.file_names().filter(|n| n.starts_with("recipes/")).map(str::to_owned).collect(),
            Files::Folder(root) => std::fs::read_dir(root.join("recipes"))
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| format!("recipes/{}", entry.file_name().to_string_lossy()))
                .collect(),
        };
        names.retain(|n| n.ends_with(".ron") && !n[8..].contains('/'));
        names.sort();
        names
            .into_iter()
            .filter_map(|name| {
                let source = String::from_utf8(self.read(&name)?).ok()?;
                Some((name, source))
            })
            .collect()
    }
}

/// A pack's list of other drawings of its textures: a map from a texture's path to the
/// paths of its alternates, `{"block/stone": ["block/stone_2", "block/stone_3"]}`.
const VARIANTS_FILE: &str = "variants.ron";

/// One texture pack: a folder, zip or jar with images at `textures/<path>.png` (our
/// layout) or `assets/minecraft/textures/<path>.png` (Minecraft's, so its client jar and
/// resource packs work unchanged), and texture recipes in `recipes/*.ron`, rendered here
/// as the game loads (see the texgen crate). A PNG beats a recipe of the same name.
///
/// A folder with a `pack.ron` (a [`Manifest`]) is a whole look the player picks, such as
/// Voxmar HD: it builds on the packs it extends, taking their images and recipes for
/// everything it does not draw itself, and draws block and item textures at its
/// resolution.
struct Pack {
    name: String,
    /// Its own files first, then those of the packs it builds on.
    files: Vec<Files>,
    recipes: Library,
    /// Texels across that block and item textures are drawn at, at least.
    resolution: u32,
    /// Other drawings of its textures, from `variants.ron` (see [`Source::block_variants`]).
    variants: HashMap<String, Vec<String>>,
    /// How things are held in the hand, from `held.ron` (see `held`), over those of the packs
    /// it builds on.
    held: Option<crate::held::Book>,
    /// Textures this pack provided since the last log.
    served: usize,
    /// Whether the game ships it (our default pack and Voxmar HD), which a test holds to
    /// opaque faces with no clear texel at all (see [`see_through_warnings`]).
    shipped: bool,
}

impl Pack {
    fn open(path: &Path) -> Result<Self, String> {
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        let manifest = if path.is_dir() { Manifest::read(path).map_err(|e| e.to_string())? } else { None };
        let folders = if manifest.is_some() { Manifest::chain(path).map_err(|e| e.to_string())? } else { vec![path.to_owned()] };
        let mut files = folders.iter().map(|folder| Files::open(folder)).collect::<Result<Vec<_>, _>>()?;
        let books: Vec<Vec<(String, String)>> = files.iter_mut().map(Files::recipe_books).collect();
        let (recipes, errors) =
            Library::from_layers_lenient(books.iter().map(|layer| layer.iter().map(|(n, s)| (n.as_str(), s.as_str())).collect()));
        for e in errors {
            bevy::log::warn!("texture pack {name}: {e}; leaving that book out");
        }
        files.reverse();
        // A pack's own variants win over those of the packs it builds on.
        let mut variants = HashMap::new();
        for layer in files.iter_mut().rev() {
            let Some(bytes) = layer.read(VARIANTS_FILE) else { continue };
            match std::str::from_utf8(&bytes).map_err(|e| e.to_string()).and_then(|s| ron::from_str::<HashMap<String, Vec<String>>>(s).map_err(|e| e.to_string())) {
                Ok(more) => variants.extend(more),
                Err(e) => bevy::log::warn!("texture pack {name}: {VARIANTS_FILE}: {e}; leaving it out"),
            }
        }
        // So do its own poses of the hand over those of the packs it builds on.
        let mut held: Option<crate::held::Book> = None;
        for layer in files.iter_mut().rev() {
            let Some(bytes) = layer.read(crate::held::FILE) else { continue };
            match std::str::from_utf8(&bytes).map_err(|e| e.to_string()).and_then(|s| crate::held::Book::parse(s)) {
                Ok(book) => held.get_or_insert_with(Default::default).overlay(book, &mut |what| bevy::log::info!("texture pack {name}: {} replaces {what} of a pack it builds on", crate::held::FILE)),
                Err(e) => bevy::log::warn!("texture pack {name}: {e}; leaving it out"),
            }
        }
        let resolution = manifest.as_ref().map_or(0, |m| m.resolution);
        Ok(Self { name: manifest.map_or(name, |m| m.name), files, recipes, resolution, variants, held, served: 0, shipped: false })
    }

    /// Whether the pack has `<path>.png`, as an image or a recipe.
    fn contains(&self, path: &str) -> bool {
        ["textures", "assets/minecraft/textures"]
            .iter()
            .any(|root| self.files.iter().any(|files| files.contains(&format!("{root}/{path}.png"))))
            || self.recipes.contains(path)
    }

    /// The frame time an image's `<path>.png.mcmeta` gives (`animation.frametime`, in
    /// ticks), as Minecraft's packs and those made for it carry one.
    fn frame_ticks(&mut self, path: &str) -> Option<u8> {
        let bytes = ["textures", "assets/minecraft/textures"]
            .iter()
            .find_map(|root| self.files.iter_mut().find_map(|files| files.read(&format!("{root}/{path}.png.mcmeta"))))?;
        let meta: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
        let ticks = meta.get("animation")?.get("frametime")?.as_u64()?;
        Some(ticks.clamp(1, 255) as u8)
    }

    fn read(&mut self, path: &str) -> Option<Rgba> {
        // Blocks and items only: interface sprites are laid out texel for texel.
        let resolution = if path.starts_with("block/") || path.starts_with("item/") { self.resolution } else { 0 };
        let image = ["textures", "assets/minecraft/textures"]
            .iter()
            .find_map(|root| self.files.iter_mut().find_map(|files| files.read(&format!("{root}/{path}.png"))))
            .and_then(|bytes| Image::decode_png(&bytes))
            .or_else(|| match self.recipes.render_at_least(path, resolution)? {
                Ok(image) => Some(image),
                Err(e) => {
                    bevy::log::warn!("texture pack {}: {e}", self.name);
                    None
                }
            })?;
        self.served += 1;
        Some(Rgba { width: image.width, height: image.height, pixels: image.pixels })
    }
}

/// The texture packs in priority order. Each texture comes from the first pack that has
/// it, then from a placeholder:
///
/// 1. the pack the player chose with `$VOXMAR_TEXTURES` (a folder, zip or jar), which
///    overrides everything below;
/// 2. with the `textures` setting at Voxmar HD, that pack (`assets/packs/voxmar_hd`);
/// 3. with the `textures` setting at Minecraft first, the newest Minecraft client jar
///    found in a standard launcher location (the other orders put it last). Only with
///    `minecraft_assets` on: without it the game never opens the player's install;
/// 4. our default pack, `assets/packs/default`, which draws everything: what Minecraft
///    has not got (our stones, woods, plants, wonders) and, with the setting, the rest;
/// 5. any other pack in `assets/packs` without a `pack.ron`, in name order (how mods ship
///    art: they add textures, but cannot replace ours). Packs with one are looks the
///    player picks, such as Voxmar HD, and come first only when picked.
///
/// A chosen pack replaces the Minecraft jar.
pub struct Source {
    packs: Vec<Pack>,
}

/// The Voxmar HD pack's folder in `assets/packs`.
pub const HD_PACK: &str = "voxmar_hd";

impl Source {
    /// The packs for texture order `order`, with the player's Minecraft jar among them
    /// when `minecraft` (the `minecraft_assets` setting).
    pub fn discover(order: TextureOrder, minecraft: bool) -> Self {
        let assets = voxmar_server::assets_dir();
        let root = assets.join("packs");
        let mut packs = Vec::new();
        let chosen = std::env::var_os("VOXMAR_TEXTURES").map(PathBuf::from);
        if let Some(path) = &chosen {
            match Pack::open(path) {
                Ok(pack) => packs.push(pack),
                Err(e) => bevy::log::warn!("cannot open texture pack {}: {e}", path.display()),
            }
        }
        if order == TextureOrder::VoxmarHd {
            match Pack::open(&root.join(HD_PACK)) {
                Ok(pack) => packs.push(Pack { shipped: true, ..pack }),
                Err(e) => bevy::log::warn!("cannot open texture pack {HD_PACK}: {e}"),
            }
        }
        let jar = if minecraft && chosen.is_none() { find_minecraft_jar() } else { None };
        let minecraft = jar.and_then(|jar| Pack::open(&jar).map_err(|e| bevy::log::warn!("cannot open {}: {e}", jar.display())).ok());
        let mut minecraft = minecraft.into_iter();
        if order == TextureOrder::Minecraft {
            packs.extend(minecraft.by_ref());
        }
        let mut bundled: Vec<PathBuf> = std::fs::read_dir(&root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir() && !path.join(Manifest::FILE).exists())
            .collect();
        bundled.sort_by_key(|path| (path.file_name() != Some("default".as_ref()), path.clone()));
        let default = root.join("default");
        packs.extend(bundled.iter().filter_map(|path| Pack::open(path).ok().map(|pack| Pack { shipped: *path == default, ..pack })));
        packs.extend(minecraft);
        Self { packs }
    }

    /// The packs every player has whatever they choose: the default pack and the mods' art
    /// (a pack without a `pack.ron`), never a look picked or the Minecraft jar.
    pub fn bundled(root: &std::path::Path) -> Self {
        let mut bundled: Vec<PathBuf> = std::fs::read_dir(root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir() && !path.join(Manifest::FILE).exists())
            .collect();
        bundled.sort_by_key(|path| (path.file_name() != Some("default".as_ref()), path.clone()));
        Self { packs: bundled.iter().filter_map(|path| Pack::open(path).ok()).collect() }
    }

    /// The effects no bundled pack draws an icon for (`ui/effect/<icon>`), by name: the
    /// launch refuses them, so every effect's chip says which it is at a glance.
    pub fn unpictured(&mut self, effects: &voxmar_engine::effect::EffectRegistry) -> Vec<String> {
        effects.ids().map(|id| effects.get(id)).filter(|def| self.read(&format!("ui/effect/{}", def.icon)).is_none()).map(|def| format!("{} (ui/effect/{})", def.name, def.icon)).collect()
    }

    /// Reads `<path>.png` from the first pack that has it.
    pub fn read(&mut self, path: &str) -> Option<Rgba> {
        self.packs.iter_mut().find_map(|pack| pack.read(path))
    }

    /// How the packs hold things in the hand (`held.ron`): each pack's book, the highest
    /// priority first.
    pub fn held_books(&self) -> Vec<(String, crate::held::Book)> {
        self.packs.iter().filter_map(|pack| Some((pack.name.clone(), pack.held.clone()?))).collect()
    }

    /// For each block texture reference, the pack that draws it (by name) and whether the
    /// game ships that pack; none for a placeholder.
    pub fn servers(&self, refs: &[TextureRef]) -> Vec<Option<(String, bool)>> {
        refs.iter()
            .map(|r| {
                let path = format!("block/{}", r.name);
                self.packs.iter().find(|pack| pack.contains(&path)).map(|pack| (pack.name.clone(), pack.shipped))
            })
            .collect()
    }

    /// One square texture per block texture reference, all `size`×`size`, its first frame
    /// if it is an animation strip, and the rest of each strip's frames. Logs how many each
    /// pack provided.
    pub fn block_textures(&mut self, refs: &[TextureRef]) -> (u32, Vec<Rgba>, Vec<Option<Animation>>) {
        let loaded: Vec<Option<Rgba>> = refs.iter().map(|r| self.read(&format!("block/{}", r.name))).collect();
        // What a texture is drawn over (an ore's host rock), from whichever pack has it.
        let unders: Vec<Option<Rgba>> =
            refs.iter().map(|r| r.under.as_ref().and_then(|under| self.read(&format!("block/{under}")))).collect();
        let served: Vec<String> = self
            .packs
            .iter_mut()
            .map(|pack| format!("{} from {}", std::mem::take(&mut pack.served), pack.name))
            .chain(Some(format!("{} placeholders", loaded.iter().filter(|t| t.is_none()).count())))
            .collect();
        bevy::log::info!("block textures: {}", served.join(", "));
        let size = loaded.iter().chain(&unders).flatten().map(|t| t.width.min(t.height)).max().unwrap_or(16).next_power_of_two();
        // A strip without a rock under it animates (an ore over its host keeps its first
        // frame), each frame for the time its pack's .mcmeta gives.
        let animations = loaded
            .iter()
            .zip(&unders)
            .zip(refs)
            .map(|((texture, under), r)| {
                let frames = texture.as_ref().filter(|_| under.is_none())?.frames();
                (frames.len() > 1).then(|| {
                    let path = format!("block/{}", r.name);
                    let ticks = self.packs.iter_mut().find(|pack| pack.contains(&path)).and_then(|pack| pack.frame_ticks(&path));
                    Animation {
                        frames: frames[1..]
                            .iter()
                            .map(|frame| match r.tint {
                                Some(tint) => frame.square(size).multiplied(tint),
                                None => frame.square(size),
                            })
                            .collect(),
                        ticks: ticks.unwrap_or(FRAME_TICKS),
                    }
                })
            })
            .collect();
        let textures = loaded
            .into_iter()
            .zip(unders)
            .zip(refs)
            .map(|((texture, under), r)| {
                let drawn = match (texture, under) {
                    (Some(t), Some(under)) => t.square(size).over(&under.square(size)),
                    (Some(t), None) => t.square(size),
                    (None, _) if r.render == RenderKind::Plant => plant_placeholder(size, r.color, &r.name),
                    (None, _) => placeholder(size, r.color, &r.name),
                };
                match r.tint {
                    Some(tint) => drawn.multiplied(tint),
                    None => drawn,
                }
            })
            .collect();
        (size, textures, animations)
    }

    /// For each texture reference, the other drawings its pack declares for it in
    /// `variants.ron`, `size`×`size`: each block face shows the texture or one of these,
    /// picked by its position, all equally often (a name listed twice comes up twice as
    /// often). Only the pack that serves a texture can give it variants, so they match it.
    /// None for an ore over its rock or an animation strip.
    pub fn block_variants(&mut self, refs: &[TextureRef], size: u32) -> Vec<Vec<Rgba>> {
        refs.iter()
            .map(|r| {
                let path = format!("block/{}", r.name);
                if r.under.is_some() {
                    return Vec::new();
                }
                let Some(pack) = self.packs.iter_mut().find(|pack| pack.contains(&path)) else { return Vec::new() };
                let Some(names) = pack.variants.get(&path).cloned() else { return Vec::new() };
                let mut alternates = Vec::new();
                for name in names {
                    match pack.read(&name) {
                        Some(image) if image.frames().len() == 1 => {
                            let drawn = image.square(size);
                            alternates.push(match r.tint {
                                Some(tint) => drawn.multiplied(tint),
                                None => drawn,
                            });
                        }
                        Some(_) => bevy::log::warn!("texture pack {}: variant {name} of {path} is an animation strip; leaving it out", pack.name),
                        None => bevy::log::warn!("texture pack {}: variant {name} of {path} not found", pack.name),
                    }
                }
                alternates
            })
            .collect()
    }

    /// For each texture reference `wanted`, its overlay, `block/<name>_overlay`, drawn over
    /// it tinted (a grass block side's fringe of grass, which Minecraft colours like the
    /// top), from the same pack as the texture so the two match; `size`×`size`.
    pub fn overlays(&mut self, refs: &[TextureRef], size: u32, wanted: impl Fn(usize) -> bool) -> Vec<Option<Rgba>> {
        refs.iter()
            .enumerate()
            .map(|(i, r)| {
                let path = format!("block/{}", r.name);
                let pack = self.packs.iter_mut().find(|pack| wanted(i) && pack.contains(&path))?;
                pack.read(&format!("{path}_overlay")).map(|overlay| overlay.square(size))
            })
            .collect()
    }

    /// Grass and foliage colormaps (256×256), indexed by temperature and humidity.
    pub fn colormaps(&mut self) -> [Rgba; 2] {
        let mut load = |name: &str, fallback: [u8; 3]| {
            self.read(&format!("colormap/{name}"))
                .filter(|t| t.width == 256 && t.height == 256)
                .unwrap_or_else(|| gradient_colormap(fallback))
        };
        [load("grass", [110, 180, 70]), load("foliage", [80, 160, 50])]
    }

    /// The empty hand's textures (`held.ron`'s `bare`), `size`×`size` each; a skin-coloured
    /// placeholder for one no pack draws.
    pub fn held_textures(&mut self, names: &[String], size: u32) -> Vec<Rgba> {
        names
            .iter()
            .map(|name| {
                self.read(name).map(|t| t.square(size)).unwrap_or_else(|| {
                    bevy::log::warn!("{}: no pack draws `{name}` for the empty hand", crate::held::FILE);
                    placeholder(size, [196, 146, 108], name)
                })
            })
            .collect()
    }

    /// A HUD sprite, or a flat fallback.
    pub fn sprite(&mut self, name: &str, fallback: (u32, u32, [u8; 4])) -> Rgba {
        self.read(&format!("gui/sprites/hud/{name}")).unwrap_or_else(|| Rgba::solid(fallback.0, fallback.1, fallback.2))
    }
}

/// For each block texture reference, the name of an opaque block one of whose faces it is,
/// if any (the first by id).
pub fn opaque_faces(registry: &BlockRegistry) -> Vec<Option<String>> {
    let mut opaque = vec![None; registry.textures().len()];
    for id in 0..registry.len() {
        let block = BlockId(id as u16);
        let info = registry.info(block);
        if info.render == RenderKind::Opaque {
            for layer in info.layers {
                opaque[layer as usize].get_or_insert_with(|| registry.def(block).name.clone());
            }
        }
    }
    opaque
}

/// Warnings for packs we don't ship, at most one each, about an opaque block's face that is
/// more than half see-through (a cable drawn on clear texels for a full cube): its clear
/// texels are filled with its average colour, and the warning says which. A face with a
/// few clear texels (a cactus side's margins) is filled without a word. By texture
/// reference: the textures, the opaque block each is a face of, and the pack drawing it.
pub fn see_through_warnings(refs: &[TextureRef], textures: &[Rgba], opaque: &[Option<String>], servers: &[Option<(String, bool)>]) -> Vec<String> {
    let mut warned: Vec<&str> = Vec::new();
    let mut warnings = Vec::new();
    for (((r, texture), block), server) in refs.iter().zip(textures).zip(opaque).zip(servers) {
        let (Some(block), Some((pack, false))) = (block, server) else { continue };
        let clear = texture.see_through().1;
        if r.under.is_some() || clear * 2 <= texture.pixels.len() / 4 || warned.contains(&pack.as_str()) {
            continue;
        }
        warned.push(pack);
        let [red, green, blue] = texture.fill_colour();
        warnings.push(format!(
            "texture pack {pack}: block/{}, a face of the opaque block {block}, is {}% see-through; its clear texels are filled with ({red}, {green}, {blue})",
            r.name,
            clear * 100 / (texture.pixels.len() / 4).max(1),
        ));
    }
    warnings
}

pub fn find_minecraft_jar() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)?;
    let appdata = std::env::var_os("APPDATA").map(PathBuf::from);
    let mut version_dirs = vec![
        home.join("Library/Application Support/minecraft/versions"),
        home.join(".minecraft/versions"),
    ];
    let mut prism_dirs = vec![
        home.join("Library/Application Support/PrismLauncher/libraries/com/mojang/minecraft"),
        home.join(".local/share/PrismLauncher/libraries/com/mojang/minecraft"),
    ];
    if let Some(appdata) = appdata {
        version_dirs.push(appdata.join(".minecraft/versions"));
        prism_dirs.push(appdata.join("PrismLauncher/libraries/com/mojang/minecraft"));
    }

    // Both layouts are `<root>/<version>/<something>.jar`.
    let mut jars: Vec<(Vec<u32>, PathBuf)> = Vec::new();
    for root in version_dirs.iter().chain(&prism_dirs) {
        let Ok(entries) = std::fs::read_dir(root) else { continue };
        for entry in entries.flatten() {
            let version = entry.file_name().to_string_lossy().into_owned();
            let Some(numbers) = parse_release(&version) else { continue };
            let Ok(files) = std::fs::read_dir(entry.path()) else { continue };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().is_some_and(|e| e == "jar") && !path.to_string_lossy().contains("server") {
                    jars.push((numbers.clone(), path));
                }
            }
        }
    }
    jars.sort();
    jars.pop().map(|(_, path)| path)
}

/// "1.21.1" -> [1, 21, 1]; snapshots and modded versions are skipped.
fn parse_release(version: &str) -> Option<Vec<u32>> {
    version.split('.').map(|p| p.parse().ok()).collect()
}

fn name_seed(name: &str) -> u32 {
    name.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

/// Speckled placeholder in the block's colour, so materials stay distinguishable.
fn placeholder(size: u32, color: [u8; 3], name: &str) -> Rgba {
    let seed = name_seed(name);
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for i in 0..size * size {
        let mut h = (i ^ seed).wrapping_mul(0x9e37_79b9);
        h ^= h >> 15;
        let shade = 0.85 + (h % 31) as f32 / 100.0;
        pixels.extend(color.map(|c| (c as f32 * shade).min(255.0) as u8));
        pixels.push(if name.contains("leaves") && h.is_multiple_of(5) { 0 } else { 255 });
    }
    Rgba { width: size, height: size, pixels }
}

/// A tuft of blades rising from the bottom edge; flowers (any plant not named like grass,
/// a fern or a bush) get green stems and a head in the block's colour.
fn plant_placeholder(size: u32, color: [u8; 3], name: &str) -> Rgba {
    let seed = name_seed(name);
    let leafy = ["grass", "fern", "bush"].iter().any(|w| name.contains(w));
    let stem = if leafy { color } else { [70, 130, 45] };
    let mut texture = Rgba { width: size, height: size, pixels: vec![0; (size * size * 4) as usize] };
    let mut put = |x: u32, y: u32, c: [u8; 3]| {
        let i = ((y * size + x) * 4) as usize;
        texture.pixels[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
    };
    for x in 0..size {
        let h = (x ^ seed).wrapping_mul(0x9e37_79b9) >> 16;
        if h.is_multiple_of(3) {
            continue;
        }
        let top = size / 4 + h % (size / 2);
        for y in top..size {
            put(x, y, stem);
        }
    }
    if !leafy {
        let (center, radius) = (size as i32 / 2, size as i32 / 5);
        for y in 0..size as i32 {
            for x in 0..size as i32 {
                if (x - center).pow(2) + (y - size as i32 / 4).pow(2) <= radius * radius {
                    put(x as u32, y as u32, color);
                }
            }
        }
    }
    texture
}

fn gradient_colormap(base: [u8; 3]) -> Rgba {
    let mut pixels = Vec::with_capacity(256 * 256 * 4);
    for y in 0..256u32 {
        for x in 0..256u32 {
            // x: hot -> cold, y: wet -> dry. Cold and dry shift towards yellow-brown.
            let dry = (x + y) as f32 / 512.0;
            pixels.extend(base.map(|c| (c as f32 * (1.0 - 0.3 * dry) + 60.0 * dry).min(255.0) as u8));
            pixels.push(255);
        }
    }
    Rgba { width: 256, height: 256, pixels }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The look the packs we ship give to the hand (`held.ron`): the default pack's own, and
    /// Voxmar HD, which builds on it, the same.
    #[test]
    fn the_shipped_packs_hold_things_in_the_hand() {
        let packs = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/packs");
        let default = Pack::open(&packs.join("default")).expect("default pack").held.expect("the default pack's held.ron");
        let hd = Pack::open(&packs.join(HD_PACK)).expect("Voxmar HD").held.expect("Voxmar HD builds on the default pack's held.ron");
        assert_eq!(default, hd);
        default.finish().unwrap_or_else(|problems| panic!("{problems:#?}"));
    }

    /// Whether `pack`'s `texture` may be a face of the opaque block `block`: only if every texel
    /// is wholly opaque, as the packs we ship keep to (a test holds them to it). An opaque face
    /// is drawn without an alpha test (terrain.rs gives the opaque pass no `CUTOUT`), so a
    /// see-through texel would show its own colour, black as often as not, unless filled (see
    /// [`Rgba::fill_transparent`]). A block with see-through parts is a cut-out block or a shape.
    fn opaque_face(pack: &str, texture: &str, block: &str, image: &Rgba) -> Result<(), String> {
        match image.see_through().0 {
            0 => Ok(()),
            n => Err(format!("texture pack {pack}: {texture}, a face of the opaque block {block}, has {n} of {} texels see-through", image.pixels.len() / 4)),
        }
    }

    /// Voxmar HD's LV cable side as it was drawn before d69412ff: a cable across clear
    /// texels, given to a full-cube opaque block, so drawn black above and below the cable.
    fn cable_side() -> Rgba {
        let rows = std::iter::repeat_n(".".repeat(32), 10)
            .chain(["a", "b", "b", "a", "a", "c", "c", "a", "a", "a", "d", "d"].map(|c| if c == "c" { "ccccaaaa".repeat(4) } else { c.repeat(32) }))
            .chain(std::iter::repeat_n(".".repeat(32), 10));
        let pixels = rows
            .flat_map(|row| row.chars().collect::<Vec<_>>())
            .flat_map(|c| match c {
                'a' => [0x34, 0x34, 0x3a, 255],
                'b' => [0x48, 0x48, 0x4e, 255],
                'c' => [0xec, 0x6a, 0x50, 255],
                'd' => [0x14, 0x14, 0x16, 255],
                _ => [0, 0, 0, 0],
            })
            .collect();
        Rgba { width: 32, height: 32, pixels }
    }

    /// Every texture the packs we ship give an opaque block's face is wholly opaque; the
    /// check refuses one that isn't, by pack, texture and block (Voxmar HD's cable side as
    /// it was).
    #[test]
    fn our_packs_give_opaque_faces_only_opaque_texels() {
        let refusal = opaque_face("Voxmar HD", "block/lv_cable_side", "base:lv_cable", &cable_side()).unwrap_err();
        assert_eq!(refusal, "texture pack Voxmar HD: block/lv_cable_side, a face of the opaque block base:lv_cable, has 640 of 1024 texels see-through");
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets").canonicalize().expect("assets");
        let content = voxmar_server::load_content(&assets).expect("content");
        let blocks = &content.blocks;
        let opaque = opaque_faces(blocks);
        let mut refused = Vec::new();
        for pack in ["default", HD_PACK] {
            let mut pack = Pack::open(&assets.join("packs").join(pack)).expect("pack opens");
            for (r, block) in blocks.textures().iter().zip(&opaque) {
                // An ore is drawn over its rock: its own texture is clear around the ore.
                let (Some(block), None) = (block, &r.under) else { continue };
                let path = format!("block/{}", r.name);
                if let Some(texture) = pack.read(&path)
                    && let Err(e) = opaque_face(&pack.name, &path, block, &texture)
                {
                    refused.push(e);
                }
            }
        }
        assert!(refused.is_empty(), "{}", refused.join("\n"));
    }

    /// Every effect has an icon of its own in the default pack, ten texels across, none
    /// like another's, so the HUD's chips tell Camp from Cosy and Larder from Pantry (the
    /// plain buff icon did not); and the launch's check finds none missing.
    #[test]
    fn every_effect_has_an_icon_of_its_own() {
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets").canonicalize().expect("assets");
        let content = voxmar_server::load_content(&assets).expect("content");
        let effects = &content.effects;
        assert_eq!(Source::bundled(&assets.join("packs")).unpictured(effects), Vec::<String>::new());
        let mut pack = Pack::open(&assets.join("packs").join("default")).expect("default pack");
        let mut drawn: Vec<(String, Vec<u8>)> = Vec::new();
        for def in effects.ids().map(|id| effects.get(id)) {
            let image = pack.read(&format!("ui/effect/{}", def.icon)).unwrap_or_else(|| panic!("{} has no icon of its own", def.name));
            assert_eq!((image.width, image.height), (10, 10), "{}", def.name);
            if let Some((other, _)) = drawn.iter().find(|(_, pixels)| *pixels == image.pixels) {
                panic!("{} looks like {other}", def.name);
            }
            drawn.push((def.name.clone(), image.pixels));
        }
        // An effect whose icon no pack draws is named.
        let mut file = voxmar_engine::effect::EffectRegistry::read("test", r#"(effects: [(name: "unseen", kind: Buff, title: "Unseen", description: "")])"#).expect("reads");
        file.effects[0].icon = "nothing_draws_this".into();
        let unseen = voxmar_engine::effect::EffectRegistry::new(vec![file]).expect("loads");
        assert_eq!(Source::bundled(&assets.join("packs")).unpictured(&unseen), ["test:unseen (ui/effect/nothing_draws_this)"]);
    }

    /// A block's fixed tint multiplies its texels' colours by the colour (white leaves them,
    /// black blackens them) and never touches their transparency.
    #[test]
    fn a_fixed_tint_multiplies_the_colour_and_keeps_the_alpha() {
        let mut texture = Rgba::solid(2, 1, [200, 100, 50, 255]);
        texture.pixels[7] = 90;
        let tinted = texture.clone().multiplied([255, 128, 0]);
        assert_eq!(tinted.pixels, [200, 50, 0, 255, 200, 50, 0, 90]);
        assert_eq!(texture.clone().multiplied([255, 255, 255]).pixels, texture.pixels);
    }

    /// A pack's animated drawing retains a block's fixed colour after the first frame,
    /// including its translucent texels and the pack's own frame time.
    #[test]
    fn a_fixed_tint_colours_every_animation_frame_from_a_pack() {
        let root = std::env::temp_dir().join(format!("voxmar-tinted-animation-{}", std::process::id()));
        std::fs::create_dir_all(root.join("textures/block")).unwrap();
        let pixels = [[200, 100, 50, 255].repeat(4), [120, 80, 60, 90].repeat(4)].concat();
        image::RgbaImage::from_raw(2, 4, pixels).unwrap().save(root.join("textures/block/cloth.png")).unwrap();
        std::fs::write(root.join("textures/block/cloth.png.mcmeta"), r#"{"animation":{"frametime":7}}"#).unwrap();
        let mut source = Source { packs: vec![Pack::open(&root).unwrap()] };
        let refs = [Some([255, 128, 0]), None].map(|tint| TextureRef { name: "cloth".into(), under: None, color: [0; 3], render: RenderKind::Model, tint });
        let (size, textures, animations) = source.block_textures(&refs);
        assert_eq!(size, 2);
        assert_eq!(textures[0].pixels, [200, 50, 0, 255].repeat(4));
        assert_eq!(textures[1].pixels, [200, 100, 50, 255].repeat(4));
        let tinted = animations[0].as_ref().unwrap();
        assert_eq!(tinted.ticks, 7);
        assert_eq!(tinted.frames.len(), 1);
        assert_eq!(tinted.frames[0].pixels, [120, 40, 0, 90].repeat(4));
        assert_eq!(animations[1].as_ref().unwrap().frames[0].pixels, [120, 80, 60, 90].repeat(4));
        std::fs::remove_dir_all(root).unwrap();
    }

    /// Every face of the decor models resolves in both shipped looks, including the
    /// separate drawings on their boxes. HD may inherit the default recipe at 32 texels.
    /// One test a look, so the two draw side by side.
    #[test]
    fn decor_faces_resolve_in_the_default_pack() {
        decor_faces_resolve_in("default");
    }

    #[test]
    fn decor_faces_resolve_in_the_hd_pack() {
        decor_faces_resolve_in(HD_PACK);
    }

    fn decor_faces_resolve_in(name: &str) {
        let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let content = voxmar_server::load_content(&assets).expect("content");
        let blocks = &content.blocks;
        let decor: Vec<_> = blocks.ids().filter(|&id| blocks.def(id).has_tag("decor")).collect();
        assert!(decor.len() >= 172, "only {} decor blocks", decor.len());
        // Each drawing once (many decor blocks share theirs), named by the first block using it.
        let mut paths: Vec<(String, &str)> = Vec::new();
        for &id in &decor {
            for &layer in blocks.info(id).layers.iter().chain(blocks.shape_layers(id)) {
                let path = format!("block/{}", blocks.textures()[layer as usize].name);
                if !paths.iter().any(|(p, _)| *p == path) {
                    paths.push((path, blocks.def(id).name.as_str()));
                }
            }
        }
        let mut pack = Pack::open(&assets.join("packs").join(name)).expect("pack");
        for (path, block) in &paths {
            let drawn = pack.read(path).unwrap_or_else(|| panic!("{name}: {block} has no {path}"));
            assert!(drawn.pixels.chunks_exact(4).any(|p| p[3] != 0), "{name}: {path} is invisible");
        }
    }

    /// A pack we don't ship hears once of its opaque faces more than half see-through,
    /// with the colour they are filled with; a face with clear margins, or a pack we ship,
    /// hears nothing.
    #[test]
    fn a_pack_hears_once_of_see_through_opaque_faces() {
        let texture = |name: &str| TextureRef { name: name.into(), under: None, color: [0; 3], render: RenderKind::Opaque, tint: None };
        let refs = [texture("lv_cable_side"), texture("mv_cable_side"), texture("cactus_side")];
        let mut cactus = Rgba::solid(16, 16, [80, 140, 60, 255]);
        cactus.pixels[..16 * 4].fill(0);
        let textures = [cable_side(), cable_side(), cactus];
        let opaque = ["base:lv_cable", "base:mv_cable", "base:cactus"].map(|b| Some(b.to_owned()));
        let warnings = |pack: (&str, bool)| see_through_warnings(&refs, &textures, &opaque, &[Some((pack.0.to_owned(), pack.1)), Some((pack.0.to_owned(), pack.1)), Some((pack.0.to_owned(), pack.1))]);
        assert_eq!(
            warnings(("Cables Plus", false)),
            ["texture pack Cables Plus: block/lv_cable_side, a face of the opaque block base:lv_cable, is 62% see-through; its clear texels are filled with (65, 54, 57)"]
        );
        assert!(warnings(("Voxmar HD", true)).is_empty());
    }

    /// Every block texture and item icon the content uses resolves to art in the Voxmar HD
    /// look: the pack's own 32×32 art, or the default pack's (or a mod pack's) drawn as the
    /// game draws it. Only a name that no pack draws, which would show a placeholder in
    /// every look, fails here; a block landed with default art alone passes and joins the
    /// pack's queue: the names still on the fallback are written, one per line, to
    /// `target/voxmar-hd-pending.txt`, so Voxmar HD never holds up anyone's landing.
    #[test]
    fn voxmar_hd_resolves_every_texture() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("repository root");
        let assets = root.join("assets");
        let content = voxmar_server::load_content(&assets).expect("content");
        let packs = assets.join("packs");
        let mut hd = Pack::open(&packs.join(HD_PACK)).expect("Voxmar HD opens");
        assert_eq!(hd.name, "Voxmar HD");
        // What the default pack alone would draw at the pack's resolution: textures that
        // come out the same are fallbacks, not yet redrawn for Voxmar HD.
        let default = Library::load(&packs.join("default")).expect("default pack");
        let mut addons: Vec<Pack> = std::fs::read_dir(&packs)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir() && !path.join(Manifest::FILE).exists() && path.file_name() != Some("default".as_ref()))
            .filter_map(|path| Pack::open(&path).ok())
            .collect();
        let blocks = &content.blocks;
        let icons = (0..blocks.len()).filter_map(|i| blocks.def(voxmar_engine::block::BlockId(i as u16)).icon.clone()).map(|name| format!("item/{name}"));
        let mut names: Vec<String> = blocks.textures().iter().map(|r| format!("block/{}", r.name)).chain(icons).collect();
        names.sort();
        names.dedup();
        let (mut missing, mut pending) = (Vec::new(), Vec::new());
        for name in &names {
            match hd.read(name) {
                Some(texture) => {
                    let fallback = default.render_at_least(name, hd.resolution).and_then(Result::ok);
                    if fallback.is_some_and(|f| f.pixels == texture.pixels) {
                        pending.push(name.clone());
                    }
                }
                None if addons.iter_mut().any(|pack| pack.read(name).is_some()) => pending.push(name.clone()),
                None => missing.push(name.clone()),
            }
        }
        let list = root.join("target/voxmar-hd-pending.txt");
        if std::fs::create_dir_all(root.join("target")).is_ok() {
            let _ = std::fs::write(&list, pending.iter().map(|n| format!("{n}\n")).collect::<String>());
        }
        println!("Voxmar HD: {} of {} textures drawn for it, {} on the default fallback (listed in {})", names.len() - pending.len() - missing.len(), names.len(), pending.len(), list.display());
        assert!(missing.is_empty(), "no pack draws {} texture names, so they show placeholders in every look: {missing:?}", missing.len());
    }
}
