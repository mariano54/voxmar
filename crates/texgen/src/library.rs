//! A pack's recipes: every book merged, families expanded, ready to render by name.

use std::collections::BTreeMap;
use std::path::Path;

use crate::colour::{self, Rgba};
use crate::field::seed_of;
use crate::recipe::{Book, Palette, Recipe};
use crate::render::{self, Resolved};
use crate::{Error, Image};

/// How deep `base` chains may go, which also catches cycles.
const MAX_DEPTH: usize = 32;

#[derive(Debug, Default)]
pub struct Library {
    palettes: BTreeMap<String, Palette>,
    recipes: BTreeMap<String, Recipe>,
}

impl Library {
    /// Merges recipe books, given as (name for errors, RON source). Texture and palette
    /// names must be unique across books; family textures give way to explicit ones.
    pub fn from_books<'a>(books: impl IntoIterator<Item = (&'a str, &'a str)>) -> Result<Self, Error> {
        let mut library = Library::default();
        let mut templates = BTreeMap::new();
        let mut families = Vec::new();
        for (origin, source) in books {
            let book: Book = options().from_str(source).map_err(|e| Error(format!("{origin}: {e}")))?;
            for (name, palette) in book.palettes {
                if library.palettes.insert(name.clone(), palette).is_some() {
                    return Err(Error(format!("{origin}: palette `{name}` is defined twice")));
                }
            }
            for (name, recipe) in book.textures {
                if library.recipes.insert(name.clone(), recipe).is_some() {
                    return Err(Error(format!("{origin}: texture `{name}` is defined twice")));
                }
            }
            for (name, template) in book.templates {
                if templates.insert(name.clone(), template).is_some() {
                    return Err(Error(format!("{origin}: template `{name}` is defined twice")));
                }
            }
            families.extend(book.families.into_iter().map(|family| (origin.to_owned(), family)));
        }

