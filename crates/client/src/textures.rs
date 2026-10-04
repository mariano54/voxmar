//! Texture sources. Block, colormap and HUD textures are read at runtime from a stack of
//! texture packs: our own default pack, the player's chosen pack, and their Minecraft
//! install if they have one (nothing from Minecraft is shipped or needed). Whatever no pack
//! provides gets a simple generated placeholder.

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use voxmar_engine::block::{RenderKind, TextureRef};
use voxmar_texgen::{Image, Library};

#[derive(Clone)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Rgba {
    fn solid(width: u32, height: u32, color: [u8; 4]) -> Self {
        Self { width, height, pixels: color.repeat((width * height) as usize) }
    }

    /// Makes every texel opaque, giving transparent ones the average opaque colour. For
    /// textures drawn on full cubes (a cactus side has transparent margins in Minecraft's
    /// model), which would otherwise show holes at the edges.
    pub fn fill_transparent(&mut self) {
        let (mut sum, mut count) = ([0u32; 3], 0u32);
        for pixel in self.pixels.as_chunks::<4>().0.iter().filter(|p| p[3] >= 128) {
            for c in 0..3 {
                sum[c] += pixel[c] as u32;
            }
            count += 1;
        }
        let average = sum.map(|s| s.checked_div(count).unwrap_or(0) as u8);
        for pixel in self.pixels.as_chunks_mut::<4>().0 {
            if pixel[3] < 128 {
                pixel[..3].copy_from_slice(&average);
            }
            pixel[3] = 255;
        }
    }

    /// The top square of an image (first frame of an animation strip), nearest-resampled
    /// to `size`.
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

/// One texture pack: a folder, zip or jar with images at `textures/<path>.png` (our
/// layout) or `assets/minecraft/textures/<path>.png` (Minecraft's, so its client jar and
/// resource packs work unchanged), and texture recipes in `recipes/*.ron`, rendered here
/// as the game loads (see the texgen crate). A PNG beats a recipe of the same name.
struct Pack {
    name: String,
    files: Files,
    recipes: Library,
    /// Textures this pack provided since the last log.
    served: usize,
}

impl Pack {
    fn open(path: &Path) -> Result<Self, String> {
        let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
        let mut files = Files::open(path)?;
        let books = files.recipe_books();
        let recipes = Library::from_books(books.iter().map(|(n, s)| (n.as_str(), s.as_str()))).unwrap_or_else(|e| {
            bevy::log::warn!("texture pack {name}: {e}; using its images only");
            Library::default()
        });
        Ok(Self { name, files, recipes, served: 0 })
    }

    fn read(&mut self, path: &str) -> Option<Rgba> {
        let image = ["textures", "assets/minecraft/textures"]
            .iter()
            .find_map(|root| self.files.read(&format!("{root}/{path}.png")))
            .and_then(|bytes| Image::decode_png(&bytes))
            .or_else(|| match self.recipes.render(path)? {
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
///    overrides ours;
/// 2. our default pack, `assets/packs/default`;
/// 3. any other pack in `assets/packs`, in name order (how mods ship art: they add
///    textures, but cannot replace ours);
/// 4. without a chosen pack, the newest Minecraft client jar found in a standard launcher
///    location, to fill in names the packs above lack.
pub struct Source {
    packs: Vec<Pack>,
}

impl Source {
    pub fn discover() -> Self {
        let assets = voxmar_server::assets_dir();
        let mut packs = Vec::new();
        let chosen = std::env::var_os("VOXMAR_TEXTURES").map(PathBuf::from);
        if let Some(path) = &chosen {
            match Pack::open(path) {
                Ok(pack) => packs.push(pack),
                Err(e) => bevy::log::warn!("cannot open texture pack {}: {e}", path.display()),
            }
        }
        let root = assets.join("packs");
        let mut bundled: Vec<PathBuf> = std::fs::read_dir(&root)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        bundled.sort_by_key(|path| (path.file_name() != Some("default".as_ref()), path.clone()));
        packs.extend(bundled.iter().filter_map(|path| Pack::open(path).ok()));
        if chosen.is_none()
            && let Some(jar) = find_minecraft_jar()
        {
            match Pack::open(&jar) {
                Ok(pack) => packs.push(pack),
                Err(e) => bevy::log::warn!("cannot open {}: {e}", jar.display()),
            }
        }
        Self { packs }
    }

    /// Reads `<path>.png` from the first pack that has it.
    pub fn read(&mut self, path: &str) -> Option<Rgba> {
        self.packs.iter_mut().find_map(|pack| pack.read(path))
    }

    /// One square texture per block texture reference, all `size`×`size`. Logs how many
    /// each pack provided.
    pub fn block_textures(&mut self, refs: &[TextureRef]) -> (u32, Vec<Rgba>) {
        let loaded: Vec<Option<Rgba>> = refs.iter().map(|r| self.read(&format!("block/{}", r.name))).collect();
        let served: Vec<String> = self
            .packs
            .iter_mut()
            .map(|pack| format!("{} from {}", std::mem::take(&mut pack.served), pack.name))
            .chain(Some(format!("{} placeholders", loaded.iter().filter(|t| t.is_none()).count())))
            .collect();
        bevy::log::info!("block textures: {}", served.join(", "));
        let size = loaded.iter().flatten().map(|t| t.width.min(t.height)).max().unwrap_or(16).next_power_of_two();
        let textures = loaded
            .into_iter()
            .zip(refs)
            .map(|(texture, r)| match texture {
                Some(t) => t.square(size),
                None if r.render == RenderKind::Plant => plant_placeholder(size, r.color, &r.name),
                None => placeholder(size, r.color, &r.name),
            })
            .collect();
        (size, textures)
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

    /// A HUD sprite, or a flat fallback.
    pub fn sprite(&mut self, name: &str, fallback: (u32, u32, [u8; 4])) -> Rgba {
        self.read(&format!("gui/sprites/hud/{name}")).unwrap_or_else(|| Rgba::solid(fallback.0, fallback.1, fallback.2))
    }
}

fn find_minecraft_jar() -> Option<PathBuf> {
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
