//! Збірки (профілі): назва + версія + нікнейм + пам'ять. Кожна збірка має власну папку
//! `<gameDir>/instances/<id>` (свої mods, saves, options.txt), а assets/libraries/versions спільні.

use crate::{data_dir, safe_component};
use crate::offline_auth::validate_username;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf, time::{SystemTime, UNIX_EPOCH}};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Profile {
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub version: String,
    pub username: String,
    pub memory_mb: u32,
    #[serde(default = "default_loader")]
    pub loader: String,
    #[serde(default)]
    pub jvm_args: String,
}

fn default_loader() -> String { "fabric".to_string() }

fn file() -> PathBuf { data_dir().join("profiles.json") }

fn read_all() -> Vec<Profile> {
    fs::read_to_string(file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn write_all(list: &[Profile]) -> Result<(), String> {
    let path = file();
    if let Some(dir) = path.parent() { fs::create_dir_all(dir).map_err(|e| e.to_string())?; }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(list).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn list_profiles() -> Vec<Profile> { read_all() }

#[tauri::command]
pub fn save_profile(mut profile: Profile) -> Result<Profile, String> {
    profile.name = profile.name.trim().to_string();
    if profile.name.is_empty() || profile.name.chars().count() > 40 { return Err("Назва збірки: від 1 до 40 символів.".into()); }
    if !safe_component(&profile.version) { return Err("Обери версію гри.".into()); }
    validate_username(&profile.username).map_err(|e| format!("Нікнейм: {e:?}"))?;
    profile.memory_mb = profile.memory_mb.clamp(512, 32768);
    if !["vanilla", "fabric", "quilt", "forge", "neoforge"].contains(&profile.loader.as_str()) { return Err("Непідтримуваний модлоадер".into()); }
    let mut list = read_all();
    if profile.id.is_empty() {
        let ms = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        profile.id = format!("b{ms:x}");
    } else if !safe_component(&profile.id) {
        return Err("Некоректний ID збірки".into());
    }
    match list.iter_mut().find(|p| p.id == profile.id) {
        Some(slot) => *slot = profile.clone(),
        None => list.push(profile.clone()),
    }
    write_all(&list)?;
    Ok(profile)
}

/// Видаляє збірку зі списку. Файли в `instances/<id>` (світи, моди) НЕ чіпає.
#[tauri::command]
pub fn delete_profile(id: String) -> Result<(), String> {
    let mut list = read_all();
    list.retain(|p| p.id != id);
    write_all(&list)
}