        let mut made = BTreeMap::new();
        for (origin, family) in families {
            let template = templates
                .get(&family.template)
                .ok_or_else(|| Error(format!("{origin}: family `{}` uses unknown template `{}`", family.name, family.template)))?;
            for (pattern, recipe) in template {
                if family.without.contains(pattern) {
                    continue;
                }
                let name = pattern.replace("{name}", &family.name);
                if library.recipes.contains_key(&name) {
                    continue;
                }
                let mut recipe = recipe.clone();
                // Textures made from another of the family's own keep its seed, so a
                // leaf sprite matches its leaves.
                let own_base = recipe.base.as_ref().is_some_and(|b| b.contains("{name}"));
                recipe.base = recipe.base.map(|b| b.replace("{name}", &family.name));
                for palette in recipe.palettes.values_mut() {
                    if let Palette::Named(n) = palette {
                        *n = n.replace("{name}", &family.name);
                    }
                }
                recipe.palettes.extend(family.palettes.clone());
                if !own_base {
                    recipe.seed = family.seed.or(recipe.seed).or(Some(seed_of(&family.name)));
                }
                if made.insert(name.clone(), recipe).is_some() {
                    return Err(Error(format!("{origin}: two families make texture `{name}`")));
                }
            }
        }
        library.recipes.extend(made);
        Ok(library)
    }

    /// Reads every `recipes/*.ron` book in a pack folder (none is fine).
    pub fn load(pack: &Path) -> Result<Self, Error> {
        let dir = pack.join("recipes");
        let mut paths: Vec<_> = match std::fs::read_dir(&dir) {
            Ok(entries) => entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "ron")).collect(),
            Err(_) => Vec::new(),
        };
        paths.sort();
        let sources: Vec<(String, String)> = paths
            .iter()
            .map(|p| std::fs::read_to_string(p).map(|s| (p.display().to_string(), s)).map_err(|e| Error(format!("{}: {e}", p.display()))))
            .collect::<Result<_, _>>()?;
        Self::from_books(sources.iter().map(|(o, s)| (o.as_str(), s.as_str())))
    }

    pub fn contains(&self, name: &str) -> bool {
        self.recipes.contains_key(name)
    }

    /// Every texture name, sorted.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.recipes.keys().map(String::as_str)
    }

    /// Renders a texture, or `None` if the library has no recipe by that name.
    pub fn render(&self, name: &str) -> Option<Result<Image, Error>> {
        self.contains(name).then(|| {
            let resolved = self.resolve(name, 0)?;
            render::render(&resolved).map_err(|Error(e)| Error(format!("{name}: {e}")))
        })
    }

    fn resolve(&self, name: &str, depth: usize) -> Result<Resolved, Error> {
        if depth > MAX_DEPTH {
            return Err(Error(format!("{name}: `base` chain too deep (a cycle?)")));
        }
        let recipe = self.recipes.get(name).ok_or_else(|| Error(format!("no texture `{name}`")))?;
        let mut resolved = match &recipe.base {
            Some(base) => self.resolve(base, depth + 1).map_err(|Error(e)| Error(format!("{name}: {e}")))?,
            None => Resolved { size: (16, 16), seed: seed_of(name), palettes: BTreeMap::new(), layers: Vec::new() },
        };
        if let Some(size) = recipe.size {
            resolved.size = size;
        }
        if let Some(seed) = recipe.seed {
            resolved.seed = seed;
        }
        for (slot, palette) in &recipe.palettes {
            let shades = self.shades(palette, 0).map_err(|Error(e)| Error(format!("{name}: palette `{slot}`: {e}")))?;
            resolved.palettes.insert(slot.clone(), shades);
        }
        let first = resolved.layers.len();
        resolved.layers.extend(recipe.layers.iter().cloned().enumerate().map(|(i, layer)| (first + i, layer)));
        Ok(resolved)
    }

    fn shades(&self, palette: &Palette, depth: usize) -> Result<Vec<Rgba>, Error> {
        match palette {
            Palette::Shades(list) if list.is_empty() => Err(Error("a palette needs at least one shade".into())),
            Palette::Shades(list) => list.iter().map(|c| colour::parse(c)).collect(),
            Palette::Ramp(ramp) => colour::ramp(ramp),
            Palette::Named(_) if depth > MAX_DEPTH => Err(Error("palette names refer to each other in a cycle".into())),
            Palette::Named(name) => {
                let palette = self.palettes.get(name).ok_or_else(|| Error(format!("no palette `{name}`")))?;
                self.shades(palette, depth + 1)
            }
        }
    }
}

