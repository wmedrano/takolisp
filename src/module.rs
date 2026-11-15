use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
};

use crate::val::{Symbol, Val};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ModuleId {
    Internal(Symbol),
    File(PathBuf),
}

impl ModuleId {
    pub const GLOBAL: Self = Self::Internal(Symbol(smol_str::SmolStr::new_static("global")));
    pub const USER: Self = Self::Internal(Symbol(smol_str::SmolStr::new_static("user")));

    pub fn internal(name: impl Into<Symbol>) -> Self {
        Self::Internal(name.into())
    }

    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self::File(normalize_path(&path.into()))
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match normalized.last() {
                Some(Component::Normal(_)) => {
                    normalized.pop();
                }
                Some(Component::RootDir | Component::Prefix(_)) => {}
                Some(Component::ParentDir) | None => normalized.push(component),
                Some(Component::CurDir) => unreachable!(),
            },
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component);
            }
        }
    }
    if normalized.is_empty() && !path.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized.iter().collect()
    }
}

#[derive(Clone, Debug)]
pub struct Module {
    id: ModuleId,
    loaded: bool,
    globals: HashMap<Symbol, Val>,
    imports: HashMap<Symbol, ModuleId>,
}

impl Module {
    pub fn new(id: ModuleId) -> Self {
        let loaded = matches!(id, ModuleId::Internal(_));
        Self {
            id,
            loaded,
            globals: HashMap::new(),
            imports: HashMap::new(),
        }
    }

    pub fn internal(name: impl Into<Symbol>) -> Self {
        Self::new(ModuleId::internal(name))
    }

    pub fn from_file(path: impl Into<PathBuf>) -> Self {
        Self::new(ModuleId::file(path))
    }

    pub fn id(&self) -> &ModuleId {
        &self.id
    }

    pub fn is_loaded(&self) -> bool {
        self.loaded
    }

    pub fn set_loaded(&mut self, loaded: bool) {
        self.loaded = loaded;
    }

    pub fn get(&self, symbol: &Symbol) -> Option<&Val> {
        self.globals.get(symbol)
    }

    pub fn insert(&mut self, symbol: impl Into<Symbol>, val: Val) -> Option<Val> {
        self.globals.insert(symbol.into(), val)
    }

    pub fn imports(&self) -> &HashMap<Symbol, ModuleId> {
        &self.imports
    }

    pub fn get_import(&self, symbol: &Symbol) -> Option<&ModuleId> {
        self.imports.get(symbol)
    }

    pub fn import(&mut self, symbol: impl Into<Symbol>, module: ModuleId) -> Option<ModuleId> {
        self.imports.insert(symbol.into(), module)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_stores_and_looks_up_globals() {
        let mut module = Module::new(ModuleId::USER);
        assert_eq!(module.get(&Symbol::from("x")), None);
        assert_eq!(module.insert("x", Val::Int(42)), None);
        assert_eq!(module.get(&Symbol::from("x")), Some(&Val::Int(42)));
        assert_eq!(module.insert("x", Val::Int(99)), Some(Val::Int(42)));
        assert_eq!(module.get(&Symbol::from("x")), Some(&Val::Int(99)));
    }

    #[test]
    fn module_tracks_internal_and_file_identity() {
        let user_mod = Module::new(ModuleId::USER);
        assert_eq!(user_mod.id(), &ModuleId::internal("user"));

        let core_mod = Module::internal("core");
        assert_eq!(core_mod.id(), &ModuleId::Internal("core".into()));

        let file_mod = Module::from_file("examples/hello.tl");
        assert_eq!(
            file_mod.id(),
            &ModuleId::File(PathBuf::from("examples/hello.tl"))
        );
    }

    #[test]
    fn module_id_file_normalizes_paths() {
        assert_eq!(
            ModuleId::file("./examples/./sub/../hello.tl"),
            ModuleId::file("examples/hello.tl")
        );
        assert_eq!(
            ModuleId::file("../shared/math.tl"),
            ModuleId::File(PathBuf::from("../shared/math.tl"))
        );
        assert_eq!(
            ModuleId::file("/a/b/../c/../../d.tl"),
            ModuleId::File(PathBuf::from("/d.tl"))
        );
        assert_eq!(ModuleId::file("a/.."), ModuleId::File(PathBuf::from(".")));
    }

    #[test]
    fn module_stores_and_looks_up_imports() {
        let mut module = Module::new(ModuleId::USER);
        assert!(module.imports().is_empty());
        assert_eq!(module.get_import(&Symbol::from("math")), None);

        let math_mod = ModuleId::internal("math");
        assert_eq!(module.import("math", math_mod.clone()), None);
        assert_eq!(module.get_import(&Symbol::from("math")), Some(&math_mod));
        assert_eq!(module.imports().get(&Symbol::from("math")), Some(&math_mod));

        let file_math_mod = ModuleId::file("lib/math.tl");
        assert_eq!(
            module.import("math", file_math_mod.clone()),
            Some(math_mod)
        );
        assert_eq!(
            module.get_import(&Symbol::from("math")),
            Some(&file_math_mod)
        );
    }

    #[test]
    fn module_tracks_loaded_state_with_expected_defaults() {
        let user_mod = Module::new(ModuleId::USER);
        assert!(user_mod.is_loaded());

        let internal_mod = Module::internal("core");
        assert!(internal_mod.is_loaded());

        let mut file_mod = Module::from_file("examples/hello.tl");
        assert!(!file_mod.is_loaded());

        file_mod.set_loaded(true);
        assert!(file_mod.is_loaded());

        file_mod.set_loaded(false);
        assert!(!file_mod.is_loaded());
    }
}
