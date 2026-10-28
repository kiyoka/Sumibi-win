//! COM/native-TSF backend for low-integrity redirected test hives ONLY.
//! Registry layout is validated against the observed closed schema; it is not
//! copied into production registry hives. Real TSF writes always use TSF APIs.
use super::registration_native::{NativeTsf, DESCRIPTION};
use super::registration_registry::{self as reg, Failure, Tree};
use sumibi_setup::install_transaction::{
    Backend, Error, InstallSpec, Observation, Resource, RESOURCES,
};
use windows::core::GUID;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::TextServices::{
    GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER, GUID_TFCAT_TIP_KEYBOARD,
};

pub(super) const TARGET: &str = "sumibi-native-tsf-redirected-x64-v1";
const OWNER: &str = "sumibiregistrationowner";
fn error(value: impl std::fmt::Display) -> Error {
    Error(value.to_string())
}

#[cfg(test)]
#[path = "registration_backend_tests.rs"]
mod tests;
pub(super) fn spec(id: &str) -> InstallSpec {
    InstallSpec {
        registration_target: TARGET.into(),
        transaction: id.into(),
        dll_path: r"C:\Sumibi-native-fixture-never-loaded.dll".into(),
        sha256: "1".repeat(64),
        signer_thumbprint: "2".repeat(40),
    }
}
fn guid_name(guid: &GUID) -> String {
    format!("{{{guid:?}}}").to_ascii_lowercase()
}
pub(super) struct Layout {
    pub class: GUID,
    pub profile: GUID,
    pub com: String,
    pub service: String,
    pub language: String,
    pub categories: [(String, String, String); 2],
    pub expected: Tree,
}
impl Layout {
    pub fn new(spec: &InstallSpec) -> Result<Self, Failure> {
        spec.validate()?;
        if spec.registration_target != TARGET || *spec != self::spec(&spec.transaction) {
            return Err("native fixture artifact/target mismatch".into());
        }
        let class = GUID::from_u128(u128::from_str_radix(&spec.transaction, 16)?);
        if class == sumibi_tip::CLSID_SUMIBI_TIP {
            return Err("production CLSID forbidden in native fixture".into());
        }
        let profile = GUID::from_u128(class.to_u128() ^ 1);
        let name = guid_name(&class);
        let com = format!(r"software\classes\clsid\{name}");
        let service = format!(r"software\microsoft\ctf\tip\{name}");
        let language = format!(
            r"{service}\languageprofile\0x00000411\{}",
            guid_name(&profile)
        );
        let categories =
            [GUID_TFCAT_TIP_KEYBOARD, GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER].map(|category| {
                let category = guid_name(&category);
                (
                    format!(r"{service}\category\category\{category}\{name}"),
                    format!(r"{service}\category\item\{name}\{category}"),
                    format!(r"{service}\category\category\{category}"),
                )
            });
        let mut expected = Tree::default();
        expected
            .at_mut(&com)
            .values
            .insert(OWNER.into(), reg::string(&serde_json::to_string(spec)?));
        let server = expected.at_mut(&format!(r"{com}\inprocserver32"));
        server.values.insert("".into(), reg::string(&spec.dll_path));
        server
            .values
            .insert("threadingmodel".into(), reg::string("Apartment"));
        let lang = expected.at_mut(&language);
        lang.values
            .insert("description".into(), reg::string(DESCRIPTION));
        lang.values.insert("enable".into(), reg::dword(0));
        for (forward, reverse, _) in &categories {
            expected.at_mut(forward);
            expected.at_mut(reverse);
        }
        Ok(Self {
            class,
            profile,
            com,
            service,
            language,
            categories,
            expected,
        })
    }
    fn classify(&self, tree: &Tree, resource: Resource) -> Observation {
        if !tree.subset_of(&self.expected) {
            return Observation::Conflict;
        }
        match resource {
            Resource::ComServer => match tree.at(&self.com) {
                None if tree.at(r"software\classes").is_some()
                    || tree.at("software") == Some(&Tree::default()) =>
                {
                    Observation::OwnedIncomplete
                }
                None => Observation::Absent,
                Some(content) if Some(content) == self.expected.at(&self.com) => Observation::Owned,
                _ => Observation::OwnedIncomplete,
            },
            Resource::LanguageProfile => {
                if tree.at(&self.service).is_none() && tree.at(r"software\microsoft").is_none() {
                    Observation::Absent
                } else if tree.at(&self.language) == self.expected.at(&self.language) {
                    Observation::Owned
                } else {
                    Observation::OwnedIncomplete
                }
            }
            Resource::KeyboardCategory | Resource::DisplayAttributeCategory => {
                let index = usize::from(resource == Resource::DisplayAttributeCategory);
                let (forward, reverse, container) = &self.categories[index];
                if tree.at(forward).is_some() && tree.at(reverse).is_some() {
                    Observation::Owned
                } else if tree.at(container).is_some() || tree.at(reverse).is_some() {
                    Observation::OwnedIncomplete
                } else {
                    Observation::Absent
                }
            }
        }
    }
    pub fn empty_paths(&self) -> Vec<String> {
        let name = guid_name(&self.class);
        let mut paths = vec![];
        for (_, _, container) in &self.categories {
            paths.push(container.clone());
        }
        paths.extend([
            format!(r"{}\category\item\{name}", self.service),
            format!(r"{}\category\category", self.service),
            format!(r"{}\category\item", self.service),
            format!(r"{}\category", self.service),
            format!(r"{}\languageprofile\0x00000411", self.service),
            format!(r"{}\languageprofile", self.service),
            self.service.clone(),
            r"software\microsoft\ctf\tip".into(),
            r"software\microsoft\ctf".into(),
            r"software\microsoft".into(),
            r"software\classes\clsid".into(),
            r"software\classes".into(),
            "software".into(),
        ]);
        paths
    }
}
#[derive(Clone, Copy)]
pub(super) struct Cut<'a> {
    pub stage: &'a str,
    pub index: usize,
}
impl Cut<'_> {
    fn checkpoint(self, stage: &str, resource: Resource) {
        if self.stage == stage && RESOURCES[self.index] == resource {
            std::process::exit(77);
        }
    }
}
pub(super) struct RedirectedBackend<'a> {
    pub hives: &'a [reg::Key; 3],
    pub native: &'a NativeTsf,
    pub cut: Cut<'a>,
}
impl RedirectedBackend<'_> {
    fn inspect(&self, spec: &InstallSpec) -> Result<(Layout, Tree), Failure> {
        let layout = Layout::new(spec)?;
        let tree = reg::snapshot(self.hives[0].0)?;
        if !tree.subset_of(&layout.expected)
            || reg::snapshot(self.hives[1].0)? != Tree::default()
            || reg::snapshot(self.hives[2].0)? != Tree::default()
        {
            return Err("unknown native registration state; preserving all resources".into());
        }
        Ok((layout, tree))
    }
    pub fn prune_empty(&self, spec: &InstallSpec, resource: Resource) -> Result<(), Failure> {
        let (layout, _) = self.inspect(spec)?;
        for path in layout.empty_paths() {
            if let Some(key) = reg::open(self.hives[0].0, &path, KEY_READ)? {
                let empty = reg::snapshot(key.0)? == Tree::default();
                drop(key);
                if empty {
                    reg::delete_key(self.hives[0].0, &path)?;
                    reg::flush(self.hives[0].0)?;
                    self.cut.checkpoint("delete-during-prune", resource);
                }
            }
        }
        reg::flush(self.hives[0].0)
    }
}
impl Backend for RedirectedBackend<'_> {
    fn preflight(&mut self, spec: &InstallSpec) -> Result<(), Error> {
        let (_, tree) = self.inspect(spec).map_err(error)?;
        if tree != Tree::default() {
            return Err(error("native fixture must start empty"));
        }
        Ok(())
    }
    fn observe(&mut self, spec: &InstallSpec, resource: Resource) -> Result<Observation, Error> {
        let (layout, tree) = self.inspect(spec).map_err(error)?;
        let state = layout.classify(&tree, resource);
        if resource == Resource::LanguageProfile
            && state == Observation::Owned
            && layout
                .categories
                .iter()
                .all(|(a, b, _)| tree.at(a).is_some() && tree.at(b).is_some())
        {
            self.native
                .verify_disabled(&layout.class, &layout.profile)
                .map_err(error)?;
        }
        Ok(state)
    }
    fn create(&mut self, spec: &InstallSpec, resource: Resource) -> Result<(), Error> {
        let (layout, tree) = self.inspect(spec).map_err(error)?;
        if layout.classify(&tree, resource) != Observation::Absent {
            return Err(error("native resource already present"));
        }
        self.cut.checkpoint("create-before", resource);
        if self.cut.stage == "create-fail" && RESOURCES[self.cut.index] == resource {
            return Err(error("injected native API failure"));
        }
        let result = (|| -> Result<(), Failure> {
            match resource {
                Resource::ComServer => {
                    let key = reg::create_new(self.hives[0].0, &layout.com)?;
                    reg::flush(key.0)?;
                    self.cut.checkpoint("com-empty", resource);
                    reg::put_string(key.0, OWNER, &serde_json::to_string(spec)?)?;
                    reg::flush(key.0)?;
                    self.cut.checkpoint("com-partial", resource);
                    let server = reg::create_new(key.0, "InprocServer32")?;
                    reg::put_string(server.0, "", &spec.dll_path)?;
                    reg::put_string(server.0, "ThreadingModel", "Apartment")?;
                    let classes =
                        reg::open(self.hives[0].0, r"Software\Classes", KEY_READ | KEY_WRITE)?
                            .ok_or("fixture COM classes missing")?;
                    // SAFETY: this backend runs only inside the remapped worker;
                    // HKCR must reflect the same COM records as the fake HKLM.
                    unsafe {
                        RegOverridePredefKey(HKEY_CLASSES_ROOT, Some(classes.0)).ok()?;
                    }
                }
                Resource::LanguageProfile => self
                    .native
                    .register_profile(&layout.class, &layout.profile)?,
                Resource::KeyboardCategory => self
                    .native
                    .register_category(&layout.class, &GUID_TFCAT_TIP_KEYBOARD)?,
                Resource::DisplayAttributeCategory => self
                    .native
                    .register_category(&layout.class, &GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER)?,
            }
            reg::flush(self.hives[0].0)?;
            self.cut.checkpoint("create-after", resource);
            Ok(())
        })();
        result.map_err(error)
    }
    fn remove_owned(&mut self, spec: &InstallSpec, resource: Resource) -> Result<(), Error> {
        let (layout, tree) = self.inspect(spec).map_err(error)?;
        if !matches!(
            layout.classify(&tree, resource),
            Observation::Owned | Observation::OwnedIncomplete
        ) {
            return Err(error("native resource is not owned"));
        }
        self.cut.checkpoint("delete-before", resource);
        let result = (|| -> Result<(), Failure> {
            match resource {
                Resource::ComServer => {
                    let server = format!(r"{}\inprocserver32", layout.com);
                    if tree.at(&server).is_some() {
                        reg::delete_key(self.hives[0].0, &server)?;
                    }
                    if tree.at(&layout.com).is_some() {
                        reg::delete_key(self.hives[0].0, &layout.com)?;
                    }
                }
                Resource::LanguageProfile => {
                    if tree.at(&layout.language).is_some() {
                        self.native
                            .unregister_profile(&layout.class, &layout.profile)?;
                    }
                }
                Resource::KeyboardCategory | Resource::DisplayAttributeCategory => {
                    let index = usize::from(resource == Resource::DisplayAttributeCategory);
                    let (forward, reverse, _) = &layout.categories[index];
                    if tree.at(forward).is_some() || tree.at(reverse).is_some() {
                        let category = if index == 0 {
                            GUID_TFCAT_TIP_KEYBOARD
                        } else {
                            GUID_TFCAT_DISPLAYATTRIBUTEPROVIDER
                        };
                        self.native.unregister_category(&layout.class, &category)?;
                    }
                }
            }
            reg::flush(self.hives[0].0)?;
            self.cut.checkpoint("delete-after-api", resource);
            self.prune_empty(spec, resource)?;
            self.cut.checkpoint("delete-after", resource);
            Ok(())
        })();
        result.map_err(error)
    }
}
