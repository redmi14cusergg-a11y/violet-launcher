//! Скіни в офлайн-режимі. Без сервера Mojang гра показує «стандартний» скін за UUID,
//! тому ми генеруємо ресурс-пак `VioletSkin`, який підміняє ВСІ стандартні скіни (Steve, Alex, Ari, …)
//! на обраний, і вмикаємо його в options.txt збірки. Скін бачить лише власник (клієнтська підміна).

use crate::offline_auth::validate_username;
use crate::{data_dir, http};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::Serialize;
use serde_json::{json, Value};
use std::{fs, path::{Path, PathBuf}};

const PACK_ID: &str = "VioletSkin";
const DEFAULTS: [&str; 9] = ["steve", "alex", "ari", "efe", "kai", "makena", "noor", "sunny", "zuri"];

fn skins_dir() -> PathBuf { data_dir().join("skins") }
fn skin_path(name: &str) -> PathBuf { skins_dir().join(format!("{name}.png")) }

fn clean_name(n: &str) -> Result<String, String> {
    let t = n.trim();
    let ok = !t.is_empty() && t.chars().count() <= 32 && t.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '_' | '-'));
    if ok { Ok(t.to_string()) } else { Err("Назва скіна: 1–32 символи (літери, цифри, пробіл, _ та -).".into()) }
}

pub fn png_size(b: &[u8]) -> Result<(u32, u32), String> {
    if b.len() < 24 || &b[..8] != b"\x89PNG\r\n\x1a\n" || &b[12..16] != b"IHDR" { return Err("Це не PNG-файл.".into()); }
    Ok((u32::from_be_bytes([b[16], b[17], b[18], b[19]]), u32::from_be_bytes([b[20], b[21], b[22], b[23]])))
}

pub fn validate_skin(b: &[u8]) -> Result<(), String> {
    if b.len() > 300_000 { return Err("Файл завеликий для скіна.".into()); }
    let (w, h) = png_size(b)?;
    if (w, h) != (64, 64) {
        return Err(format!("Скін має бути PNG 64×64 (у цього файлу {w}×{h}). Старий формат 64×32 не підтримується."));
    }
    Ok(())
}

#[derive(Serialize)]
pub struct SkinInfo { name: String, data_url: String, active: bool }

pub fn active_skin() -> Option<String> {
    let n = fs::read_to_string(skins_dir().join("active.txt")).ok()?.trim().to_string();
    if !n.is_empty() && skin_path(&n).is_file() { Some(n) } else { None }
}

#[tauri::command]
pub fn list_skins() -> Vec<SkinInfo> {
    let active = active_skin();
    let mut out: Vec<SkinInfo> = fs::read_dir(skins_dir()).into_iter().flatten().flatten().filter_map(|e| {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("png") { return None; }
        let name = p.file_stem()?.to_string_lossy().to_string();
        let bytes = fs::read(&p).ok()?;
        Some(SkinInfo { active: active.as_deref() == Some(name.as_str()), data_url: format!("data:image/png;base64,{}", B64.encode(bytes)), name })
    }).collect();
    out.sort_by_key(|s| s.name.to_lowercase());
    out
}

fn save_skin(name: &str, bytes: &[u8]) -> Result<String, String> {
    let name = clean_name(name)?;
    validate_skin(bytes)?;
    fs::create_dir_all(skins_dir()).map_err(|e| e.to_string())?;
    fs::write(skin_path(&name), bytes).map_err(|e| e.to_string())?;
    Ok(name)
}

#[tauri::command]
pub fn add_skin(name: String, bytes: Vec<u8>) -> Result<String, String> { save_skin(&name, &bytes) }

#[tauri::command]
pub fn delete_skin(name: String) -> Result<(), String> {
    let name = clean_name(&name)?;
    if active_skin().as_deref() == Some(name.as_str()) { let _ = fs::remove_file(skins_dir().join("active.txt")); }
    let p = skin_path(&name);
    if p.is_file() { fs::remove_file(p).map_err(|e| e.to_string())?; }
    Ok(())
}

