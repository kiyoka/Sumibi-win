//! Bounded native registry inspection used by the isolated TSF registration test.
use serde::Serialize;
use std::collections::BTreeMap;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, FILETIME};
use windows::Win32::System::Registry::*;

pub(super) type Failure = Box<dyn std::error::Error>;
pub(super) struct Key(pub HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: only successful native open/create handles enter Key.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}
pub(super) fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}
pub(super) fn open(
    parent: HKEY,
    name: &str,
    access: REG_SAM_FLAGS,
) -> Result<Option<Key>, Failure> {
    let name = wide(name);
    let mut key = HKEY::default();
    // SAFETY: live parent, terminated name, initialized output, explicit x64 view.
    let status = unsafe {
        RegOpenKeyExW(
            parent,
            PCWSTR(name.as_ptr()),
            None,
            access | KEY_WOW64_64KEY,
            &mut key,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    status.ok()?;
    Ok(Some(Key(key)))
}
pub(super) fn create_new(parent: HKEY, name: &str) -> Result<Key, Failure> {
    let name = wide(name);
    let mut key = HKEY::default();
    let mut disposition = REG_CREATE_KEY_DISPOSITION::default();
    // SAFETY: live parent and initialized outputs. Refuse existing keys before writing.
    unsafe {
        RegCreateKeyExW(
            parent,
            PCWSTR(name.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_ALL_ACCESS | KEY_WOW64_64KEY,
            None,
            &mut key,
            Some(&mut disposition),
        )
        .ok()?;
    }
    let key = Key(key);
    if disposition != REG_CREATED_NEW_KEY {
        return Err("native registry key already exists".into());
    }
    Ok(key)
}
pub(super) fn put_string(key: HKEY, name: &str, value: &str) -> Result<(), Failure> {
    let name = wide(name);
    let data: Vec<u8> = wide(value).into_iter().flat_map(u16::to_le_bytes).collect();
    // SAFETY: terminated name and owned data remain live throughout the call.
    unsafe {
        RegSetValueExW(key, PCWSTR(name.as_ptr()), None, REG_SZ, Some(&data)).ok()?;
    }
    Ok(())
}

pub(super) fn delete_key(parent: HKEY, name: &str) -> Result<(), Failure> {
    let name = wide(name);
    // SAFETY: one exact key in the explicit x64 view; never RegDeleteTree.
    unsafe {
        RegDeleteKeyExW(parent, PCWSTR(name.as_ptr()), KEY_WOW64_64KEY.0, None).ok()?;
    }
    Ok(())
}
pub(super) fn flush(key: HKEY) -> Result<(), Failure> {
    // SAFETY: the caller retains the live key throughout this synchronous call.
    unsafe {
        RegFlushKey(key).ok()?;
    }
    Ok(())
}
pub(super) fn put_value(key: HKEY, name: &str, value: &Value) -> Result<(), Failure> {
    let name = wide(name);
    // SAFETY: bounded fixture value supplied by this crate; owned name/data.
    unsafe {
        RegSetValueExW(
            key,
            PCWSTR(name.as_ptr()),
            None,
            REG_VALUE_TYPE(value.kind),
            Some(&value.bytes),
        )
        .ok()?;
    }
    Ok(())
}
pub(super) fn delete_value(key: HKEY, name: &str) -> Result<(), Failure> {
    let name = wide(name);
    // SAFETY: exact named sentinel in a live fixture key.
    unsafe {
        RegDeleteValueW(key, PCWSTR(name.as_ptr())).ok()?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(super) struct Value {
    pub kind: u32,
    pub bytes: Vec<u8>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub(super) struct Tree {
    pub values: BTreeMap<String, Value>,
    pub children: BTreeMap<String, Tree>,
}
impl Tree {
    pub fn at(&self, path: &str) -> Option<&Self> {
        path.split('\\').try_fold(self, |tree, name| {
            tree.children.get(&name.to_ascii_lowercase())
        })
    }
    pub fn at_mut(&mut self, path: &str) -> &mut Self {
        path.split('\\').fold(self, |tree, name| {
            tree.children.entry(name.to_ascii_lowercase()).or_default()
        })
    }
    pub fn subset_of(&self, expected: &Self) -> bool {
        self.values
            .iter()
            .all(|(name, value)| expected.values.get(name) == Some(value))
            && self.children.iter().all(|(name, tree)| {
                expected
                    .children
                    .get(name)
                    .is_some_and(|model| tree.subset_of(model))
            })
    }
}
pub(super) fn string(value: &str) -> Value {
    Value {
        kind: REG_SZ.0,
        bytes: wide(value).into_iter().flat_map(u16::to_le_bytes).collect(),
    }
}
pub(super) fn dword(value: u32) -> Value {
    Value {
        kind: REG_DWORD.0,
        bytes: value.to_le_bytes().to_vec(),
    }
}
fn stamp(key: HKEY) -> Result<(u32, u32, u64), Failure> {
    let (mut children, mut values) = (0, 0);
    let mut modified = FILETIME::default();
    // SAFETY: valid key and initialized scalar outputs.
    unsafe {
        RegQueryInfoKeyW(
            key,
            None,
            None,
            None,
            Some(&mut children),
            None,
            None,
            Some(&mut values),
            None,
            None,
            None,
            Some(&mut modified),
        )
        .ok()?;
    }
    Ok((
        children,
        values,
        (u64::from(modified.dwHighDateTime) << 32) | u64::from(modified.dwLowDateTime),
    ))
}
pub(super) fn snapshot(key: HKEY) -> Result<Tree, Failure> {
    fn read(
        key: HKEY,
        depth: usize,
        nodes: &mut usize,
        total: &mut usize,
    ) -> Result<Tree, Failure> {
        *nodes += 1;
        let before = stamp(key)?;
        if depth > 16 || *nodes > 128 || before.0 > 64 || before.1 > 32 {
            return Err("registry snapshot bounds exceeded".into());
        }
        let mut tree = Tree::default();
        for index in 0..before.1 {
            let mut name = [0u16; 256];
            let mut length = name.len() as u32;
            let mut kind = 0;
            let mut data = vec![0u8; 4096];
            let mut used = data.len() as u32;
            // SAFETY: buffers have explicit bounded sizes and initialized outputs.
            unsafe {
                RegEnumValueW(
                    key,
                    index,
                    Some(PWSTR(name.as_mut_ptr())),
                    &mut length,
                    None,
                    Some(&mut kind),
                    Some(data.as_mut_ptr()),
                    Some(&mut used),
                )
                .ok()?;
            }
            if used as usize > data.len() || length as usize >= name.len() {
                return Err("registry value changed during snapshot".into());
            }
            data.truncate(used as usize);
            *total += data.len() + length as usize * 2;
            if *total > 65536 {
                return Err("registry snapshot byte limit exceeded".into());
            }
            let name = String::from_utf16(&name[..length as usize])?.to_ascii_lowercase();
            if tree
                .values
                .insert(name, Value { kind, bytes: data })
                .is_some()
            {
                return Err("duplicate registry value name".into());
            }
        }
        for index in 0..before.0 {
            let mut name = [0u16; 256];
            let mut length = name.len() as u32;
            // SAFETY: bounded UTF-16 name buffer and writable size output.
            unsafe {
                RegEnumKeyExW(
                    key,
                    index,
                    Some(PWSTR(name.as_mut_ptr())),
                    &mut length,
                    None,
                    None,
                    None,
                    None,
                )
                .ok()?;
            }
            if length as usize >= name.len() {
                return Err("registry child name exceeds bound".into());
            }
            let name = String::from_utf16(&name[..length as usize])?;
            let child =
                open(key, &name, KEY_READ)?.ok_or("registry child disappeared during snapshot")?;
            let content = read(child.0, depth + 1, nodes, total)?;
            if tree
                .children
                .insert(name.to_ascii_lowercase(), content)
                .is_some()
            {
                return Err("duplicate registry child name".into());
            }
        }
        if stamp(key)? != before {
            return Err("registry changed during snapshot".into());
        }
        Ok(tree)
    }
    read(key, 0, &mut 0, &mut 0)
}