/// Layers and terms are written `Paint(palette: "stone")` rather than
/// `Paint((palette: "stone"))`, and optional values without `Some(…)`.
fn options() -> ron::Options {
    ron::Options::default().with_default_extension(ron::extensions::Extensions::UNWRAP_VARIANT_NEWTYPES | ron::extensions::Extensions::IMPLICIT_SOME)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOOK: &str = r##"(
        palettes: {
            "grey": ["#202020", "#606060", "#a0a0a0", "#e0e0e0"],
            "red": (ramp: "#c03020", shades: 4),
        },
        textures: {
            "block/rock": (
                palettes: {"main": "grey"},
                layers: [Paint(palette: "main", shade: [Noise(cells: (4, 4), amount: 0.6)], clean: true)],
            ),
            "block/red_rock": (base: "block/rock", palettes: {"main": "red"}),
            "block/ore": (
                base: "block/rock",
                palettes: {"ore": ["#000000", "#ffff00", "#ffffff"]},
                layers: [Clusters(palette: "ore", count: 3, size: (2, 4), shadow: -1)],
            ),
            "block/flower": (
                palettes: {"petal": "red"},
                layers: [Grid(
                    legend: {'p': "petal:1", 'g': "#30a030", 'x': "clear"},
                    rows: ["..p..", ".ppp.", "..g..", "..g.."],
                    at: [(5, 10)],
                )],
            ),
            "block/oak_planks": (
                palettes: {"wood": ["#403020", "#705030", "#906a40", "#b08850"]},
                layers: [Paint(palette: "wood", shade: [Boards()])],
            ),
            "block/maple_planks": (base: "block/oak_planks", palettes: {"wood": "red"}, seed: 9),
        },
        templates: {
            "wood": {
                "block/{name}_planks": (base: "block/oak_planks"),
                "block/{name}_planks_dark": (base: "block/{name}_planks", layers: [Shift(by: [Flat(-1.0)])]),
            },
        },
        families: [
            (template: "wood", name: "maple", palettes: {"wood": "grey"}),
            (template: "wood", name: "pine", palettes: {"wood": "grey"}),
        ],
    )"##;

    fn library() -> Library {
        Library::from_books([("test", BOOK)]).unwrap()
    }

    #[test]
    fn renders_deterministically() {
        let library = library();
        for name in library.names() {
            let a = library.render(name).unwrap().unwrap();
            let b = library.render(name).unwrap().unwrap();
            assert_eq!(a, b, "{name}");
            assert_eq!((a.width, a.height), (16, 16));
        }
        assert!(library.render("block/nothing").is_none());
    }

    #[test]
    fn derived_textures_keep_the_pattern() {
        let library = library();
        let rock = library.render("block/rock").unwrap().unwrap();
        let red = library.render("block/red_rock").unwrap().unwrap();
        let ore = library.render("block/ore").unwrap().unwrap();
        // Same pattern, different colours: equal rock texels are equal red texels.
        for i in 0..256 {
            for j in 0..256 {
                assert_eq!(rock.pixels[i * 4..i * 4 + 3] == rock.pixels[j * 4..j * 4 + 3], red.pixels[i * 4..i * 4 + 3] == red.pixels[j * 4..j * 4 + 3]);
            }
        }
        // The ore is the rock with some texels changed.
        let changed = (0..256).filter(|i| rock.pixels[i * 4..i * 4 + 4] != ore.pixels[i * 4..i * 4 + 4]).count();
        assert!(changed > 4 && changed < 64, "{changed}");
    }

    #[test]
    fn grids_paint_only_their_characters() {
        let flower = library().render("block/flower").unwrap().unwrap();
        assert_eq!(flower.get(7, 10)[3], 255);
        assert_eq!(flower.get(7, 13), [0x30, 0xa0, 0x30, 255]);
        assert_eq!(flower.get(6, 10)[3], 0);
        assert_eq!(flower.get(0, 0)[3], 0);
    }

    #[test]
    fn families_give_way_to_explicit_textures() {
        let library = library();
        assert!(library.contains("block/pine_planks") && library.contains("block/pine_planks_dark"));
        // `maple_planks` is explicit, so it keeps its red palette rather than the family's grey.
        let maple = library.render("block/maple_planks").unwrap().unwrap();
        assert!(maple.pixels.chunks(4).any(|p| p[0] > p[2] + 40));
        let dark = library.render("block/pine_planks_dark").unwrap().unwrap();
        let pine = library.render("block/pine_planks").unwrap().unwrap();
        assert_ne!(dark, pine);
    }

    #[test]
    fn reports_mistakes() {
        assert!(Library::from_books([("a", "(textures: {\"x\": (layers: [Paint(palette: \"none\")])})")]).unwrap().render("x").unwrap().is_err());
        assert!(Library::from_books([("a", "(textures: {\"x\": (base: \"x\")})")]).unwrap().render("x").unwrap().is_err());
        assert!(Library::from_books([("a", "(textures: {\"x\": (bogus: 1)})")]).is_err());
        assert!(Library::from_books([("a", "(textures: {\"x\": ()})"), ("b", "(textures: {\"x\": ()})")]).is_err());
    }
}