/// Порожня назва — вимкнути кастомний скін.
#[tauri::command]
pub fn set_active_skin(name: String) -> Result<(), String> {
    fs::create_dir_all(skins_dir()).map_err(|e| e.to_string())?;
    if name.trim().is_empty() { let _ = fs::remove_file(skins_dir().join("active.txt")); return Ok(()); }
    let name = clean_name(&name)?;
    if !skin_path(&name).is_file() { return Err("Скін не знайдено.".into()); }
    fs::write(skins_dir().join("active.txt"), name).map_err(|e| e.to_string())
}

fn import_blocking(username: String) -> Result<String, String> {
    validate_username(&username).map_err(|_| "Некоректний нікнейм (A–Z, a–z, 0–9, _; до 16 символів).".to_string())?;
    let c = http()?;
    let r = c.get(format!("https://api.mojang.com/users/profiles/minecraft/{username}")).send().map_err(|e| format!("Помилка мережі: {e}"))?;
    if r.status() == 404 || r.status() == 204 { return Err("Гравця з таким нікнеймом не знайдено.".into()); }
    let prof: Value = r.error_for_status().map_err(|e| e.to_string())?.json().map_err(|e| e.to_string())?;
    let id = prof.get("id").and_then(Value::as_str).filter(|s| s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit())).ok_or("Mojang повернув некоректний ID")?;
    let real_name = prof.get("name").and_then(Value::as_str).unwrap_or(&username).to_string();
    let session: Value = c.get(format!("https://sessionserver.mojang.com/session/minecraft/profile/{id}")).send().map_err(|e| format!("Помилка мережі: {e}"))?
        .error_for_status().map_err(|e| format!("Mojang тимчасово не віддає профіль: {e}"))?.json().map_err(|e| e.to_string())?;
    let encoded = session.get("properties").and_then(Value::as_array)
        .and_then(|a| a.iter().find(|p| p.get("name").and_then(Value::as_str) == Some("textures")))
        .and_then(|p| p.get("value")).and_then(Value::as_str).ok_or("У профілі немає даних про скін")?;
    let decoded: Value = serde_json::from_slice(&B64.decode(encoded).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let url = decoded.pointer("/textures/SKIN/url").and_then(Value::as_str).ok_or("Цей гравець використовує стандартний скін.")?
        .replacen("http://", "https://", 1);
    if !url.starts_with("https://textures.minecraft.net/") { return Err("Несподіване джерело скіна.".into()); }
    let bytes = c.get(&url).send().map_err(|e| format!("Помилка мережі: {e}"))?.error_for_status().map_err(|e| e.to_string())?.bytes().map_err(|e| e.to_string())?;
    save_skin(&real_name, &bytes)
}

#[tauri::command]
pub async fn import_skin_by_username(username: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || import_blocking(username)).await.map_err(|e| format!("Фонова задача перервалася: {e}"))?
}

// ---------- застосування до збірки ----------

fn pack_format(version: &str) -> u32 {
    let mut it = version.split('.').map(|x| x.parse::<u32>().ok());
    if it.next() != Some(Some(1)) { return 75; }
    let minor = it.next().flatten().unwrap_or(99);
    let patch = it.next().flatten().unwrap_or(0);
    match (minor, patch) {
        (m, _) if m <= 8 => 1,
        (m, _) if m <= 10 => 2,
        (m, _) if m <= 12 => 3,
        (m, _) if m <= 14 => 4,
        (15, _) => 5,
        (16, _) => 6,
        (17, _) => 7,
        (18, _) => 8,
        (19, p) if p <= 2 => 9,
        (19, _) => 12,
        (20, p) if p <= 1 => 15,
        (20, 2) => 18,
        (20, p) if p <= 4 => 22,
        (20, _) => 32,
        (21, p) if p <= 1 => 34,
        (21, p) if p <= 3 => 42,
        (21, 4) => 46,
        (21, 5) => 55,
        (21, 6) => 63,
        (21, p) if p <= 8 => 64,
        (21, p) if p <= 10 => 69,
        _ => 75,
    }
}

