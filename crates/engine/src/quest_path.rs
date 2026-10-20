//! The quest book's paths (`quest_paths.ron`): each age's branches as the lanes the book
//! draws them in, top to bottom, each with a short name and an item for its icon. Every
//! branch a quest names needs one, and every path a branch some quest has, so a package
//! adding quests on a new branch adds its path beside them; a later package's path for a
//! branch already given replaces it in place.

use serde::Deserialize;

use crate::block::{BlockId, BlockRegistry, RegistryError};
use crate::quest::QuestRegistry;

/// One lane of an age's quest book.
#[derive(Clone, Debug, PartialEq)]
pub struct QuestPath {
    pub age: String,
    pub branch: String,
    /// What it is called, in English (`quest.path.<age>.<branch>` in the language files).
    pub title: String,
    pub icon: BlockId,
}

impl QuestPath {
    /// The key its name has in the language files.
    pub fn key(&self) -> String {
        format!("quest.path.{}.{}", self.age, self.branch)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathDef {
    age: String,
    branch: String,
    title: String,
    icon: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathFile {
    paths: Vec<PathDef>,
}

/// Every age's paths, in the order the book draws them.
#[derive(Clone, Debug, Default)]
pub struct QuestPaths {
    paths: Vec<QuestPath>,
}

impl QuestPaths {
    pub const FILE: &str = "quest_paths.ron";

    pub fn read(namespace: &str, source: &str) -> Result<PathFile, RegistryError> {
        ron::from_str(source).map_err(|e| RegistryError(format!("{namespace}/{}: {e}", Self::FILE)))
    }

    /// Joins the packages' files, in order, and checks them against the quests: each
    /// age and branch the quests name has a path, each path is one the quests name, and
    /// each icon is an item.
    pub fn new(files: Vec<(String, PathFile)>, quests: &QuestRegistry, blocks: &BlockRegistry) -> Result<Self, RegistryError> {
        let mut paths: Vec<QuestPath> = Vec::new();
        for (package, file) in files {
            for def in file.paths {
                let where_ = format!("{package}/{}: the path `{}` / `{}`", Self::FILE, def.age, def.branch);
                let name = if def.icon.contains(':') { def.icon.clone() } else { format!("{package}:{}", def.icon) };
                let icon = blocks.id(&name).ok_or_else(|| RegistryError(format!("{where_} has the icon `{name}`, which no package has")))?;
                if def.title.trim().is_empty() {
                    return Err(RegistryError(format!("{where_} has no title")));
                }
                let path = QuestPath { age: def.age, branch: def.branch, title: def.title, icon };
                match paths.iter_mut().find(|p| p.age == path.age && p.branch == path.branch) {
                    Some(given) => *given = path,
                    None => paths.push(path),
                }
            }
        }
        if let Some(quest) = quests.iter().find(|q| !paths.iter().any(|p| p.age == q.age && p.branch == q.branch)) {
            return Err(RegistryError(format!(
                "{}: the quest `{}` is on the branch `{}` of the age `{}`, which has no path: give it one (a title and an icon)",
                Self::FILE,
                quest.name,
                quest.branch,
                quest.age
            )));
        }
        if let Some(path) = paths.iter().find(|p| !quests.iter().any(|q| q.age == p.age && q.branch == p.branch)) {
            return Err(RegistryError(format!("{}: the path `{}` / `{}` has no quest on it", Self::FILE, path.age, path.branch)));
        }
        Ok(Self { paths })
    }

    /// Age `age`'s paths, top to bottom.
    pub fn of<'a>(&'a self, age: &'a str) -> impl Iterator<Item = &'a QuestPath> + 'a {
        self.paths.iter().filter(move |p| p.age == age)
    }

    pub fn iter(&self) -> impl Iterator<Item = &QuestPath> {
        self.paths.iter()
    }

    /// The ages, in the order their paths are first written: the book's chapters. Never by
    /// the quests' order, which follows file names (`quests.ron` before `quests_nomad.ron`).
    pub fn ages(&self) -> Vec<&str> {
        let mut ages: Vec<&str> = Vec::new();
        for path in &self.paths {
            if !ages.contains(&path.age.as_str()) {
                ages.push(&path.age);
            }
        }
        ages
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(source: &str) -> (String, PathFile) {
        ("mod".to_owned(), QuestPaths::read("mod", source).unwrap())
    }

    /// The shipped paths cover every branch; a path for no quest, a quest with no path, or
    /// an icon that is no item fails the launch; a later package renames a path in place.
    #[test]
    fn every_branch_has_a_path_and_every_path_a_branch() {
        let content = crate::content::Content::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content")).expect("the game's content loads");
        let (quests, blocks) = (&*content.quests, &*content.blocks);
        let shipped = || ("base".to_owned(), QuestPaths::read("base", &std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/content/base").join(QuestPaths::FILE)).unwrap()).unwrap());
        let paths = QuestPaths::new(vec![shipped()], quests, blocks).unwrap();
        assert!(quests.iter().all(|q| paths.iter().any(|p| p.age == q.age && p.branch == q.branch)));
        let renamed = QuestPaths::new(vec![shipped(), file(r#"(paths: [(age: "farm", branch: "main", title: "Fire", icon: "base:clay")])"#)], quests, blocks).unwrap();
        assert_eq!(renamed.iter().position(|p| p.title == "Fire"), paths.iter().position(|p| p.age == "farm" && p.branch == "main"), "renamed in place");
        let err = |extra: &str| QuestPaths::new(vec![shipped(), file(extra)], quests, blocks).unwrap_err().0;
        assert!(err(r#"(paths: [(age: "farm", branch: "sailing", title: "Sailing", icon: "base:clay")])"#).contains("no quest on it"));
        assert!(err(r#"(paths: [(age: "farm", branch: "main", title: "Fire", icon: "nothing")])"#).contains("mod:nothing"));
        assert!(err(r#"(paths: [(age: "farm", branch: "main", title: " ", icon: "base:clay")])"#).contains("no title"));
        assert_eq!(paths.ages(), ["nomad", "farm"], "the ages as the paths list them, though the Farm book's file is read first");
        assert_eq!(quests.iter().next().map(|q| q.age.as_str()), Some("farm"), "quests.ron, the Farm book, is read before quests_nomad.ron");
        let renamed_nomad = QuestPaths::new(vec![shipped(), file(r#"(paths: [(age: "nomad", branch: "main", title: "Fire", icon: "base:clay")])"#)], quests, blocks).unwrap();
        assert_eq!(renamed_nomad.ages(), ["nomad", "farm"], "a later package's path keeps its age's place");
        let without_farm_main = QuestPaths::new(vec![("base".to_owned(), QuestPaths::read("base", r#"(paths: [])"#).unwrap())], quests, blocks).unwrap_err().0;
        assert!(without_farm_main.contains("has no path"), "{without_farm_main}");
    }
}
