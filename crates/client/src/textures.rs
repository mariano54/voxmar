//! Texture sources. Block, colormap and HUD textures are read at runtime from the
//! player's own Minecraft install or a resource pack (zip, jar or folder); nothing from
//! Minecraft is shipped. Without one, simple placeholder textures are generated.

use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

use voxmar_engine::block::TextureRef;

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

pub enum Source {
    Zip(zip::ZipArchive<File>),
    Folder(PathBuf),
    None,
}

impl Source {
    /// `$VOXMAR_TEXTURES` (a jar, zip or folder), else the newest Minecraft client jar
    /// found in a standard launcher location.
    pub fn discover() -> Self {
        let path = std::env::var_os("VOXMAR_TEXTURES").map(PathBuf::from).or_else(find_minecraft_jar);
        match path {
            Some(path) if path.is_dir() => {
                bevy::log::info!("textures from {}", path.display());
                Source::Folder(path)
            }
            Some(path) => match File::open(&path).map_err(|e| e.to_string()).and_then(|f| {
                zip::ZipArchive::new(f).map_err(|e| e.to_string())
            }) {
                Ok(archive) => {
                    bevy::log::info!("textures from {}", path.display());
                    Source::Zip(archive)
                }
                Err(e) => {
                    bevy::log::warn!("cannot open {}: {e}; using placeholder textures", path.display());
                    Source::None
                }
            },
            None => {
                bevy::log::info!("no Minecraft install or VOXMAR_TEXTURES found; using placeholder textures");
                Source::None
            }
        }
    }

    /// Reads `assets/minecraft/textures/<path>.png`.
    pub fn read(&mut self, path: &str) -> Option<Rgba> {
        let path = format!("assets/minecraft/textures/{path}.png");
        let bytes = match self {
            Source::Zip(archive) => {
                let mut file = archive.by_name(&path).ok()?;
                let mut bytes = Vec::with_capacity(file.size() as usize);
                file.read_to_end(&mut bytes).ok()?;
                bytes
            }
            Source::Folder(root) => std::fs::read(root.join(&path)).ok()?,
            Source::None => return None,
        };
        decode_png(&bytes)
    }

    /// One square texture per block texture reference, all `size`×`size`.
    pub fn block_textures(&mut self, refs: &[TextureRef]) -> (u32, Vec<Rgba>) {
        let loaded: Vec<Option<Rgba>> = refs.iter().map(|r| self.read(&format!("block/{}", r.name))).collect();
        let size = loaded.iter().flatten().map(|t| t.width.min(t.height)).max().unwrap_or(16).next_power_of_two();
        let textures = loaded
            .into_iter()
            .zip(refs)
            .map(|(texture, r)| match texture {
                Some(t) => t.square(size),
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

fn decode_png(bytes: &[u8]) -> Option<Rgba> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buffer).ok()?;
    let data = &buffer[..info.buffer_size()];
    let pixels = match info.color_type {
        png::ColorType::Rgba => data.to_vec(),
        png::ColorType::Rgb => data.chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => data.chunks_exact(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => data.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some(Rgba { width: info.width, height: info.height, pixels })
}

/// Speckled placeholder in the block's colour, so materials stay distinguishable.
fn placeholder(size: u32, color: [u8; 3], name: &str) -> Rgba {
    let seed = name.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193));
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for i in 0..size * size {
        let mut h = (i ^ seed).wrapping_mul(0x9e37_79b9);
        h ^= h >> 15;
        let shade = 0.85 + (h % 31) as f32 / 100.0;
        pixels.extend(color.map(|c| (c as f32 * shade).min(255.0) as u8));
        pixels.push(if name.contains("leaves") && h % 5 == 0 { 0 } else { 255 });
    }
    Rgba { width: size, height: size, pixels }
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
