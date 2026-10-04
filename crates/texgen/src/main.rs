//! `texgen`: renders a texture pack's recipes for review.
//!
//! ```text
//! texgen check  [--pack DIR]                        render every recipe, report errors
//! texgen sheet  [--pack DIR] [--only A,B] [--compare PACK] [--columns N] [--zoom PX] [--out FILE]
//! texgen export [--pack DIR] --out DIR              write every recipe as a PNG
//! ```
//!
//! The pack defaults to `assets/packs/default`. `sheet` draws every texture (recipes and
//! PNGs) whose name contains one of `--only`'s words, large, tiled 3×3 and at a distance;
//! `--compare` adds the same name from another pack (a folder, zip or jar) alongside.

use std::io::Read;
use std::path::{Path, PathBuf};

use voxmar_texgen::sheet::{self, Entry};
use voxmar_texgen::{Image, Library};

struct Args {
    command: String,
    pack: PathBuf,
    only: Vec<String>,
    compare: Option<PathBuf>,
    columns: u32,
    zoom: u32,
    out: Option<PathBuf>,
}

fn parse() -> Result<Args, String> {
    let mut raw = std::env::args().skip(1);
    let command = raw.next().ok_or("usage: texgen check|sheet|export [options]")?;
    let mut args = Args { command, pack: "assets/packs/default".into(), only: Vec::new(), compare: None, columns: 4, zoom: 96, out: None };
    while let Some(arg) = raw.next() {
        let mut value = || raw.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--pack" => args.pack = value()?.into(),
            "--only" => args.only = value()?.split(',').map(str::to_owned).collect(),
            "--compare" => args.compare = Some(value()?.into()),
            "--columns" => args.columns = value()?.parse().map_err(|e| format!("--columns: {e}"))?,
            "--zoom" => args.zoom = value()?.parse().map_err(|e| format!("--zoom: {e}"))?,
            "--out" | "-o" => args.out = Some(value()?.into()),
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    Ok(args)
}

fn main() {
    if let Err(e) = run() {
        eprintln!("texgen: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = parse()?;
    let library = Library::load(&args.pack).map_err(|e| e.to_string())?;
    match args.command.as_str() {
        "check" => {
            let mut failed = 0;
            let mut count = 0;
            for name in library.names() {
                count += 1;
                if let Some(Err(e)) = library.render(name) {
                    eprintln!("{e}");
                    failed += 1;
                }
            }
            println!("{count} recipes, {failed} failed");
            if failed > 0 {
                return Err("some recipes failed".into());
            }
        }
        "sheet" => {
            let mut reference = args.compare.as_deref().map(Reference::open).transpose()?;
            let mut entries = Vec::new();
            for (name, image) in textures(&library, &args.pack)? {
                if !args.only.is_empty() && !args.only.iter().any(|w| name.contains(w.as_str())) {
                    continue;
                }
                let reference = reference.as_mut().and_then(|r| r.read(&name));
                entries.push(Entry { name, image, reference });
            }
            if entries.is_empty() {
                return Err("no textures match".into());
            }
            let out = args.out.unwrap_or_else(|| "target/texgen/sheet.png".into());
            write(&out, &sheet::contact_sheet(&entries, args.columns, args.zoom))?;
            println!("{} textures -> {}", entries.len(), out.display());
        }
        "export" => {
            let out = args.out.ok_or("export needs --out DIR")?;
            let mut count = 0;
            for name in library.names() {
                let image = library.render(name).unwrap().map_err(|e| e.to_string())?;
                write(&out.join(format!("{name}.png")), &image)?;
                count += 1;
            }
            println!("{count} textures -> {}", out.display());
        }
        other => return Err(format!("unknown command {other}")),
    }
    Ok(())
}

/// Every texture in the pack by name: rendered recipes and PNGs (a PNG wins).
fn textures(library: &Library, pack: &Path) -> Result<Vec<(String, Image)>, String> {
    let mut all = std::collections::BTreeMap::new();
    for name in library.names() {
        all.insert(name.to_owned(), library.render(name).unwrap().map_err(|e| e.to_string())?);
    }
    let root = pack.join("textures");
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "png") {
                let name = path.strip_prefix(&root).unwrap().with_extension("").to_string_lossy().replace('\\', "/");
                let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                let image = Image::decode_png(&bytes).ok_or_else(|| format!("{}: not a PNG", path.display()))?;
                all.insert(name, image);
            }
        }
    }
    Ok(all.into_iter().collect())
}

fn write(path: &Path, image: &Image) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, image.encode_png()).map_err(|e| format!("{}: {e}", path.display()))
}

/// Another pack to compare against: a folder, or a zip or jar.
enum Reference {
    Folder(PathBuf),
    Zip(zip::ZipArchive<std::fs::File>),
}

impl Reference {
    fn open(path: &Path) -> Result<Self, String> {
        if path.is_dir() {
            return Ok(Reference::Folder(path.to_owned()));
        }
        let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        zip::ZipArchive::new(file).map(Reference::Zip).map_err(|e| format!("{}: {e}", path.display()))
    }

    fn read(&mut self, name: &str) -> Option<Image> {
        ["textures", "assets/minecraft/textures"].iter().find_map(|root| {
            let path = format!("{root}/{name}.png");
            let bytes = match self {
                Reference::Folder(dir) => std::fs::read(dir.join(&path)).ok()?,
                Reference::Zip(archive) => {
                    let mut file = archive.by_name(&path).ok()?;
                    let mut bytes = Vec::new();
                    file.read_to_end(&mut bytes).ok()?;
                    bytes
                }
            };
            let image = Image::decode_png(&bytes)?;
            // Animation strips: the first frame.
            let mut frame = Image::new(image.width, image.height.min(image.width));
            frame.draw(&image, 0, 0, 1);
            Some(frame)
        })
    }
}