fn set_pack_enabled(opts: &Path, enabled: bool) -> Result<(), String> {
    let text = fs::read_to_string(opts).unwrap_or_default();
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    let entry = format!("file/{PACK_ID}");
    let idx = lines.iter().position(|l| l.starts_with("resourcePacks:"));
    if idx.is_none() && !enabled { return Ok(()); }
    let mut packs: Vec<String> = idx
        .and_then(|i| serde_json::from_str(&lines[i]["resourcePacks:".len()..]).ok())
        .unwrap_or_else(|| vec!["vanilla".to_string()]);
    packs.retain(|p| p != &entry);
    if enabled { packs.push(entry); } // останній = найвищий пріоритет
    if !packs.iter().any(|p| p == "vanilla") { packs.insert(0, "vanilla".into()); }
    let line = format!("resourcePacks:{}", serde_json::to_string(&packs).map_err(|e| e.to_string())?);
    match idx { Some(i) => lines[i] = line, None => lines.push(line) }
    fs::write(opts, lines.join("\n") + "\n").map_err(|e| e.to_string())
}

/// Створює/видаляє ресурс-пак зі скіном у папці збірки та вмикає/вимикає його в options.txt.
pub fn apply_skin(instance: &Path, version: &str, skin: Option<&str>) -> Result<(), String> {
    let pack_dir = instance.join("resourcepacks").join(PACK_ID);
    let opts = instance.join("options.txt");
    let Some(name) = skin else {
        let _ = fs::remove_dir_all(&pack_dir);
        return set_pack_enabled(&opts, false);
    };
    let png = fs::read(skin_path(name)).map_err(|_| format!("Скін «{name}» не знайдено"))?;
    validate_skin(&png)?;
    let tex = pack_dir.join("assets").join("minecraft").join("textures").join("entity");
    let mut targets: Vec<PathBuf> = vec![tex.join("steve.png"), tex.join("alex.png")]; // ≤1.19.2
    for n in DEFAULTS { for kind in ["wide", "slim"] { targets.push(tex.join("player").join(kind).join(format!("{n}.png"))); } }
    for t in targets {
        fs::create_dir_all(t.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::write(t, &png).map_err(|e| e.to_string())?;
    }
    let meta = json!({"pack": {
        "pack_format": pack_format(version),
        "supported_formats": {"min_inclusive": 1, "max_inclusive": 999},
        // 1.21.9+ читає діапазон із min_format/max_format; старі версії ці ключі ігнорують.
        "min_format": 1, "max_format": 999,
        "description": "Violet Launcher: скін"}});
    fs::write(pack_dir.join("pack.mcmeta"), serde_json::to_vec_pretty(&meta).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    set_pack_enabled(&opts, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
        v.extend([0, 0, 0, 13]); v.extend(b"IHDR"); v.extend(w.to_be_bytes()); v.extend(h.to_be_bytes()); v.extend([8, 6, 0, 0, 0]);
        v
    }
    #[test] fn validates() {
        assert!(validate_skin(&png(64, 64)).is_ok());
        assert!(validate_skin(&png(64, 32)).is_err());
        assert!(validate_skin(b"hello world hello world hello").is_err());
    }
    #[test] fn formats() { assert_eq!(pack_format("1.21.11"), 75); assert_eq!(pack_format("1.21.4"), 46); assert_eq!(pack_format("1.8.9"), 1); assert_eq!(pack_format("25w10a"), 75); }
    #[test] fn options_roundtrip() {
        let d = std::env::temp_dir().join("skin_opts_test"); let _ = fs::remove_dir_all(&d); fs::create_dir_all(&d).unwrap();
        let o = d.join("options.txt");
        set_pack_enabled(&o, true).unwrap();
        assert!(fs::read_to_string(&o).unwrap().contains(r#"resourcePacks:["vanilla","file/VioletSkin"]"#));
        fs::write(&o, "lang:uk_ua\nresourcePacks:[\"vanilla\",\"file/Other\"]\n").unwrap();
        set_pack_enabled(&o, true).unwrap(); set_pack_enabled(&o, true).unwrap();
        let t = fs::read_to_string(&o).unwrap();
        assert!(t.contains(r#"["vanilla","file/Other","file/VioletSkin"]"#) && t.contains("lang:uk_ua") && t.matches("VioletSkin").count() == 1);
        set_pack_enabled(&o, false).unwrap();
        assert!(fs::read_to_string(&o).unwrap().contains(r#"["vanilla","file/Other"]"#));
    }
}
