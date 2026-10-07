//! A pack's recipes: every book merged, families expanded, ready to render by name.

use std::collections::BTreeMap;
use std::path::Path;

use crate::colour::{self, Rgba};
use crate::field::{hash, seed_of};
use crate::recipe::{Book, Family, Layer, Palette, Recipe};
use crate::render::{self, Resolved};
use crate::scale;
use crate::{Error, Image, Manifest};

/// How deep `base` chains may go, which also catches cycles.
const MAX_DEPTH: usize = 32;

#[derive(Debug, Default)]
pub struct Library {
    palettes: BTreeMap<String, Palette>,
    recipes: BTreeMap<String, Recipe>,
    /// The layer each recipe in `recipes` comes from: 0 for the pack everything builds on,
    /// one more for each pack over it.
    levels: BTreeMap<String, usize>,
    /// The recipes later layers replaced, by name, each with its layer, earliest first: what
    /// a lower layer's book saw when it was written (see [`Library::size_seen_at`]).
    replaced: BTreeMap<String, Vec<(usize, Recipe)>>,
}

impl Library {
    /// Like [`Self::from_books`], but a book that cannot be read is left out (its error
    /// returned) instead of failing them all, so one mistake in a pack, or a book written
    /// for a newer texgen, costs only that book's textures.
    pub fn from_books_lenient<'a>(books: impl IntoIterator<Item = (&'a str, &'a str)>) -> (Self, Vec<Error>) {
        Self::from_layers_lenient([books.into_iter().collect::<Vec<_>>()])
    }

    /// [`Self::from_layers`], leaving out books that cannot be read (see
    /// [`Self::from_books_lenient`]).
    pub fn from_layers_lenient<'a>(layers: impl IntoIterator<Item = Vec<(&'a str, &'a str)>>) -> (Self, Vec<Error>) {
        let mut errors = Vec::new();
        let layers: Vec<Vec<(&str, &str)>> = layers
            .into_iter()
            .map(|books| {
                books
                    .into_iter()
                    .filter(|&(origin, source)| match options().from_str::<Book>(source) {
                        Ok(_) => true,
                        Err(e) => {
                            errors.push(Error(format!("{origin}: {e}")));
                            false
                        }
                    })
                    .collect()
            })
            .collect();
        match Self::from_layers(layers) {
            Ok(library) => (library, errors),
            Err(e) => (Library::default(), errors.into_iter().chain([e]).collect()),
        }
    }

    /// Merges recipe books, given as (name for errors, RON source). Texture and palette
    /// names must be unique across books; family textures give way to explicit ones.
    pub fn from_books<'a>(books: impl IntoIterator<Item = (&'a str, &'a str)>) -> Result<Self, Error> {
        Self::from_layers([books.into_iter().collect::<Vec<_>>()])
    }

    /// Merges layers of recipe books, each as [`Self::from_books`] takes them: a pack's
    /// own books over the books of the pack it extends. Within a layer every name must be
    /// unique; a later layer's palette or texture replaces an earlier one of the same
    /// name, and its template members replace those of the same pattern, so a
    /// high-resolution pack can redraw every wood's planks by replacing one template
    /// member, or recolour every texture made with a palette by replacing the palette.
    /// Explicit textures still beat family textures, from whichever layer.
    pub fn from_layers<'a>(layers: impl IntoIterator<Item = Vec<(&'a str, &'a str)>>) -> Result<Self, Error> {
        let mut library = Library::default();
        let mut templates = BTreeMap::new();
        let mut families = Vec::new();
        for (level, books) in layers.into_iter().enumerate() {
            let mut palettes_here = std::collections::BTreeSet::new();
            let mut textures_here = std::collections::BTreeSet::new();
            let mut templates_here = std::collections::BTreeSet::new();
            for (origin, source) in books {
                let book: Book = options().from_str(source).map_err(|e| Error(format!("{origin}: {e}")))?;
                for (name, palette) in book.palettes {
                    if !palettes_here.insert(name.clone()) {
                        return Err(Error(format!("{origin}: palette `{name}` is defined twice")));
                    }
                    library.palettes.insert(name, palette);
                }
                let with_legend = |mut recipe: Recipe| {
                    for layer in &mut recipe.layers {
                        if let Layer::Grid(grid) = layer {
                            for (&ch, ink) in &book.legend {
                                grid.legend.entry(ch).or_insert_with(|| ink.clone());
                            }
                        }
                    }
                    recipe
                };
                let templates_with_legend: Vec<(String, BTreeMap<String, Recipe>)> = book
                    .templates
                    .into_iter()
                    .map(|(name, members)| (name, members.into_iter().map(|(n, r)| (n, with_legend(r))).collect()))
                    .collect();
                for (name, recipe) in book.textures {
                    let recipe = with_legend(recipe);
                    for layer in &recipe.layers {
                        if let Layer::Grid(grid) = layer
                            && let Some(row) = grid.rows.iter().find(|r| r.chars().count() != grid.rows[0].chars().count())
                        {
                            return Err(Error(format!("{origin}: texture `{name}`: grid row `{row}` is not as long as the first")));
                        }
                    }
                    if !textures_here.insert(name.clone()) {
                        return Err(Error(format!("{origin}: texture `{name}` is defined twice")));
                    }
                    library.put(name, level, recipe);
                }
                for (name, template) in templates_with_legend {
                    if !templates_here.insert(name.clone()) {
                        return Err(Error(format!("{origin}: template `{name}` is defined twice")));
                    }
                    // A later layer redraws a template's members one by one: those it
                    // names replace the earlier ones, the rest stay. Each member keeps the
                    // drawings it replaced, with their layers, for what earlier layers saw.
                    let members = templates.entry(name).or_insert_with(BTreeMap::new);
                    for (pattern, recipe) in template {
                        members.entry(pattern).or_insert_with(Vec::new).push((level, recipe));
                    }
                }
                families.extend(book.families.into_iter().map(|family| (level, origin.to_owned(), family)));
            }
        }

        // Each family is made with the template as finally replaced; a later layer's
        // family replaces an earlier layer's texture of the same name. A texture made from
        // a member a later layer redrew is that later layer's, and the one made from the
        // member it replaced is kept as what the layers below saw.
        let mut made: BTreeMap<String, (usize, Vec<(usize, Recipe)>)> = BTreeMap::new();
        for (level, origin, family) in families {
            let template = templates
                .get(&family.template)
                .ok_or_else(|| Error(format!("{origin}: family `{}` uses unknown template `{}`", family.name, family.template)))?;
            for (pattern, drawings) in template {
                if family.without.contains(pattern) {
                    continue;
                }
                let name = pattern.replace("{name}", &family.name);
                if library.recipes.contains_key(&name) {
                    continue;
                }
                let mut versions: Vec<(usize, Recipe)> = Vec::new();
                for (drawn, recipe) in drawings {
                    let made_at = level.max(*drawn);
                    if versions.last().is_some_and(|(at, _)| *at == made_at) {
                        versions.pop();
                    }
                    versions.push((made_at, family_member(recipe, &family)));
                }
                match made.get(&name) {
                    Some((earlier, _)) if *earlier == level => {
                        return Err(Error(format!("{origin}: two families make texture `{name}`")));
                    }
                    Some((earlier, _)) if *earlier > level => continue,
                    _ => {
                        made.insert(name, (level, versions));
                    }
                }
            }
        }
        for (name, (_, versions)) in made {
            for (level, recipe) in versions {
                library.put(name.clone(), level, recipe);
            }
        }
        Ok(library)
    }

    /// Adds a recipe from `level`, keeping the one it replaces as what lower layers saw.
    fn put(&mut self, name: String, level: usize, recipe: Recipe) {
        if let Some(old) = self.recipes.insert(name.clone(), recipe) {
            let old_level = self.levels[&name];
            self.replaced.entry(name.clone()).or_default().push((old_level, old));
        }
        self.levels.insert(name, level);
    }

    /// Reads every `recipes/*.ron` book in a pack folder (none is fine), over the books
    /// of the packs its manifest extends (see [`Manifest`]).
    pub fn load(pack: &Path) -> Result<Self, Error> {
        let mut layers = Vec::new();
        for folder in Manifest::chain(pack)? {
            let dir = folder.join("recipes");
            let mut paths: Vec<_> = match std::fs::read_dir(&dir) {
                Ok(entries) => entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "ron")).collect(),
                Err(_) => Vec::new(),
            };
            paths.sort();
            let sources: Vec<(String, String)> = paths
                .iter()
                .map(|p| std::fs::read_to_string(p).map(|s| (p.display().to_string(), s)).map_err(|e| Error(format!("{}: {e}", p.display()))))
                .collect::<Result<_, _>>()?;
            layers.push(sources);
        }
        Self::from_layers(layers.iter().map(|books| books.iter().map(|(o, s)| (o.as_str(), s.as_str())).collect()))
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
        self.render_at_least(name, 0)
    }

    /// Renders a texture at least `texels` across: a recipe written smaller is drawn
    /// two, four or more times finer (see the `scale` module), so a 32×32 pack can take
    /// every texture it does not redraw from 16×16 books and still show 32×32 detail.
    pub fn render_at_least(&self, name: &str, texels: u32) -> Option<Result<Image, Error>> {
        self.contains(name).then(|| {
            let native = self.resolve(name, 0, 1)?;
            let mut k = 1;
            while native.size.0 * k * 2 <= texels {
                k *= 2;
            }
            let resolved = if k == 1 { native } else { self.resolve(name, 0, k)? };
            render::render(&resolved).map_err(|Error(e)| Error(format!("{name}: {e}")))
        })
    }

    /// The recipe with its bases folded in, drawn `k` times finer than written.
    fn resolve(&self, name: &str, depth: usize, k: u32) -> Result<Resolved, Error> {
        if depth > MAX_DEPTH {
            return Err(Error(format!("{name}: `base` chain too deep (a cycle?)")));
        }
        let recipe = self.recipes.get(name).ok_or_else(|| Error(format!("no texture `{name}`")))?;
        let mut resolved = match &recipe.base {
            Some(base) => self.resolve(base, depth + 1, k).map_err(|Error(e)| Error(format!("{name}: {e}")))?,
            None => Resolved { size: (16 * k, 16 * k), seed: seed_of(name), palettes: BTreeMap::new(), layers: Vec::new(), stamps: BTreeMap::new() },
        };
        if let Some((w, h)) = recipe.size {
            resolved.size = (w * k, h * k);
        }
        // An explicit seed reseeds every layer, inherited ones too (a family's maple
        // planks differ from oak's); otherwise inherited layers keep their pattern and
        // this recipe's own layers are seeded by its name (each ore's flecks differ).
        if let Some(seed) = recipe.seed {
            resolved.seed = seed;
            resolved.layers.iter_mut().for_each(|(layer_seed, _)| *layer_seed = None);
        }
        let own = recipe.seed.unwrap_or_else(|| seed_of(name));
        // A recipe without a size of its own is written at its base's size as its own layer
        // saw it, but a later layer may redraw the base finer: default's crafting table over
        // the planks Voxmar HD draws at 32×32. Its own layers are drawn that much finer.
        let k = k * self.redrawn_finer(name, recipe, resolved.size, k);
        for (slot, palette) in &recipe.palettes {
            let shades = self.shades(palette, 0).map_err(|Error(e)| Error(format!("{name}: palette `{slot}`: {e}")))?;
            resolved.palettes.insert(slot.clone(), shades);
        }
        for layer in &recipe.layers {
            if let Layer::Stamp(stamp) = layer
                && !resolved.stamps.contains_key(&stamp_key(&stamp.texture, k))
            {
                let image = self.resolve(&stamp.texture, depth + 1, k).and_then(|r| render::render(&r)).map_err(|Error(e)| Error(format!("{name}: {e}")))?;
                resolved.stamps.insert(stamp_key(&stamp.texture, k), image);
            }
        }
        if let (None, None, Some(Layer::Stamp(stamp))) = (recipe.size, &recipe.base, recipe.layers.first())
            && stamp.relative
        {
            let image = &resolved.stamps[&stamp_key(&stamp.texture, k)];
            resolved.size = (image.width, image.height);
        }
        let first = resolved.layers.len();
        resolved.layers.extend(recipe.layers.iter().enumerate().map(|(i, layer)| {
            let mut layer = if k == 1 { layer.clone() } else { scale::layer(layer, k) };
            if let Layer::Stamp(stamp) = &mut layer {
                stamp.texture = stamp_key(&stamp.texture, k);
            }
            (Some(hash(own, (first + i) as i32, 0x1a7e)), layer)
        }));
        Ok(resolved)
    }

    /// How many times finer than its own layer saw it the base of `recipe` (`name`) is
    /// drawn: `size` is the base as finally resolved, `k` times finer than written. 1 when
    /// the recipe has a size of its own or no base, or the sizes do not divide evenly.
    fn redrawn_finer(&self, name: &str, recipe: &Recipe, size: (u32, u32), k: u32) -> u32 {
        let (None, Some(base)) = (recipe.size, &recipe.base) else { return 1 };
        let Some((w, h)) = self.size_seen_at(base, self.levels[name], 0) else { return 1 };
        let (w, h) = (w * k, h * k);
        if size.0 % w != 0 || size.1 % h != 0 {
            return 1;
        }
        let m = size.0 / w;
        if m >= 2 && size.1 / h == m { m } else { 1 }
    }

    /// The size `name` has as the books of `level` and below draw it, before any later
    /// layer redraws it or its bases; `None` if it did not exist there.
    fn size_seen_at(&self, name: &str, level: usize, depth: usize) -> Option<(u32, u32)> {
        if depth > MAX_DEPTH {
            return None;
        }
        let recipe = match self.levels.get(name) {
            Some(&at) if at <= level => self.recipes.get(name)?,
            _ => self.replaced.get(name)?.iter().rev().find(|(at, _)| *at <= level).map(|(_, r)| r)?,
        };
        if let Some(size) = recipe.size {
            return Some(size);
        }
        if let Some(base) = &recipe.base {
            return self.size_seen_at(base, level, depth + 1);
        }
        if let Some(Layer::Stamp(stamp)) = recipe.layers.first()
            && stamp.relative
        {
            return self.size_seen_at(&stamp.texture, level, depth + 1);
        }
        Some((16, 16))
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

/// A template member made for a family: `{name}` filled in, the family's palettes over its
/// own, and the family's seed unless it is made from another of the family's textures.
fn family_member(recipe: &Recipe, family: &Family) -> Recipe {
    let mut recipe = recipe.clone();
    // Textures made from another of the family's own keep its seed, so a leaf sprite
    // matches its leaves.
    let own_base = recipe.base.as_ref().is_some_and(|b| b.contains("{name}"));
    recipe.base = recipe.base.map(|b| b.replace("{name}", &family.name));
    for layer in &mut recipe.layers {
        if let Layer::Stamp(stamp) = layer {
            stamp.texture = stamp.texture.replace("{name}", &family.name);
        }
    }
    for palette in recipe.palettes.values_mut() {
        if let Palette::Named(n) = palette {
            *n = n.replace("{name}", &family.name);
        }
    }
    recipe.palettes.extend(family.palettes.clone());
    if !own_base {
        recipe.seed = family.seed.or(recipe.seed).or(Some(seed_of(&family.name)));
    }
    recipe
}

/// Stamped textures are rendered once for each fineness they are drawn at: a base and a
/// recipe drawn finer than it may stamp the same texture at two sizes.
fn stamp_key(texture: &str, k: u32) -> String {
    format!("{texture}@{k}")
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
                "block/{name}_planks_pair": (layers: [Stamp(texture: "block/{name}_planks", at: [(-8, 0), (8, 0)])]),
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
        // A stamp draws the family's own texture.
        let pair = library.render("block/pine_planks_pair").unwrap().unwrap();
        let planks = library.render("block/pine_planks").unwrap().unwrap();
        assert_eq!(pair.get(0, 3), planks.get(8, 3));
        assert_eq!(pair.get(15, 3), planks.get(7, 3));
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
        // Leniently, a broken book costs only itself.
        let (library, errors) = Library::from_books_lenient([("good", "(textures: {\"x\": ()})"), ("bad", "(textures: {\"y\": (bogus: 1)})")]);
        assert!(library.contains("x") && !library.contains("y"));
        assert_eq!(errors.len(), 1);
        assert!(errors[0].0.starts_with("bad:"));
    }

    #[test]
    fn later_layers_replace_earlier_ones() {
        let overlay = r##"(
            palettes: {"red": ["#ff0000"]},
            textures: {"block/rock": (size: (32, 32), palettes: {"main": "grey"}, layers: [Paint(palette: "main", base: 0.9)])},
            templates: {"wood": {"block/{name}_planks_dark": (layers: [Paint(palette: "wood", base: 0.0)])}},
        )"##;
        let library = Library::from_layers([vec![("base", BOOK)], vec![("hd", overlay)]]).unwrap();
        // A replaced texture, and the textures derived from it, take the new recipe.
        let rock = library.render("block/rock").unwrap().unwrap();
        assert_eq!((rock.width, rock.height), (32, 32));
        assert_eq!(rock.get(5, 5), [0xe0, 0xe0, 0xe0, 255]);
        assert_eq!(library.render("block/red_rock").unwrap().unwrap().width, 32);
        // A replaced palette recolours what uses it.
        assert!(library.render("block/red_rock").unwrap().unwrap().pixels.chunks(4).all(|p| p == [0xff, 0, 0, 0xff]));
        // A replaced template member redraws it for every family; the others stay.
        let dark = library.render("block/pine_planks_dark").unwrap().unwrap();
        assert!(dark.pixels.chunks(4).all(|p| p == [0x20, 0x20, 0x20, 0xff]));
        assert!(library.contains("block/pine_planks_pair"));
        // Within one layer, names stay unique.
        assert!(Library::from_layers([vec![("a", "(textures: {\"x\": ()})"), ("b", "(textures: {\"x\": ()})")]]).is_err());
    }

    #[test]
    fn a_child_written_at_16_fills_a_base_redrawn_at_32() {
        let child = r##"(textures: {"block/chest": (palettes: {"main": "grey"}, layers: [Paint(palette: "main", base: 0.0)]),
            "block/locked_chest": (base: "block/chest", layers: [Grid(legend: {'x': "#ff0000"}, rows: [
                "x...............", "................", "................", "................",
                "................", "................", "................", "................",
                "................", "................", "................", "................",
                "................", "................", "................", "...............x",
            ])])})"##;
        let hd = r##"(textures: {"block/chest": (size: (32, 32), palettes: {"main": "grey"}, layers: [Paint(palette: "main", base: 0.0)])})"##;
        let library = Library::from_layers([vec![("base", BOOK), ("child", child)], vec![("hd", hd)]]).unwrap();
        let locked = library.render("block/locked_chest").unwrap().unwrap();
        assert_eq!((locked.width, locked.height), (32, 32));
        // The lock plate's corners land in the face's corners, not its top-left quarter.
        for (x, y) in [(0, 0), (1, 1), (30, 30), (31, 31)] {
            assert_eq!(locked.get(x, y), [0xff, 0, 0, 0xff], "({x}, {y})");
        }
        assert_ne!(locked.get(15, 15), [0xff, 0, 0, 0xff]);
        // Drawn at the base's own size, it is drawn as written, and a pack drawing it at
        // 32 draws it 32 across, not 64.
        let plain = Library::from_books([("base", BOOK), ("child", child)]).unwrap();
        let native = plain.render("block/locked_chest").unwrap().unwrap();
        assert_eq!((native.width, native.get(15, 15)), (16, [0xff, 0, 0, 0xff]));
        let finer = plain.render_at_least("block/locked_chest", 32).unwrap().unwrap();
        assert_eq!((finer.width, finer.height), (32, 32));
        assert_eq!((finer.get(0, 0), finer.get(31, 31)), ([0xff, 0, 0, 0xff], [0xff, 0, 0, 0xff]));
        assert_ne!(finer.get(15, 15), [0xff, 0, 0, 0xff]);
    }

    #[test]
    fn a_recipe_drawn_with_its_base_stays_as_written() {
        // A pack drawing both the chest at 32×32 and a 16×16 badge on it meant the badge
        // to cover a quarter of the face.
        let hd = r##"(textures: {"block/chest": (size: (32, 32), palettes: {"main": "grey"}, layers: [Paint(palette: "main", base: 0.0)]),
            "block/badge": (base: "block/chest", layers: [Grid(legend: {'x': "#ff0000"}, rows: [
                "x...............", "................", "................", "................",
                "................", "................", "................", "................",
                "................", "................", "................", "................",
                "................", "................", "................", "...............x",
            ])])})"##;
        let badge = Library::from_layers([vec![("base", BOOK)], vec![("hd", hd)]]).unwrap().render("block/badge").unwrap().unwrap();
        assert_eq!((badge.width, badge.get(0, 0), badge.get(15, 15)), (32, [0xff, 0, 0, 0xff], [0xff, 0, 0, 0xff]));
        assert_ne!(badge.get(31, 31), [0xff, 0, 0, 0xff]);
    }

    #[test]
    fn any_layer_written_at_16_keeps_its_place_on_a_base_redrawn_finer() {
        // An 8×8 badge in the corner of a 16×16 plate covers a quarter of it, and a one-texel
        // border its edge, however finely a later pack redraws the plate.
        let child = r##"(textures: {"block/plate": (palettes: {"main": "grey"}, layers: [Paint(palette: "main", base: 0.0)]),
            "block/badge": (base: "block/plate", layers: [Grid(legend: {'x': "#ff0000"}, rows: [
                "x.......", "........", "........", "........", "........", "........", "........", ".......x",
            ])]),
            "block/framed": (base: "block/plate", palettes: {"edge": ["#00ff00"]}, layers: [Paint(palette: "edge", mask: [Border(width: 1, amount: 1.0)], above: 0.5)])})"##;
        let hd = r##"(textures: {"block/plate": (size: (32, 32), palettes: {"main": "grey"}, layers: [Paint(palette: "main", base: 0.0)])})"##;
        let library = Library::from_layers([vec![("base", BOOK), ("child", child)], vec![("hd", hd)]]).unwrap();
        let badge = library.render("block/badge").unwrap().unwrap();
        assert_eq!((badge.width, badge.get(15, 15)), (32, [0xff, 0, 0, 0xff]));
        assert_ne!(badge.get(16, 16), [0xff, 0, 0, 0xff]);
        let framed = library.render("block/framed").unwrap().unwrap();
        assert_eq!((framed.get(1, 1), framed.get(30, 30)), ([0, 0xff, 0, 0xff], [0, 0xff, 0, 0xff]));
        assert_ne!(framed.get(2, 2), [0, 0xff, 0, 0xff]);
    }

    #[test]
    fn a_family_member_redrawn_finer_counts_as_the_later_layer() {
        // The base pack's table is built on pine planks, which the family makes from the
        // `wood` template; a later pack redraws the template's planks at 32×32.
        let child = r##"(textures: {"block/pine_table": (base: "block/pine_planks", layers: [Grid(legend: {'x': "#ff0000"}, rows: [
                "x...............", "................", "................", "................",
                "................", "................", "................", "................",
                "................", "................", "................", "................",
                "................", "................", "................", "...............x",
            ])])})"##;
        let hd = r##"(templates: {"wood": {"block/{name}_planks": (size: (32, 32), layers: [Paint(palette: "wood", base: 0.5)])}})"##;
        let library = Library::from_layers([vec![("base", BOOK), ("child", child)], vec![("hd", hd)]]).unwrap();
        let table = library.render("block/pine_table").unwrap().unwrap();
        assert_eq!((table.width, table.get(31, 31)), (32, [0xff, 0, 0, 0xff]));
        assert_ne!(table.get(15, 15), [0xff, 0, 0, 0xff]);
    }

    #[test]
    fn a_texture_stamped_at_two_finenesses_is_drawn_at_each() {
        // The later pack's `framed` stamps the dot as written; the base pack's `doubled`,
        // built on `framed` and drawn twice as fine, stamps it twice as large.
        let base = r##"(textures: {"block/dot": (palettes: {"red": ["#ff0000"]}, layers: [Paint(palette: "red")]),
            "block/plain": (palettes: {"main": "grey"}, layers: [Paint(palette: "main", base: 0.0)]),
            "block/framed": (base: "block/plain"),
            "block/doubled": (base: "block/framed", layers: [Stamp(texture: "block/dot")])})"##;
        let hd = r##"(textures: {"block/framed": (size: (32, 32), palettes: {"main": "grey"}, layers: [Paint(palette: "main", base: 0.0), Stamp(texture: "block/dot", at: [(16, 0)])])})"##;
        let library = Library::from_layers([vec![("base", BOOK), ("more", base)], vec![("hd", hd)]]).unwrap();
        let doubled = library.render("block/doubled").unwrap().unwrap();
        // `framed`'s dot covers the top right quarter; `doubled`'s, twice as large, the face.
        assert_eq!((doubled.width, doubled.get(0, 0), doubled.get(31, 31)), (32, [0xff, 0, 0, 0xff], [0xff, 0, 0, 0xff]));
    }

    #[test]
    fn renders_finer_at_a_higher_resolution() {
        let library = library();
        for name in library.names() {
            let fine = library.render_at_least(name, 32).unwrap().unwrap();
            assert_eq!((fine.width, fine.height), (32, 32), "{name}");
        }
        // Shapes keep their place and proportions: the flower's sprite is twice as big.
        let flower = library.render("block/flower").unwrap().unwrap();
        let fine = library.render_at_least("block/flower", 32).unwrap().unwrap();
        for (x, y) in [(7, 10), (6, 10), (7, 13), (0, 0)] {
            assert_eq!(fine.get(2 * x, 2 * y), flower.get(x, y));
            assert_eq!(fine.get(2 * x + 1, 2 * y + 1), flower.get(x, y));
        }
        // A stamp lands where it did, in finer texels.
        let pair = library.render_at_least("block/pine_planks_pair", 32).unwrap().unwrap();
        let planks = library.render_at_least("block/pine_planks", 32).unwrap().unwrap();
        assert_eq!(pair.get(0, 6), planks.get(16, 6));
        // Already large enough: drawn as written.
        assert_eq!(library.render_at_least("block/rock", 16).unwrap().unwrap(), library.render("block/rock").unwrap().unwrap());
    }
}
